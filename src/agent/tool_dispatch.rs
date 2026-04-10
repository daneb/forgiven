use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

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
    tx: &mpsc::UnboundedSender<StreamEvent>,
    snapshotted: &mut HashSet<String>,
    mcp_manager: Option<Arc<McpManager>>,
    auto_compress: bool,
    abort_rx: &mut oneshot::Receiver<()>,
    question_rx: &mut mpsc::UnboundedReceiver<String>,
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

        let _ = tx.send(StreamEvent::ToolStart {
            name: call.name.clone(),
            args_summary: call.args_summary(),
        });

        // ── Pre-snapshot: capture original content before first mutating edit ──
        if matches!(call.name.as_str(), "write_file" | "edit_file") {
            if let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) {
                if let Some(path_str) = args.get("path").and_then(|v| v.as_str()) {
                    if snapshotted.insert(path_str.to_string()) {
                        let abs = project_root.join(path_str);
                        if abs.exists() {
                            // Existing file — snapshot its current content.
                            let original = std::fs::read_to_string(&abs).unwrap_or_default();
                            let _ = tx.send(StreamEvent::FileSnapshot {
                                path: path_str.to_string(),
                                original,
                            });
                        } else {
                            // New file — record it so revert_session() can delete it.
                            let _ =
                                tx.send(StreamEvent::FileCreated { path: path_str.to_string() });
                        }
                    }
                }
            }
        }

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
                .map(|arr| arr.iter().filter_map(|o| o.as_str().map(|s| s.to_string())).collect())
                .filter(|v: &Vec<String>| !v.is_empty())
                .unwrap_or_else(|| vec!["Yes".to_string(), "No".to_string()]);

            let _ = tx.send(StreamEvent::AskingUser {
                question: question.clone(),
                options: options.clone(),
            });

            let answer = tokio::select! {
                // Ctrl+C while the dialog is open — abort the whole loop.
                _ = &mut *abort_rx => {
                    let _ = tx.send(StreamEvent::Done);
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
        } else if let Some(ref mcp) = mcp_manager {
            if mcp.is_mcp_tool(&call.name) {
                mcp.call_tool(&call.name, &call.arguments).await
            } else {
                tools::execute_tool(&call, project_root)
            }
        } else {
            tools::execute_tool(&call, project_root)
        };

        // If a file was successfully written or edited, notify the editor
        // so it can reload any open buffer for that path.
        if matches!(call.name.as_str(), "write_file" | "edit_file") && !result.starts_with("error")
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

    DispatchOutcome::Continue
}
