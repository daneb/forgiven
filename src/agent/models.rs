use anyhow::{Context, Result};
use tracing::{info, warn};

use super::ModelVersion;

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
