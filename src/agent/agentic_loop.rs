use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::mcp::McpManager;

use super::auth::{exchange_token, load_oauth_token, TokenExpiredError};
use super::tools;
use super::StreamEvent;

// ─────────────────────────────────────────────────────────────────────────────
// LLMLingua transparent compression helper
// ─────────────────────────────────────────────────────────────────────────────

/// Tools whose results must never be compressed — they contain source code
/// where LLMLingua could silently corrupt identifiers, operators, or
/// indentation that the model will use verbatim for edits.
const COMPRESSION_SKIP_TOOLS: &[&str] = &[
    "read_file",
    "get_file_outline",
    "get_symbol_context",
    "write_file",
    "edit_file",
    "list_directory",
    "create_task",
    "complete_task",
    "ask_user",
];

/// Minimum result length (chars) worth sending to LLMLingua.
/// Shorter results compress poorly and the round-trip latency is not worth it.
const COMPRESSION_MIN_CHARS: usize = 2_000;

/// Maximum time to wait for the LLMLingua MCP server to respond.
const COMPRESSION_TIMEOUT_SECS: u64 = 10;

/// If `auto_compress` is enabled and LLMLingua is connected, compress `result`
/// before it enters the conversation history.  Falls back to the original text
/// on timeout, MCP error, or if the tool is code-producing.
async fn maybe_compress(result: String, tool_name: &str, mcp: &crate::mcp::McpManager) -> String {
    if result.len() < COMPRESSION_MIN_CHARS || COMPRESSION_SKIP_TOOLS.contains(&tool_name) {
        return result;
    }

    let args =
        serde_json::json!({ "text": result, "rate": 0.5, "keep_first_sentence": true }).to_string();

    match tokio::time::timeout(
        tokio::time::Duration::from_secs(COMPRESSION_TIMEOUT_SECS),
        mcp.call_tool("compress_text", &args),
    )
    .await
    {
        Ok(compressed)
            if !compressed.starts_with("error")
                && !compressed.starts_with("unknown")
                && !compressed.starts_with("MCP tool error")
                && !compressed.is_empty() =>
        {
            let before_t = result.len() / 4;
            let after_t = compressed.len() / 4;
            let reduction = 100u32
                .saturating_sub((after_t as u32).saturating_mul(100) / (before_t as u32).max(1));
            info!("[llmlingua] {tool_name}: {before_t}t → {after_t}t  ({reduction}% reduction)");
            compressed
        },
        Ok(err) => {
            warn!("[llmlingua] compress_text returned error for {tool_name}: {err}");
            result
        },
        Err(_) => {
            warn!(
                "[llmlingua] compress_text timed out after {COMPRESSION_TIMEOUT_SECS}s \
                 for {tool_name} — using original"
            );
            result
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agentic loop (runs in a background tokio task)
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) async fn agentic_loop(
    mut api_token: String,
    mut messages: Vec<serde_json::Value>,
    project_root: PathBuf,
    tx: mpsc::UnboundedSender<StreamEvent>,
    model_id: String,
    max_rounds: usize,
    warning_threshold: usize,
    mut cont_rx: mpsc::UnboundedReceiver<bool>,
    mut question_rx: mpsc::UnboundedReceiver<String>,
    mut abort_rx: oneshot::Receiver<()>,
    mcp_manager: Option<Arc<McpManager>>,
    auto_compress: bool,
) {
    // Merge built-in tools with any tools provided by MCP servers.
    let mut tool_defs = tools::tool_definitions();
    if let Some(ref mcp) = mcp_manager {
        let mcp_tools = mcp.tool_definitions();
        if !mcp_tools.is_empty() {
            info!("Agentic loop: adding {} MCP tools", mcp_tools.len());
            tool_defs
                .as_array_mut()
                .expect("tool_definitions() always returns a JSON array")
                .extend(mcp_tools);
        }
    }

    // Use a manual counter so we can extend the limit when the user approves
    // continuation. A `for round in 0..max_rounds` loop cannot be extended
    // mid-flight — `continue` at the last iteration simply exits the loop.
    let mut round = 0usize;
    let mut effective_max = max_rounds;
    let mut warned = false; // emit the MaxRoundsWarning only once

    loop {
        if round >= effective_max {
            // Only reached if we exhausted all rounds without the model stopping.
            let _ = tx.send(StreamEvent::Error(format!(
                "Agent reached maximum rounds ({effective_max}) without completing. \
                 Consider increasing max_agent_rounds in config."
            )));
            return;
        }

        // Report progress
        let _ = tx.send(StreamEvent::RoundProgress { current: round + 1, max: effective_max });

        // Warn once when approaching the limit
        let remaining = effective_max.saturating_sub(round + 1);
        if !warned && remaining <= warning_threshold && remaining > 0 {
            warned = true;
            let _ = tx.send(StreamEvent::MaxRoundsWarning {
                current: round + 1,
                max: effective_max,
                remaining,
            });
        }

        // ── Call the API (cancellable) ────────────────────────────────────────
        let api_result = tokio::select! {
            // User pressed Ctrl+C — abort immediately, no error shown.
            _ = &mut abort_rx => {
                let _ = tx.send(StreamEvent::Done);
                return;
            }
            res = start_chat_stream_with_tools(
                api_token.clone(),
                messages.clone(),
                tool_defs.clone(),
                &model_id,
                &tx,
            ) => res,
        };

        // On token expiry: refresh the API token once and retry the call.  A second
        // 401 after a fresh token means a genuine auth failure — surface it as an error.
        let api_result = match api_result {
            Err(ref e) if e.is::<TokenExpiredError>() => {
                warn!("API token expired mid-session — refreshing and retrying this round");
                match load_oauth_token() {
                    Ok(oauth) => match exchange_token(&oauth).await {
                        Ok(new_tok) => {
                            info!("Token refreshed successfully");
                            api_token = new_tok.token;
                            start_chat_stream_with_tools(
                                api_token.clone(),
                                messages.clone(),
                                tool_defs.clone(),
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
                let _ = tx.send(StreamEvent::Error(format!("{e}")));
                return;
            },
        };

        // ── Parse the SSE stream ──────────────────────────────────────────────
        let mut text_buf = String::new();
        let mut partial_tools: HashMap<usize, tools::PartialToolCall> = HashMap::new();
        let mut sse_buf = String::new();
        let mut byte_stream = response.bytes_stream();
        const STREAM_TIMEOUT_SECS: u64 = 60; // Timeout if no data for 60 seconds

        'sse: loop {
            // Wrap stream read in timeout to detect stalled connections
            let item = match tokio::time::timeout(
                tokio::time::Duration::from_secs(STREAM_TIMEOUT_SECS),
                byte_stream.next(),
            )
            .await
            {
                Ok(Some(result)) => result,
                Ok(None) => break 'sse, // Stream ended normally
                Err(_) => {
                    warn!("Stream timeout after {STREAM_TIMEOUT_SECS}s with no data");
                    let _ = tx
                        .send(StreamEvent::Error("Stream stalled - no data received".to_string()));
                    break 'sse;
                },
            };

            match item {
                Ok(bytes) => {
                    sse_buf.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(pos) = sse_buf.find('\n') {
                        let line = sse_buf[..pos].trim().to_string();
                        sse_buf.drain(..=pos);

                        if line == "data: [DONE]" {
                            break 'sse;
                        }

                        if let Some(json_str) = line.strip_prefix("data: ") {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                // Detect if the API silently routed to a different model (e.g. premium quota exceeded).
                                if text_buf.is_empty() && partial_tools.is_empty() {
                                    if let Some(actual) = val.get("model").and_then(|v| v.as_str())
                                    {
                                        info!(
                                            "[stream] API routed request to model={actual:?}  \
                                             (requested={model_id:?})"
                                        );
                                        // Only flag a real switch — not a dated alias of the same
                                        // model (e.g. "gpt-4.1" → "gpt-4.1-2025-04-14").
                                        let is_alias = actual.starts_with(model_id.as_str())
                                            && actual
                                                .get(model_id.len()..)
                                                .is_some_and(|s| s.starts_with('-'));
                                        if actual != model_id && !is_alias {
                                            let _ = tx.send(StreamEvent::ModelSwitched {
                                                from: model_id.to_string(),
                                                to: actual.to_string(),
                                            });
                                        }
                                    }
                                }
                                // Text content delta
                                if let Some(content) =
                                    val.pointer("/choices/0/delta/content").and_then(|v| v.as_str())
                                {
                                    if !content.is_empty() {
                                        text_buf.push_str(content);
                                        let _ = tx.send(StreamEvent::Token(content.to_string()));
                                    }
                                }

                                // Tool call delta
                                if let Some(tc_arr) = val
                                    .pointer("/choices/0/delta/tool_calls")
                                    .and_then(|v| v.as_array())
                                {
                                    for tc_val in tc_arr {
                                        let idx = tc_val
                                            .get("index")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as usize;
                                        let entry = partial_tools.entry(idx).or_default();

                                        if let Some(id) = tc_val.get("id").and_then(|v| v.as_str())
                                        {
                                            if !id.is_empty() {
                                                entry.id = id.to_string();
                                            }
                                        }
                                        if let Some(name) = tc_val
                                            .pointer("/function/name")
                                            .and_then(|v| v.as_str())
                                        {
                                            if !name.is_empty() {
                                                entry.name = name.to_string();
                                            }
                                        }
                                        if let Some(chunk) = tc_val
                                            .pointer("/function/arguments")
                                            .and_then(|v| v.as_str())
                                        {
                                            entry.arguments.push_str(chunk);
                                        }
                                    }
                                }

                                // Usage chunk (emitted by OpenAI-compatible APIs when stream_options.include_usage=true)
                                if let Some(usage) = val.get("usage").filter(|v| !v.is_null()) {
                                    let p = usage
                                        .get("prompt_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                    let c = usage
                                        .get("completion_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                    // OpenAI returns cached token count under
                                    // usage.prompt_tokens_details.cached_tokens when
                                    // automatic prompt caching is active.
                                    let cached = usage
                                        .pointer("/prompt_tokens_details/cached_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32;
                                    if p > 0 || c > 0 {
                                        let _ = tx.send(StreamEvent::Usage {
                                            prompt_tokens: p,
                                            completion_tokens: c,
                                            cached_tokens: cached,
                                        });
                                    }
                                }
                            }
                        }
                    }
                },
                Err(e) => {
                    warn!("Stream error, attempting to process buffered data: {e}");
                    // Try to salvage any complete lines from the buffer
                    while let Some(pos) = sse_buf.find('\n') {
                        let line = sse_buf[..pos].trim().to_string();
                        sse_buf.drain(..=pos);
                        if line == "data: [DONE]" {
                            break;
                        }
                        if let Some(json_str) = line.strip_prefix("data: ") {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                // Process any text content
                                if let Some(content) =
                                    val.pointer("/choices/0/delta/content").and_then(|v| v.as_str())
                                {
                                    if !content.is_empty() {
                                        text_buf.push_str(content);
                                    }
                                }
                            }
                        }
                    }
                    let _ = tx.send(StreamEvent::Error(format!("{e}")));
                    return;
                },
            }
        }

        // ── Process any remaining data in buffer after stream ends ────────────
        if !sse_buf.is_empty() {
            debug!("Processing {} bytes of remaining buffer data", sse_buf.len());
            // Split by newlines and process any complete SSE events
            for line in sse_buf.lines() {
                let line = line.trim();
                if line == "data: [DONE]" {
                    break;
                }
                if let Some(json_str) = line.strip_prefix("data: ") {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                        // Process text content delta
                        if let Some(content) =
                            val.pointer("/choices/0/delta/content").and_then(|v| v.as_str())
                        {
                            if !content.is_empty() {
                                text_buf.push_str(content);
                                let _ = tx.send(StreamEvent::Token(content.to_string()));
                            }
                        }
                        // Process tool call deltas
                        if let Some(tc_arr) =
                            val.pointer("/choices/0/delta/tool_calls").and_then(|v| v.as_array())
                        {
                            for tc_val in tc_arr {
                                let idx = tc_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0)
                                    as usize;
                                let entry = partial_tools.entry(idx).or_default();
                                if let Some(id) = tc_val.get("id").and_then(|v| v.as_str()) {
                                    if !id.is_empty() {
                                        entry.id = id.to_string();
                                    }
                                }
                                if let Some(name) =
                                    tc_val.pointer("/function/name").and_then(|v| v.as_str())
                                {
                                    if !name.is_empty() {
                                        entry.name = name.to_string();
                                    }
                                }
                                if let Some(chunk) =
                                    tc_val.pointer("/function/arguments").and_then(|v| v.as_str())
                                {
                                    entry.arguments.push_str(chunk);
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── No tool calls → plain text response, done ─────────────────────────
        if partial_tools.is_empty() {
            if !text_buf.is_empty() {
                messages.push(serde_json::json!({ "role": "assistant", "content": text_buf }));
            }
            let _ = tx.send(StreamEvent::Done);
            return;
        }

        // ── Tool calls → execute and loop ─────────────────────────────────────
        let mut sorted: Vec<(usize, tools::PartialToolCall)> = partial_tools.into_iter().collect();
        sorted.sort_by_key(|(idx, _)| *idx);

        let tool_calls_json: Vec<serde_json::Value> = sorted
            .iter()
            .map(|(_, tc)| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": { "name": tc.name, "arguments": tc.arguments }
                })
            })
            .collect();

        messages.push(serde_json::json!({
            "role": "assistant",
            "content": if text_buf.is_empty() { serde_json::Value::Null } else { serde_json::json!(text_buf) },
            "tool_calls": tool_calls_json
        }));

        for (_, partial) in sorted {
            let call = tools::ToolCall {
                id: partial.id.clone(),
                name: partial.name.clone(),
                arguments: partial.arguments.clone(),
            };

            let _ = tx.send(StreamEvent::ToolStart {
                name: call.name.clone(),
                args_summary: call.args_summary(),
            });

            let result = if call.name == "ask_user" {
                // Parse question + options, emit an AskingUser event, and block until
                // the user makes a selection (or the 5-minute timeout fires).
                let args_val =
                    serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default();
                let question = args_val
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Please choose an option")
                    .to_string();
                let options: Vec<String> = args_val
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter().filter_map(|o| o.as_str().map(|s| s.to_string())).collect()
                    })
                    .filter(|v: &Vec<String>| !v.is_empty())
                    .unwrap_or_else(|| vec!["Yes".to_string(), "No".to_string()]);

                let _ = tx.send(StreamEvent::AskingUser {
                    question: question.clone(),
                    options: options.clone(),
                });

                let answer = tokio::select! {
                    // Ctrl+C while the dialog is open — abort the whole loop.
                    _ = &mut abort_rx => {
                        let _ = tx.send(StreamEvent::Done);
                        return;
                    }
                    res = tokio::time::timeout(
                        tokio::time::Duration::from_secs(300),
                        question_rx.recv(),
                    ) => res,
                };

                match answer {
                    Ok(Some(ans)) => ans,
                    Ok(None) | Err(_) => {
                        options.last().cloned().unwrap_or_else(|| "No".to_string())
                    },
                }
            } else if let Some(ref mcp) = mcp_manager {
                if mcp.is_mcp_tool(&call.name) {
                    mcp.call_tool(&call.name, &call.arguments).await
                } else {
                    tools::execute_tool(&call, &project_root)
                }
            } else {
                tools::execute_tool(&call, &project_root)
            };

            // If a file was successfully written or edited, notify the editor
            // so it can reload any open buffer for that path.
            if matches!(call.name.as_str(), "write_file" | "edit_file")
                && !result.starts_with("error")
            {
                if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) {
                    if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
                        let _ = tx.send(StreamEvent::FileModified { path: p.to_string() });
                    }
                }
            }

            // Forward task lifecycle events to the agent panel's task strip.
            if matches!(call.name.as_str(), "create_task" | "complete_task")
                && !result.starts_with("error")
            {
                if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) {
                    if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
                        let event = if call.name == "create_task" {
                            StreamEvent::TaskCreated { title: title.to_string() }
                        } else {
                            StreamEvent::TaskCompleted { title: title.to_string() }
                        };
                        let _ = tx.send(event);
                    }
                }
            }

            // Optionally compress the result before it enters history.
            // Only fires when auto_compress is on, LLMLingua is connected,
            // and the tool is not a code-reading tool.
            let result = if auto_compress {
                if let Some(ref mcp) = mcp_manager {
                    if mcp.is_mcp_tool("compress_text") {
                        maybe_compress(result, &call.name, mcp).await
                    } else {
                        result
                    }
                } else {
                    result
                }
            } else {
                result
            };

            let result_summary = {
                // Prefer "path (N lines)" summary lines (read_file header) over
                // raw content lines.  Also skip lines that look like code
                // signatures (end with ':' or contain '(' without ' lines)').
                let meaningful = result
                    .lines()
                    .find(|l| {
                        let t = l.trim();
                        (!t.is_empty()
                            && !t.ends_with(':')
                            && (!t.contains('(') || t.contains(" lines)")))
                            || t.starts_with("error")
                    })
                    .unwrap_or_else(|| result.lines().next().unwrap_or("ok"));
                // Strip leading whitespace and any "N | " line-number prefix
                // that read_file injects into numbered content lines.
                let s = {
                    let t = meaningful.trim();
                    if let Some(pos) = t.find(" | ") {
                        if t[..pos].chars().all(|c| c.is_ascii_digit()) {
                            t[pos + 3..].trim()
                        } else {
                            t
                        }
                    } else {
                        t
                    }
                };
                // Truncate by char count (not bytes) to avoid multibyte panics.
                if s.chars().count() > 120 {
                    format!("{}…", s.chars().take(120).collect::<String>())
                } else {
                    s.to_string()
                }
            };
            let _ = tx.send(StreamEvent::ToolDone { name: call.name.clone(), result_summary });

            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": partial.id,
                "content": result
            }));
        }

        // Paragraph break between the tool-call lines and the next LLM response.
        // A single \n is only a soft break in CommonMark — the LLM text would
        // merge into the ⚙ paragraph and render as dim-gray.  Two newlines
        // create a proper paragraph boundary so the response renders normally.
        let _ = tx.send(StreamEvent::Token("\n\n".to_string()));

        round += 1;

        // Check if we've hit the limit and need user approval to continue
        if round >= effective_max {
            let _ = tx.send(StreamEvent::AwaitingContinuation);

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
                    let _ = tx.send(StreamEvent::Done);
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
    api_token: String,
    messages: Vec<serde_json::Value>,
    tools: serde_json::Value,
    model_id: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) -> Result<reqwest::Response> {
    info!("Sending completion request with model_id={model_id:?}");
    let body = serde_json::json!({
        "model": model_id,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "stream": true,
        "stream_options": { "include_usage": true },
        "n": 1,
        "temperature": 0.1,
        "max_tokens": 4096
    });

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let mut retry_attempts = 0;
    let max_retries = 5;
    let mut delay = tokio::time::Duration::from_secs(1);

    loop {
        let resp = client
            .post("https://api.githubcopilot.com/chat/completions")
            .header("Authorization", format!("Bearer {api_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("Copilot-Integration-Id", "vscode-chat")
            .header("editor-version", "forgiven/0.1.0")
            .header("editor-plugin-version", "forgiven-copilot/0.1.0")
            .header("openai-intent", "conversation-panel")
            .header("User-Agent", "forgiven/0.1.0")
            .json(&body)
            .send()
            .await;

        let failure_reason = match resp {
            Ok(response) if response.status().is_success() => {
                info!("Copilot Chat stream started ({})", response.status());
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
                let body = response.text().await.unwrap_or_default();
                // 401 means the short-lived API token expired — caller will refresh and retry.
                if status.as_u16() == 401 {
                    return Err(anyhow::Error::new(TokenExpiredError));
                }
                if status.as_u16() == 429 {
                    // If the server says to wait more than 2 minutes, the quota is likely
                    // exhausted for the session — fail fast with a helpful message.
                    if retry_after_secs.unwrap_or(0) > 120 {
                        return Err(anyhow::anyhow!(
                            "Copilot rate limit: quota exhausted (Retry-After: {}s). \
                             Try again later or switch models with Ctrl+T.",
                            retry_after_secs.unwrap_or(0)
                        ));
                    }
                    // Respect Retry-After if provided; otherwise fall through to exponential backoff.
                    if let Some(secs) = retry_after_secs {
                        delay = tokio::time::Duration::from_secs(secs);
                    }
                    warn!("Rate limited (429), retrying after {}s: {body}", delay.as_secs());
                    let _ = tx.send(StreamEvent::Retrying {
                        attempt: retry_attempts + 1,
                        max: max_retries,
                    });
                    tokio::time::sleep(delay).await;
                    retry_attempts += 1;
                    if retry_attempts >= max_retries {
                        return Err(anyhow::anyhow!(
                            "Max retries reached for Copilot Chat API (last error: HTTP 429 Too Many Requests)"
                        ));
                    }
                    delay *= 2;
                    continue;
                }
                // Other 4xx errors are permanent — don't retry.
                if status.is_client_error() {
                    return Err(anyhow::anyhow!("Copilot Chat API error ({status}): {body}"));
                }
                warn!("Retrying due to API error ({status}): {body}");
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
                "Max retries reached for Copilot Chat API (last error: {failure_reason})"
            ));
        }

        let _ = tx.send(StreamEvent::Retrying { attempt: retry_attempts, max: max_retries });
        tokio::time::sleep(delay).await;
        delay *= 2; // Exponential backoff
    }
}
