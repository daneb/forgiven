//! Copilot Chat / agent panel — with agentic tool-calling loop.
//!
//! Auth flow:
//!   1. Read the GitHub OAuth token from ~/.config/github-copilot/apps.json
//!   2. Exchange it for a short-lived Copilot API token via the GitHub API
//!   3. Stream chat completions from api.githubcopilot.com (OpenAI-compatible SSE)
//!
//! Tool-calling loop:
//!   The model may respond with `tool_calls` instead of (or before) text.
//!   The agentic_loop task executes those tools, appends results to the message
//!   list, and re-submits until the model produces a plain text reply.
//!
//!   All file operations are sandboxed to the project root (no `..` traversal).

pub mod tools;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Data types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Assistant,
    System,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent panel state
// ─────────────────────────────────────────────────────────────────────────────

pub struct AgentPanel {
    pub visible: bool,
    pub focused: bool,
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub scroll: usize,
    token: Option<CopilotApiToken>,
    pub streaming_reply: Option<String>,
    pub stream_rx: Option<mpsc::UnboundedReceiver<StreamEvent>>,
    /// Paths (project-relative) of files modified by the agent this frame.
    /// The editor drains this each tick to reload open buffers.
    pub pending_reloads: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Token(String),
    ToolStart { name: String, args_summary: String },
    ToolDone { result_summary: String },
    /// A file was successfully written or edited by a tool.
    /// The path is project-relative (as passed to the tool).
    FileModified { path: String },
    Done,
    Error(String),
}

#[derive(Debug, Clone)]
struct CopilotApiToken {
    token: String,
    expires_at: u64,
}

impl CopilotApiToken {
    fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now + 60 >= self.expires_at
    }
}

impl AgentPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            focused: false,
            messages: Vec::new(),
            input: String::new(),
            scroll: 0,
            token: None,
            streaming_reply: None,
            stream_rx: None,
            pending_reloads: Vec::new(),
        }
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        self.focused = self.visible;
    }

    pub fn focus(&mut self) { self.focused = true; }
    pub fn blur(&mut self)  { self.focused = false; }
    pub fn input_char(&mut self, ch: char) { self.input.push(ch); }
    pub fn input_backspace(&mut self) { self.input.pop(); }

    /// Submit input, launching the agentic tool-calling loop in the background.
    pub async fn submit(
        &mut self,
        context: Option<String>,
        project_root: PathBuf,
    ) -> Result<()> {
        if self.input.trim().is_empty() {
            return Ok(());
        }

        let user_text = std::mem::take(&mut self.input);
        let root_display = project_root.display().to_string();

        // Build a shallow file tree so the model knows the project layout upfront
        // and never needs to burn rounds on list_directory exploration.
        let project_tree = build_project_tree(&project_root, 2);

        let tool_rules = "\
MANDATORY PROTOCOL — follow these rules without exception:\n\
\n\
COMMUNICATION RULES:\n\
6. Do NOT output any text while working through tool calls. Work silently.\n\
   Only write a single, concise final response AFTER all tools have completed.\n\
   Do not narrate steps, explain retries, or announce what you are about to do.\n\
\n\
FILE EDITING RULES:\n\
1. ALWAYS call read_file on a file BEFORE calling edit_file or write_file on it.\n\
   Never guess or assume what a file contains — you must read it first.\n\
2. Copy old_str VERBATIM from the read_file output, including all whitespace,\n\
   indentation, and surrounding lines needed to make it unique in the file.\n\
3. If edit_file returns an error, call read_file again to get the current content\n\
   and retry with the correct old_str. Do NOT retry with the same old_str.\n\
4. Prefer edit_file over write_file for any change to an existing file.\n\
5. Use list_directory only if the project tree above is insufficient.\n\
\n\
Available tools:\n\
- read_file       Read a file (returns line-numbered content). REQUIRED before edits.\n\
- write_file      Write a complete file (for new files or full rewrites only).\n\
- edit_file       Surgical find-and-replace. old_str must match EXACTLY once.\n\
- list_directory  List a directory's contents.\n";

        let system = if let Some(ref ctx) = context {
            format!(
                "You are an agentic coding assistant embedded in the 'forgiven' terminal editor.\n\
                 Project root: {root_display}\n\n\
                 Project file tree (depth 2 — use read_file to see contents):\n\
                 ```\n{project_tree}```\n\n\
                 {tool_rules}\n\
                 Currently open file (already read — you may use this content directly for edits):\n\
                 ```\n{ctx}\n```"
            )
        } else {
            format!(
                "You are an agentic coding assistant embedded in the 'forgiven' terminal editor.\n\
                 Project root: {root_display}\n\n\
                 Project file tree (depth 2 — use read_file to see contents):\n\
                 ```\n{project_tree}```\n\n\
                 {tool_rules}"
            )
        };

        let mut send_messages: Vec<serde_json::Value> = vec![
            serde_json::json!({ "role": "system", "content": system }),
        ];

        let history_start = self.messages.len().saturating_sub(20);
        for msg in &self.messages[history_start..] {
            send_messages.push(serde_json::json!({
                "role": msg.role.as_str(),
                "content": msg.content
            }));
        }
        send_messages.push(serde_json::json!({ "role": "user", "content": user_text.clone() }));
        self.messages.push(ChatMessage { role: Role::User, content: user_text });

        self.scroll = 0;
        self.streaming_reply = Some(String::new());

        let api_token = self.ensure_token().await?;
        let (tx, rx) = mpsc::unbounded_channel::<StreamEvent>();
        self.stream_rx = Some(rx);

        tokio::spawn(agentic_loop(api_token, send_messages, project_root, tx));
        Ok(())
    }

    pub fn poll_stream(&mut self) -> bool {
        let mut active = false;
        if let Some(rx) = self.stream_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(StreamEvent::Token(t)) => {
                        active = true;
                        if let Some(r) = self.streaming_reply.as_mut() { r.push_str(&t); }
                    }
                    Ok(StreamEvent::ToolStart { name, args_summary }) => {
                        active = true;
                        let line = format!("\n⚙  {name}({args_summary})");
                        match self.streaming_reply.as_mut() {
                            Some(r) => r.push_str(&line),
                            None    => self.streaming_reply = Some(line),
                        }
                    }
                    Ok(StreamEvent::ToolDone { result_summary }) => {
                        active = true;
                        if let Some(r) = self.streaming_reply.as_mut() {
                            r.push_str(&format!(" → {result_summary}"));
                        }
                    }
                    Ok(StreamEvent::FileModified { path }) => {
                        active = true;
                        self.pending_reloads.push(path);
                    }
                    Ok(StreamEvent::Done) => {
                        if let Some(text) = self.streaming_reply.take() {
                            if !text.is_empty() {
                                self.messages.push(ChatMessage {
                                    role: Role::Assistant,
                                    content: text,
                                });
                            }
                        }
                        self.scroll = 0;
                        self.stream_rx = None;
                        break;
                    }
                    Ok(StreamEvent::Error(e)) => {
                        warn!("Copilot Chat stream error: {}", e);
                        self.messages.push(ChatMessage {
                            role: Role::Assistant,
                            content: format!("[Error: {e}]"),
                        });
                        self.streaming_reply = None;
                        self.stream_rx = None;
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        active
    }

    pub fn scroll_up(&mut self)       { self.scroll += 3; }
    pub fn scroll_down(&mut self)     { self.scroll = self.scroll.saturating_sub(3); }
    pub fn scroll_to_bottom(&mut self){ self.scroll = 0; }

    // ── Code extraction ───────────────────────────────────────────────────────

    pub fn extract_code_blocks(text: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut in_block = false;
        let mut current: Vec<&str> = Vec::new();
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                if in_block {
                    while current.last().map(|l: &&str| l.trim().is_empty()).unwrap_or(false) {
                        current.pop();
                    }
                    if !current.is_empty() { blocks.push(current.join("\n")); }
                    current.clear();
                    in_block = false;
                } else {
                    in_block = true;
                }
            } else if in_block {
                current.push(line);
            }
        }
        blocks
    }

    pub fn get_code_to_apply(&self) -> Option<String> {
        self.messages.iter().rev()
            .find(|m| m.role == Role::Assistant)
            .and_then(|m| Self::extract_code_blocks(&m.content).into_iter().next())
    }

    pub fn has_code_to_apply(&self) -> bool { self.get_code_to_apply().is_some() }

    async fn ensure_token(&mut self) -> Result<String> {
        if let Some(ref t) = self.token {
            if !t.is_expired() { return Ok(t.token.clone()); }
        }
        info!("Refreshing Copilot API token");
        let oauth = load_oauth_token()?;
        let api_token = exchange_token(&oauth).await?;
        let tok = api_token.token.clone();
        self.token = Some(api_token);
        Ok(tok)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agentic loop (runs in a background tokio task)
// ─────────────────────────────────────────────────────────────────────────────

async fn agentic_loop(
    api_token: String,
    mut messages: Vec<serde_json::Value>,
    project_root: PathBuf,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let tool_defs = tools::tool_definitions();
    const MAX_ROUNDS: usize = 20;

    for _round in 0..MAX_ROUNDS {
        // ── Call the API ──────────────────────────────────────────────────────
        let response = match start_chat_stream_with_tools(
            api_token.clone(),
            messages.clone(),
            tool_defs.clone(),
        ).await {
            Ok(r)  => r,
            Err(e) => { let _ = tx.send(StreamEvent::Error(format!("{e}"))); return; }
        };

        // ── Parse the SSE stream ──────────────────────────────────────────────
        let mut text_buf = String::new();
        let mut partial_tools: HashMap<usize, tools::PartialToolCall> = HashMap::new();
        let mut sse_buf = String::new();
        let mut byte_stream = response.bytes_stream();

        'sse: while let Some(item) = byte_stream.next().await {
            match item {
                Ok(bytes) => {
                    sse_buf.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(pos) = sse_buf.find('\n') {
                        let line = sse_buf[..pos].trim().to_string();
                        sse_buf.drain(..=pos);

                        if line == "data: [DONE]" { break 'sse; }

                        if let Some(json_str) = line.strip_prefix("data: ") {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                // Text content delta
                                if let Some(content) = val
                                    .pointer("/choices/0/delta/content")
                                    .and_then(|v| v.as_str())
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
                                        let idx = tc_val.get("index")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0) as usize;
                                        let entry = partial_tools.entry(idx).or_default();

                                        if let Some(id) = tc_val.get("id").and_then(|v| v.as_str()) {
                                            if !id.is_empty() { entry.id = id.to_string(); }
                                        }
                                        if let Some(name) = tc_val.pointer("/function/name").and_then(|v| v.as_str()) {
                                            if !name.is_empty() { entry.name = name.to_string(); }
                                        }
                                        if let Some(chunk) = tc_val.pointer("/function/arguments").and_then(|v| v.as_str()) {
                                            entry.arguments.push_str(chunk);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => { let _ = tx.send(StreamEvent::Error(format!("{e}"))); return; }
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

        let tool_calls_json: Vec<serde_json::Value> = sorted.iter().map(|(_, tc)| {
            serde_json::json!({
                "id": tc.id,
                "type": "function",
                "function": { "name": tc.name, "arguments": tc.arguments }
            })
        }).collect();

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

            let result = tools::execute_tool(&call, &project_root);

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

            let result_summary = {
                // Skip header lines like "src/foo.rs (42 lines)" or "path:"
                // and show the first meaningful content line instead.
                let meaningful = result.lines()
                    .find(|l| {
                        let t = l.trim();
                        !t.is_empty() && !t.ends_with(':') && !t.contains('(') || t.starts_with("error")
                    })
                    .unwrap_or_else(|| result.lines().next().unwrap_or("ok"));
                if meaningful.len() > 120 { format!("{}…", &meaningful[..120]) }
                else { meaningful.to_string() }
            };
            let _ = tx.send(StreamEvent::ToolDone { result_summary });

            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": partial.id,
                "content": result
            }));
        }

        // Visual separator between rounds
        let _ = tx.send(StreamEvent::Token("\n".to_string()));
        // Continue loop with tool results appended to messages
    }

    let _ = tx.send(StreamEvent::Error(
        "agentic loop reached maximum rounds without a final text response".to_string(),
    ));
}

// ─────────────────────────────────────────────────────────────────────────────
// Project tree builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a compact, indented file tree of `root` to `max_depth` levels.
/// Hidden files and noisy directories (target, node_modules, …) are skipped.
fn build_project_tree(root: &std::path::Path, max_depth: usize) -> String {
    let mut out = String::new();
    tree_recursive(root, root, 0, max_depth, &mut out);
    out
}

fn tree_recursive(
    root: &std::path::Path,
    path: &std::path::Path,
    depth: usize,
    max_depth: usize,
    out: &mut String,
) {
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|e| {
        // dirs first, then files, both alphabetical
        let is_file = e.path().is_file();
        (is_file, e.file_name().to_string_lossy().to_lowercase())
    });
    for entry in items {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // skip hidden
        }
        if matches!(
            name.as_str(),
            "target" | "node_modules" | "dist" | "build" | ".git"
        ) {
            continue;
        }
        let indent = "  ".repeat(depth);
        if entry.path().is_dir() {
            out.push_str(&format!("{indent}{name}/\n"));
            tree_recursive(root, &entry.path(), depth + 1, max_depth, out);
        } else {
            out.push_str(&format!("{indent}{name}\n"));
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP
// ─────────────────────────────────────────────────────────────────────────────

async fn start_chat_stream_with_tools(
    api_token: String,
    messages: Vec<serde_json::Value>,
    tools: serde_json::Value,
) -> Result<reqwest::Response> {
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "stream": true,
        "n": 1,
        "temperature": 0.1,
        "max_tokens": 4096
    });

    let client = reqwest::Client::new();
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
        .await
        .context("Failed to reach Copilot Chat API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Copilot Chat API error ({status}): {body}"));
    }

    info!("Copilot Chat stream started ({})", resp.status());
    Ok(resp)
}

// ─────────────────────────────────────────────────────────────────────────────
// Auth
// ─────────────────────────────────────────────────────────────────────────────

fn load_oauth_token() -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = format!("{home}/.config/github-copilot/apps.json");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {path}"))?;
    let val: serde_json::Value = serde_json::from_str(&raw).context("apps.json is not valid JSON")?;
    val.as_object()
        .and_then(|m| m.values().next())
        .and_then(|e| e.get("oauth_token"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .context("oauth_token not found in apps.json")
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    token: String,
    expires_at: Option<String>,
}

async fn exchange_token(oauth_token: &str) -> Result<CopilotApiToken> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.github.com/copilot_internal/v2/token")
        .header("Authorization", format!("token {oauth_token}"))
        .header("User-Agent", "forgiven/0.1.0")
        .header("Accept", "application/json")
        .send().await.context("Failed to reach GitHub API")?;

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    debug!("Token exchange response ({status}): {body_text}");
    if !status.is_success() {
        return Err(anyhow::anyhow!("Token exchange failed ({status}): {body_text}"));
    }

    let val: serde_json::Value = serde_json::from_str(&body_text)
        .with_context(|| format!("Token response is not JSON: {body_text}"))?;
    info!("Token response keys: {:?}", val.as_object().map(|o| o.keys().collect::<Vec<_>>()));

    let token_str = val.get("token").and_then(|v| v.as_str())
        .with_context(|| format!("No 'token' field in response: {body_text}"))?.to_string();
    let expires_at_str = val.get("expires_at").and_then(|v| v.as_str()).map(|s| s.to_string());
    debug!("Copilot API token acquired (expires_at={:?})", expires_at_str);

    let tr = TokenResponse { token: token_str, expires_at: expires_at_str };
    let expires_at = tr.expires_at.as_deref().and_then(chrono_unix_from_iso)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() + 1800).unwrap_or(1800)
        });

    Ok(CopilotApiToken { token: tr.token, expires_at })
}

fn chrono_unix_from_iso(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let s = if let Some(pos) = s.find('+') { &s[..pos] } else { s };
    let s = if let Some(pos) = s.rfind('-') { if pos > 10 { &s[..pos] } else { s } } else { s };
    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 { return None; }
    let date: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time: Vec<u64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();
    if date.len() < 3 || time.len() < 3 { return None; }
    let y = date[0].saturating_sub(1970);
    let days = y * 365 + y / 4 + days_before_month(date[1], date[0]) + date[2] - 1;
    Some(days * 86400 + time[0] * 3600 + time[1] * 60 + time[2])
}

fn days_before_month(month: u64, year: u64) -> u64 {
    let dim = [0u64, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 1 } else { 0 };
    let mut total = 0;
    for m in 1..month.min(13) {
        total += dim[m as usize];
        if m == 2 { total += leap; }
    }
    total
}
