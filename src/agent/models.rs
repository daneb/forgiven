use anyhow::{Context, Result};
use tracing::{info, warn};

use super::ModelVersion;

// ─────────────────────────────────────────────────────────────────────────────
// Ollama model discovery
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch available models from a local Ollama server via `GET /api/tags`.
///
/// Ollama does not report context-window sizes in this endpoint.  We apply a
/// family-based heuristic and honour an explicit `context_length_override` from
/// the config (strongly recommended — set it to the value you pass as `num_ctx`
/// so history truncation uses the same budget).
pub(super) async fn fetch_models_ollama(
    base_url: &str,
    context_length_override: Option<u32>,
) -> Result<Vec<ModelVersion>> {
    let url = format!("{base_url}/api/tags");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let resp = client
        .get(&url)
        .header("User-Agent", "forgiven/0.1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Ollama /api/tags: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Ollama /api/tags error ({status}): {body}"));
    }

    let body: serde_json::Value =
        resp.json().await.context("Ollama /api/tags response is not JSON")?;

    let mut models: Vec<ModelVersion> = body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let name = v.get("name")?.as_str()?.to_string();
                    let context_window = context_length_override
                        .unwrap_or_else(|| infer_ollama_context_window(&name));
                    // version = human-readable parameter size (e.g. "14.8B") from details.
                    let version = v
                        .pointer("/details/parameter_size")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    // Display name: strip tag suffix for panel clarity.
                    // "qwen2.5-coder:14b" → "qwen2.5-coder"
                    let display = name
                        .split(':')
                        .next()
                        .unwrap_or(name.as_str())
                        .to_string();
                    Some(ModelVersion { id: name, version, name: display, context_window })
                })
                .collect()
        })
        .unwrap_or_default();

    models.sort_by(|a, b| a.id.cmp(&b.id));

    info!(
        "[ollama] {} model(s): {:?}",
        models.len(),
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );

    Ok(models)
}

/// Heuristic context-window sizes for common Ollama model families.
///
/// These represent the model's MAXIMUM trained context — the actual active
/// KV-cache window is pinned separately via `num_ctx` in the request options.
/// Used only when `context_length` is not set in config.
fn infer_ollama_context_window(name: &str) -> u32 {
    let n = name.to_lowercase();
    if n.contains("qwen2.5") || n.contains("qwen3") {
        32_768
    } else if n.contains("deepseek") {
        131_072
    } else if n.contains("llama3") || n.contains("llama-3") {
        131_072
    } else if n.contains("gemma3") || n.contains("gemma-3") {
        131_072
    } else if n.contains("phi4") || n.contains("phi-4") {
        16_384
    } else if n.contains("mistral") || n.contains("mixtral") {
        32_768
    } else {
        // Conservative fallback — better to over-truncate than to send more
        // tokens than the model can handle.
        8_192
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Model discovery
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch chat/agent-capable models from the Copilot `/models` endpoint.
/// Returns `ModelVersion` entries sorted alphabetically by id.
pub(super) async fn fetch_models(api_token: &str) -> Result<Vec<ModelVersion>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let mut retry_attempts = 0;
    let max_retries = 5;
    let mut delay = tokio::time::Duration::from_secs(1);

    loop {
        let resp = client
            .get("https://api.githubcopilot.com/models")
            .header("Authorization", format!("Bearer {api_token}"))
            .header("User-Agent", "forgiven/0.1.0")
            .header("Copilot-Integration-Id", "vscode-chat")
            .header("editor-version", "forgiven/0.1.0")
            .header("editor-plugin-version", "forgiven-copilot/0.1.0")
            .send()
            .await;

        match resp {
            Ok(response) if response.status().is_success() => {
                let body: serde_json::Value =
                    response.json().await.context("/models response is not JSON")?;

                // Log every model entry from the raw response so mismatches are diagnosable.
                if let Some(arr) = body.get("data").and_then(|d| d.as_array()) {
                    for v in arr {
                        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
                        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                        let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
                        let cap_type =
                            v.pointer("/capabilities/type").and_then(|x| x.as_str()).unwrap_or("?");
                        info!(
                            "[models] id={id:?} name={name:?} version={version:?} \
                             cap_type={cap_type:?}"
                        );
                    }
                }

                let mut models: Vec<ModelVersion> = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                let id = v.get("id")?.as_str()?.to_string();
                                // Filter models that clearly don't support /chat/completions.
                                // Embedding, TTS, and image-generation models fail with
                                // unsupported_api_for_model when sent to the chat endpoint.
                                // Note: "codex" is NOT filtered — newer GPT-5.x-Codex models
                                // are chat/agent-capable and should be included.
                                if id.contains("embed")
                                    || id.contains("whisper")
                                    || id.contains("tts")
                                    || id.contains("dall")
                                {
                                    return None;
                                }
                                // Filter by capabilities.type if present: keep "chat" and "agent" models.
                                if let Some(cap_type) =
                                    v.pointer("/capabilities/type").and_then(|x| x.as_str())
                                {
                                    if cap_type != "chat" && cap_type != "agent" {
                                        return None;
                                    }
                                }
                                // `version` is informational metadata; fall back to id if absent.
                                let version = v
                                    .get("version")
                                    .and_then(|x| x.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(&id)
                                    .to_string();
                                // Use the human-readable `name` for display; fall back to the id.
                                let name = v
                                    .get("name")
                                    .and_then(|x| x.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(&id)
                                    .to_string();
                                // Context window from API; fall back to 128k if not provided.
                                let context_window = v
                                    .pointer("/capabilities/limits/max_context_window_tokens")
                                    .and_then(|x| x.as_u64())
                                    .unwrap_or(128_000)
                                    as u32;
                                Some(ModelVersion { id, version, name, context_window })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                models.sort_by(|a, b| a.id.cmp(&b.id));
                // The API sometimes returns duplicate IDs; deduplicate after sorting.
                models.dedup_by(|a, b| a.id == b.id);

                info!(
                    "Filtered+sorted model list: {:?}",
                    models.iter().map(|m| format!("{} ({})", m.name, m.id)).collect::<Vec<_>>()
                );
                return Ok(models);
            },
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                // 4xx errors (except 429 rate-limit) are permanent — don't retry.
                if status.is_client_error() && status.as_u16() != 429 {
                    return Err(anyhow::anyhow!("/models API error ({status}): {body}"));
                }
                warn!("Retrying due to API error ({status}): {body}");
            },
            Err(e) => {
                warn!("Retrying due to network error: {e}");
            },
        }

        retry_attempts += 1;
        if retry_attempts >= max_retries {
            return Err(anyhow::anyhow!("Max retries reached for Copilot /models API"));
        }

        tokio::time::sleep(delay).await;
        delay *= 2; // Exponential backoff
    }
}
