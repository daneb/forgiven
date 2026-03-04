//! Tool definitions and execution for the agentic loop.
//!
//! Four tools are exposed to the model:
//!   read_file       – read a project file (returns line-numbered content)
//!   write_file      – write / create a file with full content
//!   edit_file       – surgical find-and-replace inside an existing file (preferred)
//!   list_directory  – list a directory's entries
//!
//! All paths are validated against the project root: `..` traversal is rejected.

use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Tool JSON schema (sent to the model as the `tools` field)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the tools array to include verbatim in every chat API request.
pub fn tool_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read the full contents of a file in the project. Returns line-numbered output.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the project root."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write complete content to a file, creating it or overwriting it. Prefer edit_file for targeted changes to existing files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the project root."
                        },
                        "content": {
                            "type": "string",
                            "description": "The complete file content to write."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Make a surgical edit to a file by replacing an exact, unique string. Preferred over write_file for modifying existing files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the project root."
                        },
                        "old_str": {
                            "type": "string",
                            "description": "The exact string to replace. Must appear exactly once in the file — include enough surrounding context to make it unique."
                        },
                        "new_str": {
                            "type": "string",
                            "description": "The string to replace old_str with."
                        }
                    },
                    "required": ["path", "old_str", "new_str"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_directory",
                "description": "List files and subdirectories at a path inside the project.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path relative to the project root. Use '.' for the project root."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "complete_task",
                "description": "Mark a previously created task as done. Use the exact same title passed to create_task.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "The exact title passed to create_task for this step."
                        }
                    },
                    "required": ["title"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "create_task",
                "description": "Add a task to the user's task panel. Call this once per planned step at the start of any multi-step job so the user can track progress.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Short, imperative description of the task (e.g. 'Create Program.cs with HttpClient logic')."
                        }
                    },
                    "required": ["title"]
                }
            }
        }
    ])
}

// ─────────────────────────────────────────────────────────────────────────────
// Data types
// ─────────────────────────────────────────────────────────────────────────────

/// A fully-assembled tool call (all streaming delta chunks combined).
#[derive(Debug, Clone)]
pub struct ToolCall {
    #[allow(dead_code)] // tool-call ID is part of the Copilot API protocol
    pub id: String,
    pub name: String,
    /// Raw JSON string of the arguments object.
    pub arguments: String,
}

impl ToolCall {
    /// Short one-line display string used in the chat UI.
    pub fn args_summary(&self) -> String {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&self.arguments) {
            // Show the 'path' argument if present — it's the most meaningful for display.
            if let Some(path) = val.get("path").and_then(|v| v.as_str()) {
                return path.to_string();
            }
            // For edit_file, show path + a hint about old_str length.
            if let (Some(p), Some(o)) = (
                val.get("path").and_then(|v| v.as_str()),
                val.get("old_str").and_then(|v| v.as_str()),
            ) {
                return format!("{} ({} chars)", p, o.len());
            }
        }
        // Fallback: trim braces and truncate.
        let s = self.arguments.trim_matches(|c: char| c == '{' || c == '}');
        if s.len() > 60 {
            format!("{}…", &s[..60])
        } else {
            s.to_string()
        }
    }
}

/// Partial tool call being assembled from streaming chunks.
#[derive(Debug, Default)]
pub struct PartialToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Safety
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a project-relative `relative` path against `root`.
/// Returns `Err` if the path contains `..` (traversal attempt).
pub fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.contains("..") {
        return Err(format!("path traversal not allowed: {relative}"));
    }
    Ok(root.join(relative))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tool execution
// ─────────────────────────────────────────────────────────────────────────────

/// Execute `call` against `root` and return a result string for the model.
pub fn execute_tool(call: &ToolCall, root: &Path) -> String {
    let args: serde_json::Value = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return format!("error parsing tool arguments: {e}"),
    };

    match call.name.as_str() {
        // ── read_file ────────────────────────────────────────────────────────
        "read_file" => {
            let path_str = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return "error: missing required argument 'path'".to_string(),
            };
            let path = match safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return format!("error: {e}"),
            };
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let lines: Vec<String> = content
                        .lines()
                        .enumerate()
                        .map(|(i, l)| format!("{:4} | {l}", i + 1))
                        .collect();
                    format!("{path_str} ({} lines)\n{}", lines.len(), lines.join("\n"))
                },
                Err(e) => format!("error reading {path_str}: {e}"),
            }
        },

        // ── write_file ───────────────────────────────────────────────────────
        "write_file" => {
            let path_str = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return "error: missing required argument 'path'".to_string(),
            };
            let content = match args.get("content").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => return "error: missing required argument 'content'".to_string(),
            };
            let path = match safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return format!("error: {e}"),
            };
            if let Some(parent) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return format!("error creating parent directories for {path_str}: {e}");
                }
            }
            match std::fs::write(&path, content) {
                Ok(()) => format!("wrote {path_str} ({} bytes)", content.len()),
                Err(e) => format!("error writing {path_str}: {e}"),
            }
        },

        // ── edit_file ────────────────────────────────────────────────────────
        "edit_file" => {
            let path_str = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return "error: missing required argument 'path'".to_string(),
            };
            let old_str = match args.get("old_str").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return "error: missing required argument 'old_str'".to_string(),
            };
            let new_str = match args.get("new_str").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return "error: missing required argument 'new_str'".to_string(),
            };
            let path = match safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return format!("error: {e}"),
            };
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => return format!("error reading {path_str}: {e}"),
            };
            let count = content.matches(old_str).count();
            if count == 0 {
                return format!(
                    "error: old_str not found in {path_str}. \
                     You MUST call read_file(\"{path_str}\") first and copy old_str verbatim \
                     from that output — do NOT guess or paraphrase. \
                     Check that whitespace and indentation match exactly."
                );
            }
            if count > 1 {
                return format!(
                    "error: old_str appears {count} times in {path_str} — \
                     include more surrounding lines in old_str to make it unique."
                );
            }
            let new_content = content.replacen(old_str, new_str, 1);
            match std::fs::write(&path, &new_content) {
                Ok(()) => format!(
                    "edited {path_str} (replaced {} chars with {} chars)",
                    old_str.len(),
                    new_str.len()
                ),
                Err(e) => format!("error writing {path_str}: {e}"),
            }
        },

        // ── list_directory ───────────────────────────────────────────────────
        "list_directory" => {
            let path_str = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return "error: missing required argument 'path'".to_string(),
            };
            let path = match safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return format!("error: {e}"),
            };
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut items: Vec<String> = entries
                        .flatten()
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            if name.starts_with('.') {
                                return None; // skip hidden
                            }
                            let is_dir = e.path().is_dir();
                            Some(if is_dir { format!("{name}/") } else { name })
                        })
                        .collect();
                    items.sort();
                    if items.is_empty() {
                        format!("{path_str}: (empty)")
                    } else {
                        format!("{path_str}:\n{}", items.join("\n"))
                    }
                },
                Err(e) => format!("error listing {path_str}: {e}"),
            }
        },

        // ── create_task / complete_task ───────────────────────────────────────
        // UI updates are handled by the agentic loop via StreamEvent.
        // These just validate the argument and return an acknowledgement.
        "create_task" => match args.get("title").and_then(|v| v.as_str()) {
            Some(title) => format!("task created: {title}"),
            None => "error: missing required argument 'title'".to_string(),
        },
        "complete_task" => match args.get("title").and_then(|v| v.as_str()) {
            Some(title) => format!("task done: {title}"),
            None => "error: missing required argument 'title'".to_string(),
        },

        other => format!("unknown tool: {other}"),
    }
}
