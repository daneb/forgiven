//! Copilot Chat / agent panel.
//!
//! Auth flow:
//!   1. Read the GitHub OAuth token from ~/.config/github-copilot/apps.json
//!   2. Exchange it for a short-lived Copilot API token via the GitHub API
//!   3. Stream chat completions from api.githubcopilot.com (OpenAI-compatible SSE)
//!
//! The panel state (messages, input, visibility) lives here.  Streaming tokens
//! are sent over a tokio mpsc channel so the editor event-loop can poll them
//! non-blocking each frame.

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Data types
// ─────────────────────────────────────────────────────────────────────────────

/// A single message in the chat history.
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
    /// Whether the panel is visible on screen.
    pub visible: bool,

    /// Whether keyboard focus is inside the panel (vs the editor).
    pub focused: bool,

    /// Chat history shown in the panel.
    pub messages: Vec<ChatMessage>,

    /// Current user input being composed.
    pub input: String,

    /// Scroll offset: number of rendered lines to show above the bottom.
    /// 0 = pinned to bottom (newest content); higher = scrolled up toward older content.
    pub scroll: usize,

    /// Cached Copilot API token (short-lived; refreshed on expiry).
    token: Option<CopilotApiToken>,

    /// Current assistant reply being built from streaming chunks.
    pub streaming_reply: Option<String>,

    /// Receiver for streaming tokens from an in-flight request.
    pub stream_rx: Option<mpsc::UnboundedReceiver<StreamEvent>>,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A text token to append to the current reply.
    Token(String),
    /// The stream finished successfully.
    Done,
    /// An error occurred.
    Error(String),
}

#[derive(Debug, Clone)]
struct CopilotApiToken {
    token: String,
    /// Unix timestamp when the token expires.
    expires_at: u64,
}

impl CopilotApiToken {
    fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Refresh 60 s before expiry to avoid races.
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
        }
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.focused = true;
        } else {
            self.focused = false;
        }
    }

    pub fn focus(&mut self) {
        self.focused = true;
    }

    pub fn blur(&mut self) {
        self.focused = false;
    }

    /// Push a character to the input buffer.
    pub fn input_char(&mut self, ch: char) {
        self.input.push(ch);
    }

    /// Delete the last character from the input buffer.
    pub fn input_backspace(&mut self) {
        self.input.pop();
    }

    /// Submit the current input as a user message and start a chat request.
    /// Returns an error if we can't authenticate or start the stream.
    pub async fn submit(&mut self, context: Option<String>) -> Result<()> {
        if self.input.trim().is_empty() {
            return Ok(());
        }

        let user_text = std::mem::take(&mut self.input);

        // Build message list to send.
        let mut send_messages: Vec<serde_json::Value> = Vec::new();

        // System prompt with optional file context.
        let system = if let Some(ctx) = context {
            format!(
                "You are a helpful coding assistant embedded in the 'forgiven' terminal editor.\n\nCurrent file context:\n```\n{}\n```",
                ctx
            )
        } else {
            "You are a helpful coding assistant embedded in the 'forgiven' terminal editor.".to_string()
        };
        send_messages.push(serde_json::json!({ "role": "system", "content": system }));

        // Include prior conversation history (last 10 exchanges to stay within token budget).
        let history_start = self.messages.len().saturating_sub(20);
        for msg in &self.messages[history_start..] {
            send_messages.push(serde_json::json!({
                "role": msg.role.as_str(),
                "content": msg.content
            }));
        }

        // Append the new user message.
        send_messages.push(serde_json::json!({ "role": "user", "content": user_text.clone() }));

        // Record it in history.
        self.messages.push(ChatMessage { role: Role::User, content: user_text });

        // Pin view to bottom so the user sees their message + the incoming reply.
        self.scroll = 0;

        // Begin streaming reply (placeholder shown while tokens arrive).
        self.streaming_reply = Some(String::new());

        // Get / refresh the Copilot API token.
        let api_token = self.ensure_token().await?;

        // Spawn a background task that streams tokens into our channel.
        let (tx, rx) = mpsc::unbounded_channel::<StreamEvent>();
        self.stream_rx = Some(rx);

        let token_clone = api_token.clone();
        tokio::spawn(async move {
            // Fetch the streaming response.
            let response = match start_chat_stream(token_clone, send_messages).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(format!("{}", e)));
                    return;
                }
            };

            // Consume the SSE byte stream.
            let mut buf = String::new();
            let mut stream = response.bytes_stream();
            while let Some(item) = stream.next().await {
                let chunk: reqwest::Result<_> = item;
                match chunk {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        // Process complete SSE events (newline-terminated).
                        while let Some(pos) = buf.find('\n') {
                            let line = buf[..pos].trim().to_string();
                            buf.drain(..=pos);

                            if line == "data: [DONE]" {
                                let _ = tx.send(StreamEvent::Done);
                                return;
                            }
                            if let Some(json_str) = line.strip_prefix("data: ") {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                    if let Some(content) = val
                                        .pointer("/choices/0/delta/content")
                                        .and_then(|v| v.as_str())
                                    {
                                        if !content.is_empty() {
                                            let _ = tx.send(StreamEvent::Token(content.to_string()));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(StreamEvent::Error(format!("{}", e)));
                        return;
                    }
                }
            }
            let _ = tx.send(StreamEvent::Done);
        });

        Ok(())
    }

    /// Poll the stream receiver (non-blocking).  Returns true if the stream is
    /// still active so the caller knows to keep rendering.
    pub fn poll_stream(&mut self) -> bool {
        let mut active = false;
        if let Some(rx) = self.stream_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(StreamEvent::Token(t)) => {
                        active = true;
                        if let Some(reply) = self.streaming_reply.as_mut() {
                            reply.push_str(&t);
                        }
                    }
                    Ok(StreamEvent::Done) => {
                        // Commit the streamed reply into the history.
                        if let Some(text) = self.streaming_reply.take() {
                            if !text.is_empty() {
                                self.messages.push(ChatMessage {
                                    role: Role::Assistant,
                                    content: text,
                                });
                                // Auto-scroll to bottom so the new reply is visible.
                                self.scroll = 0;
                            }
                        }
                        self.stream_rx = None;
                        break;
                    }
                    Ok(StreamEvent::Error(e)) => {
                        warn!("Copilot Chat stream error: {}", e);
                        let msg = format!("[Error: {}]", e);
                        self.messages.push(ChatMessage { role: Role::Assistant, content: msg });
                        self.streaming_reply = None;
                        self.stream_rx = None;
                        break;
                    }
                    Err(_) => break, // channel empty
                }
            }
        }
        active
    }

    /// Scroll the chat history up (toward older messages).
    /// The renderer caps this against the actual line count.
    pub fn scroll_up(&mut self) {
        self.scroll += 3;
    }

    /// Scroll the chat history down (toward newer messages / bottom).
    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_sub(3);
    }

    /// Pin the view to the bottom (newest content).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
    }

    // ── Code extraction ───────────────────────────────────────────────────────

    /// Extract all fenced code blocks (``` … ```) from `text`.
    /// The fence line itself (```lang) is not included in the output.
    pub fn extract_code_blocks(text: &str) -> Vec<String> {
        let mut blocks: Vec<String> = Vec::new();
        let mut in_block = false;
        let mut current: Vec<&str> = Vec::new();

        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                if in_block {
                    // Closing fence — commit block (strip trailing blank lines).
                    while current.last().map(|l: &&str| l.trim().is_empty()).unwrap_or(false) {
                        current.pop();
                    }
                    if !current.is_empty() {
                        blocks.push(current.join("\n"));
                    }
                    current.clear();
                    in_block = false;
                } else {
                    // Opening fence — start collecting.
                    in_block = true;
                }
            } else if in_block {
                current.push(line);
            }
        }
        blocks
    }

    /// Return the first code block from the latest assistant message, if any.
    pub fn get_code_to_apply(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .and_then(|m| Self::extract_code_blocks(&m.content).into_iter().next())
    }

    /// True when the latest assistant reply contains at least one code block.
    pub fn has_code_to_apply(&self) -> bool {
        self.get_code_to_apply().is_some()
    }

    // ── Private auth helpers ──────────────────────────────────────────────────

    async fn ensure_token(&mut self) -> Result<String> {
        if let Some(ref t) = self.token {
            if !t.is_expired() {
                return Ok(t.token.clone());
            }
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
// Auth helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read the GitHub OAuth token from ~/.config/github-copilot/apps.json.
fn load_oauth_token() -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = format!("{}/.config/github-copilot/apps.json", home);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path))?;
    let val: serde_json::Value =
        serde_json::from_str(&raw).context("apps.json is not valid JSON")?;

    // Structure: { "<app-key>": { "oauth_token": "ghu_…", … } }
    val.as_object()
        .and_then(|m| m.values().next())
        .and_then(|entry| entry.get("oauth_token"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .context("oauth_token not found in apps.json")
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    token: String,
    expires_at: Option<String>,
}

/// Exchange the GitHub OAuth token for a short-lived Copilot API token.
async fn exchange_token(oauth_token: &str) -> Result<CopilotApiToken> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.github.com/copilot_internal/v2/token")
        .header("Authorization", format!("token {}", oauth_token))
        .header("User-Agent", "forgiven/0.1.0")
        .header("Accept", "application/json")
        .send()
        .await
        .context("Failed to reach GitHub API")?;

    let status = resp.status();
    // Read the raw body so we can log it before attempting to parse.
    let body_text = resp.text().await.unwrap_or_default();
    debug!("Token exchange response ({status}): {body_text}");

    if !status.is_success() {
        return Err(anyhow::anyhow!("Token exchange failed ({status}): {body_text}"));
    }

    // Parse via serde_json::Value first so we can extract fields flexibly
    // and log exactly what the API returned when parsing fails.
    let val: serde_json::Value = serde_json::from_str(&body_text)
        .with_context(|| format!("Token response is not JSON: {body_text}"))?;

    info!("Token response keys: {:?}", val.as_object().map(|o| o.keys().collect::<Vec<_>>()));

    let token_str = val.get("token")
        .and_then(|v| v.as_str())
        .with_context(|| format!("No 'token' field in response: {body_text}"))?
        .to_string();

    let expires_at_str = val.get("expires_at").and_then(|v| v.as_str()).map(|s| s.to_string());
    debug!("Copilot API token acquired (expires_at={:?})", expires_at_str);
    let tr = TokenResponse { token: token_str, expires_at: expires_at_str };

    // Parse ISO-8601 expiry if present, otherwise assume 30 minutes.
    let expires_at = tr
        .expires_at
        .as_deref()
        .and_then(|s| {
            // "2024-01-01T12:34:56+00:00" → unix seconds via manual parse
            chrono_unix_from_iso(s)
        })
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() + 1800)
                .unwrap_or(1800)
        });

    Ok(CopilotApiToken { token: tr.token, expires_at })
}

/// Very small ISO-8601 → Unix timestamp parser (avoids a chrono dependency).
fn chrono_unix_from_iso(s: &str) -> Option<u64> {
    // We expect "2024-01-01T12:34:56+00:00" or "2024-01-01T12:34:56Z"
    // Parse via the standard library's SystemTime is not available, so we use
    // a simple regex-free approach.
    let s = s.trim_end_matches('Z');
    let s = if let Some(pos) = s.find('+') { &s[..pos] } else { s };
    let s = if let Some(pos) = s.rfind('-') {
        // Only strip trailing -HH:MM if it looks like a timezone offset.
        if pos > 10 { &s[..pos] } else { s }
    } else {
        s
    };
    // s should now be "2024-01-01T12:34:56"
    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 { return None; }
    let date: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time: Vec<u64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();
    if date.len() < 3 || time.len() < 3 { return None; }
    // Days since epoch (approximate — good enough for token expiry).
    let years_since_1970 = date[0].saturating_sub(1970);
    let leap_years = years_since_1970 / 4;
    let days = years_since_1970 * 365 + leap_years
        + days_before_month(date[1], date[0]) + date[2] - 1;
    Some(days * 86400 + time[0] * 3600 + time[1] * 60 + time[2])
}

fn days_before_month(month: u64, year: u64) -> u64 {
    let days_in_month = [0u64, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 1 } else { 0 };
    let mut total = 0;
    for m in 1..month.min(13) {
        total += days_in_month[m as usize];
        if m == 2 { total += leap; }
    }
    total
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming chat request
// ─────────────────────────────────────────────────────────────────────────────

/// Start a streaming chat request and return the raw `reqwest::Response`.
/// The caller is responsible for consuming the byte stream via `response.bytes_stream()`.
async fn start_chat_stream(
    api_token: String,
    messages: Vec<serde_json::Value>,
) -> Result<reqwest::Response> {
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": messages,
        "stream": true,
        "n": 1,
        "temperature": 0.1,
        "max_tokens": 4096
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.githubcopilot.com/chat/completions")
        .header("Authorization", format!("Bearer {}", api_token))
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
