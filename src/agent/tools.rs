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
                "name": "read_files",
                "description": "Read multiple project files in a single call. Prefer this over repeated read_file calls when you need several files at once.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "File paths relative to the project root."
                        }
                    },
                    "required": ["paths"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_files",
                "description": "Search for a pattern across one or more files or directories. Returns matching lines with file path and line number. Prefer this over multiple read_file + manual scan calls.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Text to search for (literal match, case-sensitive)."
                        },
                        "paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "File or directory paths relative to the project root to search within."
                        }
                    },
                    "required": ["pattern", "paths"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_file_outline",
                "description": "Return a compact outline of a file: only top-level definitions (functions, structs, classes, enums, impls, interfaces) with their signatures — no bodies. Use this instead of read_file when you need to understand a file's structure or find where a symbol is defined, then use get_symbol_context or read_file to get the full definition.",
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
                "name": "get_symbol_context",
                "description": "Return the full definition of a named symbol (function, struct, class, etc.) from a file, plus the signatures of any other symbols it directly calls within the same file. Use this to get focused context on one symbol without loading the entire file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the project root."
                        },
                        "symbol": {
                            "type": "string",
                            "description": "Name of the function, struct, class, or other definition to retrieve."
                        }
                    },
                    "required": ["path", "symbol"]
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
        },
        {
            "type": "function",
            "function": {
                "name": "ask_user",
                "description": "Pause and ask the user a question before proceeding. Use when you need clarification about intent, want approval for a destructive action, or need the user to choose between meaningful alternatives. The user sees a dialog and selects an option.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question to display to the user."
                        },
                        "options": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "The choices presented to the user. Defaults to [\"Yes\", \"No\"] if omitted."
                        }
                    },
                    "required": ["question"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "ask_user_input",
                "description": "Pause and ask the user for free-text input before proceeding. Use when you need the user to type something (e.g. a name, a slug, a path, a description). The user sees a text input field and types their answer.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question to display to the user."
                        },
                        "placeholder": {
                            "type": "string",
                            "description": "Optional hint text shown inside the empty input field (e.g. \"my-feature\")."
                        }
                    },
                    "required": ["question"]
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
            // For ask_user, show the question text.
            if self.name == "ask_user" {
                if let Some(q) = val.get("question").and_then(|v| v.as_str()) {
                    return if q.len() > 60 { format!("{}…", &q[..60]) } else { q.to_string() };
                }
            }
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
pub async fn execute_tool(call: &ToolCall, root: &Path) -> String {
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
            match tokio::fs::read_to_string(&path).await {
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

        // ── read_files ───────────────────────────────────────────────────────
        "read_files" => {
            let paths = match args.get("paths").and_then(|v| v.as_array()) {
                Some(p) => p.clone(),
                None => return "error: missing required argument 'paths'".to_string(),
            };
            let mut results = Vec::new();
            for entry in &paths {
                let path_str = match entry.as_str() {
                    Some(s) => s,
                    None => continue,
                };
                let path = match safe_path(root, path_str) {
                    Ok(p) => p,
                    Err(e) => {
                        results.push(format!("=== {path_str} ===\nerror: {e}"));
                        continue;
                    },
                };
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => {
                        let lines: Vec<String> = content
                            .lines()
                            .enumerate()
                            .map(|(i, l)| format!("{:4} | {l}", i + 1))
                            .collect();
                        results.push(format!(
                            "=== {path_str} ({} lines) ===\n{}",
                            lines.len(),
                            lines.join("\n")
                        ));
                    },
                    Err(e) => results.push(format!("=== {path_str} ===\nerror reading: {e}")),
                }
            }
            results.join("\n\n")
        },

        // ── search_files ─────────────────────────────────────────────────────
        "search_files" => {
            let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return "error: missing required argument 'pattern'".to_string(),
            };
            let paths = match args.get("paths").and_then(|v| v.as_array()) {
                Some(p) => p.clone(),
                None => return "error: missing required argument 'paths'".to_string(),
            };

            let mut matches: Vec<String> = Vec::new();
            const MAX_MATCHES: usize = 200;

            for entry in &paths {
                let path_str = match entry.as_str() {
                    Some(s) => s,
                    None => continue,
                };
                let path = match safe_path(root, path_str) {
                    Ok(p) => p,
                    Err(e) => {
                        matches.push(format!("error: {e}"));
                        continue;
                    },
                };
                search_path_recursive(path_str, &path, pattern, &mut matches, MAX_MATCHES);
            }

            if matches.is_empty() {
                format!("no matches for {pattern:?}")
            } else {
                let truncated = matches.len() >= MAX_MATCHES;
                let mut out = matches.join("\n");
                if truncated {
                    out.push_str(&format!("\n... (truncated at {MAX_MATCHES} matches)"));
                }
                out
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
            let old_content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            match tokio::fs::write(&path, content).await {
                Ok(()) => unified_diff(path_str, &old_content, content, 120),
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
            let content = match tokio::fs::read_to_string(&path).await {
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
            match tokio::fs::write(&path, &new_content).await {
                Ok(()) => unified_diff(path_str, &content, &new_content, 120),
                Err(e) => format!("error writing {path_str}: {e}"),
            }
        },

        // ── get_file_outline ─────────────────────────────────────────────────
        "get_file_outline" => {
            let path_str = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return "error: missing required argument 'path'".to_string(),
            };
            let path = match safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return format!("error: {e}"),
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    let symbols = extract_symbols(&content);
                    if symbols.is_empty() {
                        format!("{path_str}: no top-level definitions found")
                    } else {
                        let lines: Vec<String> = symbols
                            .iter()
                            .map(|s| format!("{:4} | {}", s.line + 1, s.signature))
                            .collect();
                        format!("{path_str} — {} definitions:\n{}", symbols.len(), lines.join("\n"))
                    }
                },
                Err(e) => format!("error reading {path_str}: {e}"),
            }
        },

        // ── get_symbol_context ───────────────────────────────────────────────
        "get_symbol_context" => {
            let path_str = match args.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return "error: missing required argument 'path'".to_string(),
            };
            let symbol = match args.get("symbol").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return "error: missing required argument 'symbol'".to_string(),
            };
            let path = match safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return format!("error: {e}"),
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(content) => symbol_context(path_str, &content, symbol),
                Err(e) => format!("error reading {path_str}: {e}"),
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

// ─────────────────────────────────────────────────────────────────────────────
// Symbol extraction (heuristic, no tree-sitter dependency)
// ─────────────────────────────────────────────────────────────────────────────

/// A single top-level definition detected by heuristic line scanning.
pub(crate) struct SymbolDef {
    /// 0-indexed line where the definition starts.
    pub(crate) line: usize,
    /// Signature text (the definition line, trimmed).
    pub(crate) signature: String,
    /// Name of the symbol extracted from the signature.
    pub(crate) name: String,
    /// 0-indexed line where the body ends (best-effort brace/indent matching).
    pub(crate) end_line: usize,
}

/// Heuristic patterns that identify the start of a top-level definition across
/// common languages (Rust, Python, TypeScript/JavaScript, Go, Java, C/C++).
fn is_definition_line(line: &str) -> Option<String> {
    let t = line.trim();
    // Rust: fn / struct / enum / impl / trait / type alias / mod
    if t.starts_with("pub fn ")
        || t.starts_with("pub async fn ")
        || t.starts_with("async fn ")
        || t.starts_with("fn ")
        || t.starts_with("pub struct ")
        || t.starts_with("struct ")
        || t.starts_with("pub enum ")
        || t.starts_with("enum ")
        || t.starts_with("pub trait ")
        || t.starts_with("trait ")
        || t.starts_with("impl ")
        || t.starts_with("pub impl ")
        || t.starts_with("pub type ")
        || t.starts_with("type ")
        || t.starts_with("pub mod ")
        || t.starts_with("mod ")
    {
        return Some(t.to_string());
    }
    // Python: def / class / async def
    if t.starts_with("def ") || t.starts_with("async def ") || t.starts_with("class ") {
        return Some(t.trim_end_matches(':').to_string());
    }
    // TypeScript/JavaScript: function / class / export function / export class /
    // const foo = (…) => / export const foo = (…) =>
    if t.starts_with("function ")
        || t.starts_with("async function ")
        || t.starts_with("export function ")
        || t.starts_with("export async function ")
        || t.starts_with("export default function")
        || t.starts_with("export class ")
        || t.starts_with("class ")
        || (t.starts_with("export const ") && (t.contains("= (") || t.contains("= async (")))
        || (t.starts_with("const ") && (t.contains("= (") || t.contains("= async (")))
        || t.starts_with("export interface ")
        || t.starts_with("interface ")
        || t.starts_with("export type ")
    {
        return Some(t.to_string());
    }
    // Go: func
    if t.starts_with("func ") {
        return Some(t.to_string());
    }
    // Java/C#: public/private/protected + type + name + (
    if (t.starts_with("public ") || t.starts_with("private ") || t.starts_with("protected "))
        && t.contains('(')
    {
        return Some(t.to_string());
    }
    None
}

/// Extract the symbol name from a definition signature.
fn name_from_signature(sig: &str) -> &str {
    // Skip keywords to reach the name token: `pub async fn foo` → `foo`
    let skip = [
        "pub",
        "async",
        "fn",
        "struct",
        "enum",
        "trait",
        "impl",
        "type",
        "mod",
        "def",
        "class",
        "func",
        "function",
        "interface",
        "export",
        "default",
        "const",
        "public",
        "private",
        "protected",
        "static",
        "abstract",
        "override",
    ];
    for token in sig.split_whitespace() {
        if skip.contains(&token) {
            continue;
        }
        // Trim generic params, parens, angle brackets, colons.
        let clean = token.trim_end_matches(['(', '<', ':', '{', ',', ';']);
        if !clean.is_empty()
            && clean.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
        {
            return clean;
        }
    }
    sig
}

/// Extract all top-level symbol definitions from `source`.
pub(crate) fn extract_symbols(source: &str) -> Vec<SymbolDef> {
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols: Vec<SymbolDef> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        // Only consider lines with no leading whitespace (top-level) or
        // single-level indentation (methods inside impl blocks).
        let indent = line.len() - line.trim_start().len();
        if indent > 4 {
            continue;
        }
        if let Some(sig) = is_definition_line(line) {
            let name = name_from_signature(&sig).to_string();
            // Find end of this definition using brace/indent heuristics.
            let end_line = find_end_line(&lines, i);
            symbols.push(SymbolDef { line: i, signature: sig, name, end_line });
        }
    }
    symbols
}

/// Heuristic: find the end line of a definition starting at `start`.
/// For brace-delimited languages: count `{`/`}` balance.
/// Fallback: next definition at the same or lower indent level.
fn find_end_line(lines: &[&str], start: usize) -> usize {
    let start_indent = lines[start].len() - lines[start].trim_start().len();
    let mut brace_depth: i32 = 0;
    let mut found_open = false;

    for (offset, line) in lines[start..].iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '{' => {
                    brace_depth += 1;
                    found_open = true;
                },
                '}' => {
                    brace_depth -= 1;
                },
                _ => {},
            }
        }
        if found_open && brace_depth <= 0 {
            return start + offset;
        }
        // Python / indent-based: stop when we return to start indent after body
        if offset > 0 && !line.trim().is_empty() {
            let indent = line.len() - line.trim_start().len();
            if !found_open && indent <= start_indent {
                return start + offset - 1;
            }
        }
    }
    lines.len().saturating_sub(1)
}

/// Build the `get_symbol_context` response: full body of `symbol` +
/// signatures of other symbols it calls within the same file.
fn symbol_context(path_str: &str, source: &str, symbol: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let symbols = extract_symbols(source);

    // Find the requested symbol (case-sensitive exact match first, then prefix).
    let target = symbols
        .iter()
        .find(|s| s.name == symbol)
        .or_else(|| symbols.iter().find(|s| s.name.starts_with(symbol)));

    let target = match target {
        Some(t) => t,
        None => {
            return format!(
                "{path_str}: symbol {symbol:?} not found.\nAvailable: {}",
                symbols.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
            )
        },
    };

    const MAX_BODY_LINES: usize = 150;
    let body_end = target.end_line.min(target.line + MAX_BODY_LINES - 1).min(lines.len() - 1);
    let body: Vec<String> = lines[target.line..=body_end]
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{:4} | {l}", target.line + i + 1))
        .collect();
    let truncated = body_end < target.end_line;

    // Find sibling symbols called within the body.
    let body_text = lines[target.line..=body_end].join("\n");
    let sibling_sigs: Vec<String> = symbols
        .iter()
        .filter(|s| s.name != target.name && body_text.contains(&s.name))
        .map(|s| format!("{:4} | {}", s.line + 1, s.signature))
        .collect();

    let mut out = format!(
        "{path_str} — `{}` ({} lines):\n{}",
        target.name,
        body_end - target.line + 1,
        body.join("\n")
    );
    if truncated {
        out.push_str(&format!("\n... (truncated at {MAX_BODY_LINES} lines)"));
    }
    if !sibling_sigs.is_empty() {
        out.push_str(&format!("\n\nReferenced sibling definitions:\n{}", sibling_sigs.join("\n")));
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Search helper
// ─────────────────────────────────────────────────────────────────────────────

/// Recursively search `path` for `pattern`, appending `"file:line: text"` entries
/// to `out` until `max_matches` is reached.
fn search_path_recursive(
    display: &str,
    path: &std::path::Path,
    pattern: &str,
    out: &mut Vec<String>,
    max_matches: usize,
) {
    if out.len() >= max_matches {
        return;
    }
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            let mut children: Vec<_> = entries.flatten().collect();
            children.sort_by_key(|e| e.file_name());
            for entry in children {
                if out.len() >= max_matches {
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue; // skip hidden files/dirs
                }
                let child_display = format!("{display}/{name}");
                search_path_recursive(&child_display, &entry.path(), pattern, out, max_matches);
            }
        }
    } else if let Ok(content) = std::fs::read_to_string(path) {
        for (i, line) in content.lines().enumerate() {
            if out.len() >= max_matches {
                break;
            }
            if line.contains(pattern) {
                out.push(format!("{display}:{}: {line}", i + 1));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Diff helper
// ─────────────────────────────────────────────────────────────────────────────

/// Produce a compact unified diff between `old` and `new` text (3 lines of
/// context).  Returns at most `max_lines` output lines to keep tool results
/// within a sensible token budget.
pub fn unified_diff(path: &str, old: &str, new: &str, max_lines: usize) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Collect changed hunks: (old_start, old_lines, new_lines)
    let context = 3usize;
    let mut hunks: Vec<(usize, Vec<&str>, Vec<&str>)> = Vec::new();

    let mut i = 0usize;
    let mut j = 0usize;
    while i < old_lines.len() || j < new_lines.len() {
        if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
            i += 1;
            j += 1;
        } else {
            // Find extent of this changed region
            let old_start = i;
            let new_start = j;
            // Advance until we re-sync (simple greedy: match next identical line)
            while i < old_lines.len() || j < new_lines.len() {
                if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
                    break;
                }
                if i < old_lines.len() {
                    i += 1;
                }
                if j < new_lines.len() {
                    j += 1;
                }
            }
            hunks.push((
                old_start,
                old_lines[old_start..i].to_vec(),
                new_lines[new_start..j].to_vec(),
            ));
        }
    }

    if hunks.is_empty() {
        return format!("{path}: no changes");
    }

    let mut out = Vec::new();
    out.push(format!("--- a/{path}"));
    out.push(format!("+++ b/{path}"));

    for (old_start, removed, added) in &hunks {
        let ctx_before_start = old_start.saturating_sub(context);
        let ctx_before: Vec<&str> = old_lines[ctx_before_start..*old_start].to_vec();
        let ctx_after_start = (*old_start + removed.len()).min(old_lines.len());
        let ctx_after_end = (ctx_after_start + context).min(old_lines.len());
        let ctx_after: Vec<&str> = old_lines[ctx_after_start..ctx_after_end].to_vec();

        let old_len = ctx_before.len() + removed.len() + ctx_after.len();
        let new_len = ctx_before.len() + added.len() + ctx_after.len();
        out.push(format!(
            "@@ -{},{} +{},{} @@",
            ctx_before_start + 1,
            old_len,
            ctx_before_start + 1,
            new_len
        ));
        for l in &ctx_before {
            out.push(format!(" {l}"));
        }
        for l in removed {
            out.push(format!("-{l}"));
        }
        for l in added {
            out.push(format!("+{l}"));
        }
        for l in &ctx_after {
            out.push(format!(" {l}"));
        }
    }

    // Cap output to avoid re-introducing token bloat for massive diffs
    if out.len() > max_lines {
        out.truncate(max_lines);
        out.push(format!("... ({} lines truncated)", out.len() - max_lines));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{execute_tool, safe_path, ToolCall};

    fn make_call(name: &str, args: &str) -> ToolCall {
        ToolCall { id: "test".into(), name: name.into(), arguments: args.into() }
    }

    #[test]
    fn safe_path_traversal_rejected() {
        let root = std::env::temp_dir();
        assert!(safe_path(&root, "../etc/passwd").is_err());
        assert!(safe_path(&root, "foo/../../etc").is_err());
    }

    #[test]
    fn safe_path_valid() {
        let root = std::env::temp_dir();
        let result = safe_path(&root, "src/main.rs");
        assert!(result.is_ok());
        assert!(result.unwrap().starts_with(&root));
    }

    #[tokio::test]
    async fn execute_tool_unknown() {
        let root = std::env::temp_dir();
        let call = make_call("bogus_tool", "{}");
        let result = execute_tool(&call, &root).await;
        assert!(result.contains("unknown tool") || result.contains("bogus"));
    }

    #[tokio::test]
    async fn execute_tool_read_missing() {
        let root = std::env::temp_dir();
        let args = r#"{"path":"__nonexistent_file_xyz__.txt"}"#;
        let call = make_call("read_file", args);
        let result = execute_tool(&call, &root).await;
        assert!(!result.is_empty());
    }

    #[tokio::test]
    async fn execute_tool_write_then_read() {
        let root = std::env::temp_dir();
        let filename = format!("forgiven_test_{}.txt", std::process::id());
        let content = "hello from test";
        let write_args = serde_json::json!({"path": &filename, "content": content}).to_string();
        execute_tool(&make_call("write_file", &write_args), &root).await;
        let read_args = serde_json::json!({"path": &filename}).to_string();
        let result = execute_tool(&make_call("read_file", &read_args), &root).await;
        assert!(result.contains(content));
        let _ = tokio::fs::remove_file(root.join(&filename)).await;
    }
}
