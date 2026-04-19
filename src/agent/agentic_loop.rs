use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use crate::mcp::McpManager;

use super::auth::{exchange_token, load_oauth_token, TokenExpiredError};
use super::provider::{ProviderKind, ProviderSettings};
use super::streaming::{parse_sse_stream, StreamOutcome};
use super::tool_dispatch::{dispatch_tools, DispatchOutcome};
use super::tools;
use super::StreamEvent;

// ─────────────────────────────────────────────────────────────────────────────
// Agentic loop (runs in a background tokio task)
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) async fn agentic_loop(
    mut provider: ProviderSettings,
    mut messages: Vec<serde_json::Value>,
    project_root: PathBuf,
    tx: mpsc::Sender<StreamEvent>,
    model_id: String,
    max_rounds: usize,
    warning_threshold: usize,
    mut cont_rx: mpsc::UnboundedReceiver<bool>,
    mut question_rx: mpsc::UnboundedReceiver<String>,
    mut abort_rx: oneshot::Receiver<()>,
    mcp_manager: Option<Arc<McpManager>>,
    auto_compress: bool,
    expand_threshold: usize,
) {
    // Merge built-in tools with any tools provided by MCP servers.
    // When the provider does not support tool calling (e.g. Ollama with an
    // unverified model), send an empty list so the model never attempts to
    // output tool calls — many local models emit calls as raw JSON text rather
    // than the structured OpenAI tool_calls delta, which would pollute the panel.
    let tool_defs = Arc::new(if provider.supports_tool_calls {
        let mut defs = tools::tool_definitions();
        if let Some(ref mcp) = mcp_manager {
            let mcp_tools = mcp.tool_definitions();
            if !mcp_tools.is_empty() {
                info!("Agentic loop: adding {} MCP tools", mcp_tools.len());
                defs.as_array_mut()
                    .expect("tool_definitions() always returns a JSON array")
                    .extend(mcp_tools);
            }
        }
        // Strip planning / meta tools when disabled (small models misuse them).
        if !provider.planning_tools {
            const PLANNING: &[&str] = &["create_task", "complete_task", "ask_user"];
            if let Some(arr) = defs.as_array_mut() {
                arr.retain(|tool| {
                    tool.get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        .map(|name| !PLANNING.contains(&name))
                        .unwrap_or(true)
                });
            }
            info!(
                "Agentic loop: planning tools disabled for {} (planning_tools = false)",
                provider.kind.display_name()
            );
        }
        defs
    } else {
        info!(
            "Agentic loop: tool calling disabled for {} — running in chat-only mode",
            provider.kind.display_name()
        );
        serde_json::Value::Array(vec![])
    });

    // Use a manual counter so we can extend the limit when the user approves
    // continuation. A `for round in 0..max_rounds` loop cannot be extended
    // mid-flight — `continue` at the last iteration simply exits the loop.
    let mut round = 0usize;
    let mut effective_max = max_rounds;
    let mut warned = false; // emit the MaxRoundsWarning only once

    // Tracks which project-relative paths have already been snapshotted this
    // session.  Before the first write_file / edit_file for each path, we read
    // the existing content and emit FileSnapshot so the panel can restore it
    // on `SPC a u`.
    let mut snapshotted: std::collections::HashSet<String> = std::collections::HashSet::new();

    // In-memory cache for expand-on-demand tool result truncation (Intervention 1).
    // Maps tool_call_id → full result string. Keyed by tool call ID so the model
    // can request the full content via expand_result(id=...).
    let mut result_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // Soft budget guard for read_file preference (Intervention 2).
    // Fires once per session when ≥3 large files are read in a single round.
    let mut read_hint_fired = false;

    loop {
        if round >= effective_max {
            // Only reached if we exhausted all rounds without the model stopping.
            let _ = tx
                .send(StreamEvent::Error(format!(
                    "Agent reached maximum rounds ({effective_max}) without completing. \
                 Consider increasing max_agent_rounds in config."
                )))
                .await;
            return;
        }

        // Report progress
        let _ =
            tx.send(StreamEvent::RoundProgress { current: round + 1, max: effective_max }).await;

        // Warn once when approaching the limit
        let remaining = effective_max.saturating_sub(round + 1);
        if !warned && remaining <= warning_threshold && remaining > 0 {
            warned = true;
            let _ = tx
                .send(StreamEvent::MaxRoundsWarning {
                    current: round + 1,
                    max: effective_max,
                    remaining,
                })
                .await;
        }

        // ── Call the API (cancellable) ────────────────────────────────────────
        let api_result = tokio::select! {
            // User pressed Ctrl+C — abort immediately, no error shown.
            _ = &mut abort_rx => {
                let _ = tx.send(StreamEvent::Done).await;
                return;
            }
            res = start_chat_stream_with_tools(
                &provider,
                &messages,
                Arc::clone(&tool_defs),
                &model_id,
                &tx,
            ) => res,
        };

        // On token expiry (Copilot only): refresh the API token once and retry the
        // call.  A second 401 after a fresh token means a genuine auth failure.
        // Ollama uses no auth so it never returns TokenExpiredError.
        let api_result = match api_result {
            Err(ref e) if e.is::<TokenExpiredError>() && provider.kind == ProviderKind::Copilot => {
                warn!("API token expired mid-session — refreshing and retrying this round");
                match load_oauth_token() {
                    Ok(oauth) => match exchange_token(&oauth).await {
                        Ok(new_tok) => {
                            info!("Token refreshed successfully");
                            provider.api_token = new_tok.token;
                            start_chat_stream_with_tools(
                                &provider,
                                &messages,
                                Arc::clone(&tool_defs),
                                &model_id,
                                &tx,
                            )
                            .await
                        },
                        Err(e) => Err(anyhow::anyhow!("Token refresh failed: {e}")),
                    },
                    Err(e) => Err(anyhow::anyhow!("Token refresh failed: {e}")),
                }
            },
            other => other,
        };

        let response = match api_result {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(StreamEvent::Error(format!("{e}"))).await;
                return;
            },
        };

        // ── Parse the SSE stream ──────────────────────────────────────────────
        // Per-provider chunk timeout: local Ollama is fast once warm; cloud
        // needs more headroom for network jitter.
        let chunk_timeout_secs = provider.chunk_timeout_secs();
        let parsed = match parse_sse_stream(response, &tx, &model_id, chunk_timeout_secs).await {
            StreamOutcome::Done(r) => r,
            StreamOutcome::Error => return,
        };
        let text_buf = parsed.text_buf;
        let partial_tools = parsed.partial_tools;

        // ── No tool calls → plain text response, done ─────────────────────────
        if partial_tools.is_empty() {
            if !text_buf.is_empty() {
                messages.push(serde_json::json!({ "role": "assistant", "content": text_buf }));
            }
            let _ = tx.send(StreamEvent::Done).await;
            return;
        }

        // ── Tool calls → execute and loop ─────────────────────────────────────
        let mut sorted: Vec<(usize, tools::PartialToolCall)> = partial_tools.into_iter().collect();
        sorted.sort_by_key(|(idx, _)| *idx);

        let mut large_reads_this_round: usize = 0;
        if let DispatchOutcome::Abort = dispatch_tools(
            sorted,
            &mut messages,
            text_buf,
            &project_root,
            &tx,
            &mut snapshotted,
            mcp_manager.clone(),
            auto_compress,
            &mut abort_rx,
            &mut question_rx,
            &mut result_cache,
            expand_threshold,
            &mut large_reads_this_round,
        )
        .await
        {
            return;
        }

        // Soft budget hint: inject once per session when ≥3 large files (>300 lines)
        // are read in a single round, to nudge the model toward symbol-level tools.
        if !read_hint_fired && large_reads_this_round >= 3 {
            read_hint_fired = true;
            messages.push(serde_json::json!({
                "role": "user",
                "content": "[hint] You have read 3 or more large files this round. \
                             Consider get_file_outline first to locate specific symbols, \
                             then get_symbol_context for targeted reads."
            }));
            info!(
                "[ctx] soft budget hint injected: {large_reads_this_round} large reads this round"
            );
        }

        // Paragraph break between the tool-call lines and the next LLM response.
        // A single \n is only a soft break in CommonMark — the LLM text would
        // merge into the ⚙ paragraph and render as dim-gray.  Two newlines
        // create a proper paragraph boundary so the response renders normally.
        let _ = tx.send(StreamEvent::Token("\n\n".to_string())).await;

        round += 1;

        // Check if we've hit the limit and need user approval to continue
        if round >= effective_max {
            let _ = tx.send(StreamEvent::AwaitingContinuation).await;

            // Wait for user decision (with timeout to avoid hanging forever)
            let decision = tokio::time::timeout(
                tokio::time::Duration::from_secs(300), // 5 minute timeout
                cont_rx.recv(),
            )
            .await;

            match decision {
                Ok(Some(true)) => {
                    // Extend the effective limit by another batch of rounds.
                    info!("User approved continuation, extending by {} rounds", max_rounds);
                    effective_max += max_rounds;
                    warned = false; // re-arm the warning for the new batch
                },
                Ok(Some(false)) | Ok(None) | Err(_) => {
                    // User denied, channel closed, or 5-minute timeout.
                    let _ = tx.send(StreamEvent::Done).await;
                    return;
                },
            }
        }
        // Continue loop with tool results appended to messages
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP
// ─────────────────────────────────────────────────────────────────────────────

pub(super) async fn start_chat_stream_with_tools(
    provider: &ProviderSettings,
    messages: &[serde_json::Value],
    tools: Arc<serde_json::Value>,
    model_id: &str,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<reqwest::Response> {
    info!(
        "Sending completion request model={model_id:?} provider={}",
        provider.kind.display_name()
    );

    // ── Build request body ────────────────────────────────────────────────────
    let mut body = serde_json::json!({
        "model": model_id,
        "messages": messages,
        "stream": true,
        "n": 1,
        "temperature": 0.1,
        "max_tokens": 4096
    });

    // Only attach tool definitions and tool_choice when the provider can use them.
    // Sending an empty tools array with tool_choice="auto" can confuse some models.
    if provider.supports_tool_calls {
        body["tools"] = (*tools).clone();
        body["tool_choice"] = serde_json::json!("auto");
    }

    // stream_options is OpenAI/Copilot-specific — omit for Ollama to avoid
    // breaking older server versions that reject unknown fields.
    if provider.supports_stream_usage() {
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }

    // Ollama: pin the active KV-cache size.  Without this, Ollama may use a
    // server default as low as 4 096 tokens, silently ignoring the context window
    // reported by /api/tags and truncating long conversations.
    if let Some(num_ctx) = provider.num_ctx {
        body["options"] = serde_json::json!({ "num_ctx": num_ctx });
    }

    // ── HTTP client ───────────────────────────────────────────────────────────
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(provider.connect_timeout_secs()))
        .build()
        .unwrap_or_default();

    let mut retry_attempts = 0;
    let max_retries = provider.max_retries();
    let mut delay = tokio::time::Duration::from_secs(1);

    loop {
        // ── Build request with provider-specific headers ──────────────────────
        let mut req = client
            .post(&provider.chat_endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("User-Agent", "forgiven/0.1.0");

        if provider.requires_auth() {
            req = req.header("Authorization", format!("Bearer {}", provider.api_token));
        }

        // Copilot routing hints — unknown to Ollama and harmless to omit.
        if provider.kind == ProviderKind::Copilot {
            req = req
                .header("Copilot-Integration-Id", "vscode-chat")
                .header("editor-version", "forgiven/0.1.0")
                .header("editor-plugin-version", "forgiven-copilot/0.1.0")
                .header("openai-intent", "conversation-panel");
        }
        if provider.kind == ProviderKind::OpenRouter {
            if !provider.openrouter_site_url.is_empty() {
                req = req.header("HTTP-Referer", &provider.openrouter_site_url);
            }
            if !provider.openrouter_app_name.is_empty() {
                req = req.header("X-Title", &provider.openrouter_app_name);
            }
        }

        let resp = req.json(&body).send().await;

        let failure_reason = match resp {
            Ok(response) if response.status().is_success() => {
                info!("{} stream started ({})", provider.kind.display_name(), response.status());
                return Ok(response);
            },
            Ok(response) => {
                let status = response.status();
                // Read Retry-After before consuming the body.
                let retry_after_secs: Option<u64> = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok());
                let body_text = response.text().await.unwrap_or_default();

                // 401 — short-lived API token expired; caller will refresh and retry.
                // (Only relevant for Copilot; Ollama never sends 401.)
                if status.as_u16() == 401 {
                    return Err(anyhow::Error::new(TokenExpiredError));
                }

                if status.as_u16() == 429 {
                    // Quota exhausted — fail fast if the wait is impractical.
                    if retry_after_secs.unwrap_or(0) > 120 {
                        return Err(anyhow::anyhow!(
                            "{} rate limit: quota exhausted (Retry-After: {}s). \
                             Try again later or switch models with Ctrl+T.",
                            provider.kind.display_name(),
                            retry_after_secs.unwrap_or(0)
                        ));
                    }
                    if let Some(secs) = retry_after_secs {
                        delay = tokio::time::Duration::from_secs(secs);
                    }
                    warn!("Rate limited (429), retrying after {}s: {body_text}", delay.as_secs());
                    let _ = tx
                        .send(StreamEvent::Retrying {
                            attempt: retry_attempts + 1,
                            max: max_retries,
                        })
                        .await;
                    tokio::time::sleep(delay).await;
                    retry_attempts += 1;
                    if retry_attempts >= max_retries {
                        return Err(anyhow::anyhow!(
                            "Max retries reached for {} (last error: HTTP 429 Too Many Requests)",
                            provider.kind.display_name()
                        ));
                    }
                    delay *= 2;
                    continue;
                }

                // Other 4xx errors are permanent — don't retry.
                if status.is_client_error() {
                    return Err(anyhow::anyhow!(
                        "{} API error ({status}): {body_text}",
                        provider.kind.display_name()
                    ));
                }
                warn!("Retrying due to API error ({status}): {body_text}");
                format!("HTTP {status}")
            },
            Err(e) => {
                warn!("Retrying due to network error: {e}");
                format!("{e}")
            },
        };

        retry_attempts += 1;
        if retry_attempts >= max_retries {
            return Err(anyhow::anyhow!(
                "Max retries reached for {} (last error: {failure_reason})",
                provider.kind.display_name()
            ));
        }

        let _ = tx.send(StreamEvent::Retrying { attempt: retry_attempts, max: max_retries }).await;
        tokio::time::sleep(delay).await;
        delay *= 2;
    }
}
