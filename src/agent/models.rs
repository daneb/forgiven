use anyhow::{Context, Result};
use tracing::{info, warn};

use super::provider::ProviderKind;
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
                    let display = name.split(':').next().unwrap_or(name.as_str()).to_string();
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
    } else if n.contains("deepseek")
        || n.contains("llama3")
        || n.contains("llama-3")
        || n.contains("gemma3")
        || n.contains("gemma-3")
    {
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

// ─────────────────────────────────────────────────────────────────────────────
// Anthropic — static model list
// ─────────────────────────────────────────────────────────────────────────────

/// Return a static list of known Anthropic models.
///
/// The Anthropic OpenAI-compat `/models` endpoint is not reliably populated,
/// so we maintain a static list seeded at release time.  The list is sorted
/// alphabetically by ID and all models share the 200 000-token context window.
pub(super) fn fetch_models_anthropic() -> Vec<ModelVersion> {
    let mut models = vec![
        ModelVersion {
            id: "claude-haiku-4-5-20251001".to_string(),
            version: "claude-haiku-4-5-20251001".to_string(),
            name: "Claude Haiku 4.5".to_string(),
            context_window: 200_000,
        },
        ModelVersion {
            id: "claude-sonnet-4-6".to_string(),
            version: "claude-sonnet-4-6".to_string(),
            name: "Claude Sonnet 4.6".to_string(),
            context_window: 200_000,
        },
        ModelVersion {
            id: "claude-opus-4-6".to_string(),
            version: "claude-opus-4-6".to_string(),
            name: "Claude Opus 4.6".to_string(),
            context_window: 200_000,
        },
    ];
    models.sort_by(|a, b| a.id.cmp(&b.id));
    info!("[anthropic] {} static model(s)", models.len());
    models
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenAI — dynamic list with static context-window lookup
// ─────────────────────────────────────────────────────────────────────────────

/// Context-window sizes for known OpenAI models.
/// The `/models` endpoint does not return this value, so we look it up.
fn openai_context_window(id: &str) -> u32 {
    let n = id.to_lowercase();
    if n.contains("gpt-4o")
        || n.contains("gpt-4.1")
        || n.contains("o3")
        || n.contains("o4")
        || n.contains("gpt-4-turbo")
    {
        128_000
    } else if n.contains("gpt-4-32k") {
        32_768
    } else if n.contains("gpt-4") {
        8_192
    } else if n.contains("gpt-3.5-turbo-16k") {
        16_384
    } else if n.contains("gpt-3.5") {
        16_385
    } else {
        128_000 // safe default for unknown future models
    }
}

/// Fetch chat-capable models from the OpenAI `/models` endpoint.
pub(super) async fn fetch_models_openai(
    api_token: &str,
    base_url: &str,
) -> Result<Vec<ModelVersion>> {
    let url = format!("{base_url}/models");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_token}"))
        .header("User-Agent", "forgiven/0.1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("OpenAI /models: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("OpenAI /models error ({status}): {body}"));
    }

    let body: serde_json::Value =
        resp.json().await.context("OpenAI /models response is not JSON")?;

    let mut models: Vec<ModelVersion> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    // Keep only chat/reasoning-capable models.
                    if id.contains("embed")
                        || id.contains("whisper")
                        || id.contains("tts")
                        || id.contains("dall")
                        || id.contains("babbage")
                        || id.contains("davinci")
                    {
                        return None;
                    }
                    // Only keep gpt- and o- prefixed models (reasoning/chat).
                    if !id.starts_with("gpt-")
                        && !id.starts_with("o1")
                        && !id.starts_with("o3")
                        && !id.starts_with("o4")
                    {
                        return None;
                    }
                    let context_window = openai_context_window(&id);
                    Some(ModelVersion {
                        id: id.clone(),
                        version: id.clone(),
                        name: id.clone(),
                        context_window,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);

    info!(
        "[openai] {} model(s): {:?}",
        models.len(),
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
    Ok(models)
}

// ─────────────────────────────────────────────────────────────────────────────
// Gemini — dynamic list via OpenAI-compat endpoint
// ─────────────────────────────────────────────────────────────────────────────

pub(super) async fn fetch_models_gemini(api_token: &str) -> Result<Vec<ModelVersion>> {
    let url = "https://generativelanguage.googleapis.com/v1beta/openai/models";
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {api_token}"))
        .header("User-Agent", "forgiven/0.1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Gemini /models: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Gemini /models error ({status}): {body}"));
    }

    let body: serde_json::Value =
        resp.json().await.context("Gemini /models response is not JSON")?;

    let mut models: Vec<ModelVersion> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    // Keep only generative (non-embedding) models.
                    if id.contains("embed") || id.contains("aqa") {
                        return None;
                    }
                    // Context window from the compat response, or 1M fallback.
                    let context_window =
                        v.get("context_window").and_then(|x| x.as_u64()).unwrap_or(1_000_000)
                            as u32;
                    Some(ModelVersion {
                        id: id.clone(),
                        version: id.clone(),
                        name: id.clone(),
                        context_window,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);

    info!(
        "[gemini] {} model(s): {:?}",
        models.len(),
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
    Ok(models)
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenRouter — full catalogue with context_length per model
// ─────────────────────────────────────────────────────────────────────────────

pub(super) async fn fetch_models_openrouter(api_token: &str) -> Result<Vec<ModelVersion>> {
    let url = "https://openrouter.ai/api/v1/models";
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default();

    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {api_token}"))
        .header("User-Agent", "forgiven/0.1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("OpenRouter /models: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("OpenRouter /models error ({status}): {body}"));
    }

    let body: serde_json::Value =
        resp.json().await.context("OpenRouter /models response is not JSON")?;

    let mut models: Vec<ModelVersion> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    let name = v
                        .get("name")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(id.as_str())
                        .to_string();
                    let context_window =
                        v.get("context_length").and_then(|x| x.as_u64()).unwrap_or(128_000) as u32;
                    Some(ModelVersion { id: id.clone(), version: id, name, context_window })
                })
                .collect()
        })
        .unwrap_or_default();

    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);

    info!("[openrouter] {} model(s) available", models.len());
    Ok(models)
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch models for any provider.  Used by `ensure_models` and `refresh_models`
/// so those methods don't need to pattern-match on provider kind themselves.
pub(super) async fn fetch_models_for_provider(
    kind: &ProviderKind,
    api_token: &str,
    ollama_base_url: &str,
    ollama_context_length: Option<u32>,
    openai_base_url: &str,
) -> Result<Vec<ModelVersion>> {
    match kind {
        ProviderKind::Copilot => fetch_models(api_token).await,
        ProviderKind::Ollama => fetch_models_ollama(ollama_base_url, ollama_context_length).await,
        ProviderKind::Anthropic => Ok(fetch_models_anthropic()),
        ProviderKind::OpenAi => fetch_models_openai(api_token, openai_base_url).await,
        ProviderKind::Gemini => fetch_models_gemini(api_token).await,
        ProviderKind::OpenRouter => fetch_models_openrouter(api_token).await,
    }
}
