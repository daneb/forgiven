use std::collections::HashMap;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::tools;
use super::StreamEvent;

// ─────────────────────────────────────────────────────────────────────────────
// SSE line classification (pure, no I/O — tested below)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum SseLine {
    Done,
    Token(String),
    ToolDelta { index: usize, id: Option<String>, name: Option<String>, args_fragment: String },
    Skip,
}

/// Classify a single SSE line into a strongly-typed variant.
/// Lines that carry model-switch or usage events return `Skip` — the caller
/// is responsible for handling those via the full parsed JSON value.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn parse_sse_line(line: &str) -> SseLine {
    if line == "data: [DONE]" {
        return SseLine::Done;
    }
    let Some(json_str) = line.strip_prefix("data: ") else {
        return SseLine::Skip;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return SseLine::Skip;
    };
    // Token content delta
    if let Some(content) = val.pointer("/choices/0/delta/content").and_then(|v| v.as_str()) {
        if !content.is_empty() {
            return SseLine::Token(content.to_string());
        }
    }
    // Tool call delta — take first entry only (loop caller iterates the rest)
    if let Some(tc_arr) = val.pointer("/choices/0/delta/tool_calls").and_then(|v| v.as_array()) {
        if let Some(tc_val) = tc_arr.first() {
            let index = tc_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let id = tc_val
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let name = tc_val
                .pointer("/function/name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let args_fragment = tc_val
                .pointer("/function/arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return SseLine::ToolDelta { index, id, name, args_fragment };
        }
    }
    SseLine::Skip
}

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
pub(super) async fn maybe_compress(
    result: String,
    tool_name: &str,
    mcp: &crate::mcp::McpManager,
) -> String {
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
// SSE stream parser
// ─────────────────────────────────────────────────────────────────────────────

/// The result of parsing one round's SSE stream.
pub(super) struct ParsedRound {
    pub text_buf: String,
    pub partial_tools: HashMap<usize, tools::PartialToolCall>,
}

/// Outcome returned by `parse_sse_stream` to signal whether the caller should
/// continue or abort (because an error event was already sent via `tx`).
pub(super) enum StreamOutcome {
    Done(ParsedRound),
    /// A stream error occurred; `StreamEvent::Error` has already been sent.
    Error,
}

/// Drive the SSE byte-stream from a single chat completion response to
/// completion, emitting `StreamEvent` variants via `tx` as they arrive.
/// Returns the accumulated text and partial tool calls on success.
pub(super) async fn parse_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    model_id: &str,
    chunk_timeout_secs: u64,
) -> StreamOutcome {
    let mut text_buf = String::new();
    let mut partial_tools: HashMap<usize, tools::PartialToolCall> = HashMap::new();
    let mut sse_buf = String::new();
    let mut byte_stream = response.bytes_stream();

    'sse: loop {
        // Wrap stream read in timeout to detect stalled connections
        let item = match tokio::time::timeout(
            tokio::time::Duration::from_secs(chunk_timeout_secs),
            byte_stream.next(),
        )
        .await
        {
            Ok(Some(result)) => result,
            Ok(None) => break 'sse, // Stream ended normally
            Err(_) => {
                warn!("Stream timeout after {chunk_timeout_secs}s with no data");
                let _ =
                    tx.send(StreamEvent::Error("Stream stalled — no data received".to_string()));
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
                                if let Some(actual) = val.get("model").and_then(|v| v.as_str()) {
                                    info!(
                                        "[stream] API routed request to model={actual:?}  \
                                         (requested={model_id:?})"
                                    );
                                    // Only flag a real switch — not a dated alias of the same
                                    // model (e.g. "gpt-4.1" → "gpt-4.1-2025-04-14").
                                    let is_alias = actual.starts_with(model_id)
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
                                    let idx =
                                        tc_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0)
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
                                    .unwrap_or(0) as u32;
                                let c = usage
                                    .get("completion_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
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
                return StreamOutcome::Error;
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
                            let idx =
                                tc_val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
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

    StreamOutcome::Done(ParsedRound { text_buf, partial_tools })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{parse_sse_line, SseLine};

    #[test]
    fn sse_done() {
        assert_eq!(parse_sse_line("data: [DONE]"), SseLine::Done);
    }

    #[test]
    fn sse_token() {
        let line = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
        assert_eq!(parse_sse_line(line), SseLine::Token("hi".to_string()));
    }

    #[test]
    fn sse_keepalive() {
        assert_eq!(parse_sse_line(": keepalive"), SseLine::Skip);
    }

    #[test]
    fn sse_empty() {
        assert_eq!(parse_sse_line(""), SseLine::Skip);
    }
}
