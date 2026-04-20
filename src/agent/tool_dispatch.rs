use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tracing::info;

use crate::mcp::McpManager;

use super::streaming::maybe_compress;
use super::tools;
use super::StreamEvent;

// ─────────────────────────────────────────────────────────────────────────────
// Tool dispatch outcome
// ─────────────────────────────────────────────────────────────────────────────

/// Return value from `dispatch_tools` signalling whether the agentic loop
/// should continue to the next round or abort (Ctrl+C fired during ask_user).
pub(super) enum DispatchOutcome {
    Continue,
    /// User pressed Ctrl+C while an ask_user dialog was open.
    /// `StreamEvent::Done` has already been sent via `tx`.
    Abort,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tool execution loop
// ─────────────────────────────────────────────────────────────────────────────

/// Execute all tool calls from one agentic round, appending results to
/// `messages` and emitting `StreamEvent` variants via `tx`.
///
/// Returns `DispatchOutcome::Abort` if the user presses Ctrl+C during an
/// `ask_user` dialog — the caller must `return` from the agentic loop.
#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_tools(
    sorted: Vec<(usize, tools::PartialToolCall)>,
    messages: &mut Vec<serde_json::Value>,
    text_buf: String,
    project_root: &Path,
    tx: &mpsc::Sender<StreamEvent>,
    snapshotted: &mut HashSet<String>,
    mcp_manager: Option<Arc<McpManager>>,
    auto_compress: bool,
    abort_rx: &mut oneshot::Receiver<()>,
    question_rx: &mut mpsc::UnboundedReceiver<String>,
    result_cache: &mut HashMap<String, String>,
    expand_threshold: usize,
    large_reads: &mut usize,
    knowledge_docs: &[(String, std::path::PathBuf)],
    knowledge_fetch_max_bytes: usize,
) -> DispatchOutcome {
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

        let _ = tx
            .send(StreamEvent::ToolStart {
                name: call.name.clone(),
                args_summary: call.args_summary(),
            })
            .await;

        // ── Pre-snapshot: capture original content before first mutating edit ──
        if matches!(call.name.as_str(), "write_file" | "edit_file") {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) {
                if let Some(path_str) = args.get("path").and_then(|v| v.as_str()) {
                    if snapshotted.insert(path_str.to_string()) {
                        let abs = project_root.join(path_str);
                        if abs.exists() {
                            // Existing file — snapshot its current content.
                            let original =
                                tokio::fs::read_to_string(&abs).await.unwrap_or_default();
                            let _ = tx
                                .send(StreamEvent::FileSnapshot {
                                    path: path_str.to_string(),
                                    original,
                                })
                                .await;
                        } else {
                            // New file — record it so revert_session() can delete it.
                            let _ = tx
                                .send(StreamEvent::FileCreated { path: path_str.to_string() })
                                .await;
                        }
                    }
                }
            }
        }

        let result = if call.name == "fetch_knowledge" {
            let args_val =
                serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default();
            let name = args_val.get("name").and_then(|v| v.as_str()).unwrap_or("");
            match knowledge_docs.iter().find(|(n, _)| n == name) {
                Some((_, path)) => match tokio::fs::read_to_string(path).await {
                    Ok(content) => {
                        if content.len() > knowledge_fetch_max_bytes {
                            let truncated: String =
                                content.chars().take(knowledge_fetch_max_bytes).collect();
                            format!("{truncated}\n[truncated at {knowledge_fetch_max_bytes} bytes]")
                        } else {
                            content
                        }
                    },
                    Err(e) => format!("error: could not read knowledge doc {name:?}: {e}"),
                },
                None => {
                    let available: Vec<_> =
                        knowledge_docs.iter().map(|(n, _)| n.as_str()).collect();
                    if available.is_empty() {
                        "error: no knowledge documents found in .forgiven/knowledge/".to_string()
                    } else {
                        format!(
                            "error: knowledge document {name:?} not found. Available: {}",
                            available.join(", ")
                        )
                    }
                },
            }
        } else if call.name == "expand_result" {
            let args_val =
                serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default();
            let id = args_val.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match result_cache.get(id) {
                Some(full) => {
                    if let Some(range) = args_val.get("range") {
                        let start =
                            range.get("start").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let end = range.get("end").and_then(|v| v.as_u64()).map(|v| v as usize);
                        let chars: Vec<char> = full.chars().collect();
                        let end = end.unwrap_or(chars.len()).min(chars.len());
                        chars[start.min(chars.len())..end].iter().collect()
                    } else {
                        full.clone()
                    }
                },
                None => format!(
                    "error: no cached result for id={id:?}. \
                     The result may have expired or the id is incorrect."
                ),
            }
        } else if call.name == "ask_user" {
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
                .map(|arr| arr.iter().filter_map(|o| o.as_str().map(|s| s.to_string())).collect())
                .filter(|v: &Vec<String>| !v.is_empty())
                .unwrap_or_else(|| vec!["Yes".to_string(), "No".to_string()]);

            let _ = tx
                .send(StreamEvent::AskingUser {
                    question: question.clone(),
                    options: options.clone(),
                })
                .await;

            let answer = tokio::select! {
                // Ctrl+C while the dialog is open — abort the whole loop.
                _ = &mut *abort_rx => {
                    let _ = tx.send(StreamEvent::Done).await;
                    return DispatchOutcome::Abort;
                }
                res = tokio::time::timeout(
                    tokio::time::Duration::from_secs(300),
                    question_rx.recv(),
                ) => res,
            };

            match answer {
                Ok(Some(ans)) => ans,
                Ok(None) | Err(_) => options.last().cloned().unwrap_or_else(|| "No".to_string()),
            }
        } else if call.name == "ask_user_input" {
            // Parse question + placeholder, emit an AskingUserInput event, and block until
            // the user types and confirms (or the 5-minute timeout fires).
            let args_val =
                serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default();
            let question = args_val
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("Please provide input")
                .to_string();
            let placeholder =
                args_val.get("placeholder").and_then(|v| v.as_str()).unwrap_or("").to_string();

            let _ = tx
                .send(StreamEvent::AskingUserInput {
                    question: question.clone(),
                    placeholder: placeholder.clone(),
                })
                .await;

            let answer = tokio::select! {
                _ = &mut *abort_rx => {
                    let _ = tx.send(StreamEvent::Done).await;
                    return DispatchOutcome::Abort;
                }
                res = tokio::time::timeout(
                    tokio::time::Duration::from_secs(300),
                    question_rx.recv(),
                ) => res,
            };

            match answer {
                Ok(Some(ans)) => ans,
                Ok(None) | Err(_) => String::new(),
            }
        } else if let Some(ref mcp) = mcp_manager {
            if mcp.is_mcp_tool(&call.name) {
                mcp.call_tool(&call.name, &call.arguments).await
            } else {
                tools::execute_tool(&call, project_root).await
            }
        } else {
            tools::execute_tool(&call, project_root).await
        };

        // If a file was successfully written or edited, notify the editor
        // so it can reload any open buffer for that path.
        if matches!(call.name.as_str(), "write_file" | "edit_file") && !result.starts_with("error")
        {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) {
                if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
                    let _ = tx.send(StreamEvent::FileModified { path: p.to_string() }).await;
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
                    let _ = tx.send(event).await;
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

        // Count large read_file results for the soft budget guard (Intervention 2).
        // Parse line count from the header: "{path} ({N} lines)\n..."
        if call.name == "read_file" && !result.starts_with("error") {
            if let Some(line_count) = result.lines().next().and_then(|hdr| {
                let start = hdr.rfind('(')?;
                let end = hdr.find(" lines)")?;
                hdr[start + 1..end].trim().parse::<usize>().ok()
            }) {
                if line_count > 300 {
                    *large_reads += 1;
                }
            }
        }

        // Expand-on-demand truncation (Intervention 1): store full result in cache
        // and replace history entry with a truncated head + retrieval invitation.
        let result =
            if expand_threshold > 0 && call.name != "expand_result" && !result.starts_with("error")
            {
                let char_len = result.chars().count();
                if char_len > expand_threshold {
                    let truncated: String = result.chars().take(expand_threshold).collect();
                    info!(
                        "[ctx] truncated tool_result {} from {}chars to {}chars",
                        partial.id, char_len, expand_threshold
                    );
                    result_cache.insert(partial.id.clone(), result);
                    format!(
                        "{truncated}\n[truncated; {char_len} chars total. \
                         Call expand_result(id={:?}) to see full.]",
                        partial.id
                    )
                } else {
                    result
                }
            } else {
                result
            };

        // Write a tool_error record to sessions.jsonl for failed tool calls
        // so Phase 3 can break down error types without re-parsing log files.
        let success = !result.starts_with("error");
        if !success {
            let error_snippet: String = result.chars().take(120).collect();
            crate::agent::append_session_metric(&serde_json::json!({
                "type": "tool_error",
                "ts": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                "tool": call.name,
                "error": error_snippet,
            }));
        }

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
        let _ = tx
            .send(StreamEvent::ToolDone { name: call.name.clone(), result_summary, success })
            .await;

        messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": partial.id,
            "content": result
        }));
    }

    DispatchOutcome::Continue
}
