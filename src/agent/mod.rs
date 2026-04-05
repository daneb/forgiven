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

mod agentic_loop;
mod auth;
mod models;
mod panel;
pub mod provider;
pub mod token_count;
pub mod tools;
pub use auth::acquire_copilot_token;
use auth::CopilotApiToken;
pub use provider::ProviderKind;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::mcp::McpManager;
use crate::spec_framework::SpecFramework;

// ─────────────────────────────────────────────────────────────────────────────
// Data types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Image attachment placeholders: `(width, height)`.
    /// The base64 data is NOT stored in history to avoid unbounded memory growth.
    pub images: Vec<(u32, u32)>,
}

/// Score a message's importance for history retention (higher = keep longer).
///
/// Scores are additive weights used by the importance-scored truncation in
/// `send_message()` to prefer dropping large low-value messages before small
/// high-value ones when the context budget is tight.
fn message_importance(msg: &ChatMessage) -> u32 {
    let mut score: u32 = match msg.role {
        Role::User => 3,      // user instructions define the task
        Role::Assistant => 2, // model replies carry context
        Role::System => 0,    // display-only dividers, never sent to API
    };
    let c = &msg.content;
    // Messages containing errors or failures are highly valuable to retain.
    if c.contains("error") || c.contains("Error") || c.contains("failed") || c.contains("panic") {
        score += 3;
    }
    // Large messages that look like raw file reads (line-numbered output) or batch
    // results are low-value once the model has already acted on them.
    if c.len() > 2000 && (c.contains(" | ") || c.starts_with("=== ")) {
        score = score.saturating_sub(2);
    }
    score
}

/// An image captured from the system clipboard via Ctrl+V.
/// Stored as a pre-encoded PNG base64 data URI ready for the API.
#[derive(Debug, Clone)]
pub struct ClipboardImage {
    /// Width of the original image in pixels.
    pub width: u32,
    /// Height of the original image in pixels.
    pub height: u32,
    /// Complete data URI: `"data:image/png;base64,<encoded>"`.
    pub data_uri: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Assistant,
    #[allow(dead_code)] // used in as_str(); reserved for system-prompt messages
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
// Think-block splitting
// ─────────────────────────────────────────────────────────────────────────────

/// A segment of an assistant reply split on `<think>` / `</think>` tags.
#[derive(Debug)]
pub enum ContentSegment {
    /// Chain-of-thought reasoning — render as plain dim text, no markdown.
    Thinking(String),
    /// Normal reply content — render as formatted markdown.
    Normal(String),
}

/// Split `content` on `<think>` / `</think>` into alternating [`ContentSegment`]s.
///
/// An unclosed `<think>` (common mid-stream before `</think>` has arrived)
/// produces a trailing `Thinking` segment from the open tag to end-of-string.
pub fn split_thinking(content: &str) -> Vec<ContentSegment> {
    let mut segments = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        match remaining.find("<think>") {
            Some(start) => {
                let before = &remaining[..start];
                if !before.trim().is_empty() {
                    segments.push(ContentSegment::Normal(before.to_owned()));
                }
                let after_open = &remaining[start + 7..]; // skip "<think>"
                match after_open.find("</think>") {
                    Some(end) => {
                        let thinking = after_open[..end].trim().to_owned();
                        if !thinking.is_empty() {
                            segments.push(ContentSegment::Thinking(thinking));
                        }
                        remaining = &after_open[end + 8..]; // skip "</think>"
                    },
                    None => {
                        // Unclosed tag — rest is in-progress thinking (streaming).
                        let thinking = after_open.trim().to_owned();
                        if !thinking.is_empty() {
                            segments.push(ContentSegment::Thinking(thinking));
                        }
                        break;
                    },
                }
            },
            None => {
                if !remaining.trim().is_empty() {
                    segments.push(ContentSegment::Normal(remaining.to_owned()));
                }
                break;
            },
        }
    }

    segments
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent panel state
// ─────────────────────────────────────────────────────────────────────────────

/// A single planned step shown in the agent panel's task strip.
#[derive(Debug, Clone)]
pub struct AgentTask {
    pub title: String,
    pub done: bool,
}

/// State for the ask_user dialog while the agent is waiting for a response.
#[derive(Debug, Clone)]
pub struct AskUserState {
    pub question: String,
    pub options: Vec<String>,
    pub selected: usize,
}

/// State for the slash-command autocomplete dropdown.
#[derive(Debug, Clone)]
pub struct SlashMenuState {
    /// Filtered command names matching the current input prefix.
    pub items: Vec<String>,
    /// Currently highlighted index.
    pub selected: usize,
    /// Description of the currently selected command, if available.
    pub description: Option<String>,
}

/// Maximum number of lines included from a file attached via the Ctrl+P picker.
/// Files exceeding this limit are truncated and a warning is appended.
pub const AT_PICKER_MAX_LINES: usize = 500;

/// Per-segment token breakdown captured at `submit()` time.
/// Shown in the `SPC d` diagnostics overlay and the status-bar fuel gauge.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextBreakdown {
    /// Tokens used by the system-prompt rules + preamble (without the open file).
    pub sys_rules_t: u32,
    /// Tokens used by the open-file snippet injected into the system prompt.
    pub ctx_file_t: u32,
    /// Tokens used by the chat history sent this round (after truncation).
    pub history_t: u32,
    /// Tokens used by the new user message.
    pub user_msg_t: u32,
    /// Model context window size in tokens.
    pub ctx_window: u32,
}

impl ContextBreakdown {
    pub fn total(&self) -> u32 {
        self.sys_rules_t + self.ctx_file_t + self.history_t + self.user_msg_t
    }

    /// Percentage of the context window consumed (0–100).
    pub fn used_pct(&self) -> u32 {
        self.total() * 100 / self.ctx_window.max(1)
    }
}

/// Context-budget snapshot captured at `submit()` time, correlated with the
/// `StreamEvent::Usage` that arrives after the round completes.
/// Used to write per-invocation metrics to `~/.local/share/forgiven/sessions.jsonl`.
#[derive(Debug, Clone, Copy)]
pub struct SubmitCtx {
    /// Model context window in tokens (from the /models API, or 128k fallback).
    pub ctx_window: u32,
    /// Estimated system-prompt tokens (system.len() / 4).
    pub sys_tokens: u32,
    /// Tokens remaining for history after system-prompt deduction (80% of window − sys).
    pub budget_for_history: u32,
}

/// Resolve the path for the persistent session-metrics JSONL file.
/// `~/.local/share/forgiven/sessions.jsonl` (XDG_DATA_HOME-aware).
pub fn metrics_data_path() -> Option<std::path::PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        std::path::PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        std::path::PathBuf::from(home).join(".local/share")
    };
    Some(base.join("forgiven").join("sessions.jsonl"))
}

/// Append one JSON line to the persistent session-metrics file.
/// Creates the directory and file on first use. Silently swallows I/O errors
/// so a permissions problem never interrupts the agentic loop.
pub fn append_session_metric(record: &serde_json::Value) {
    let Some(path) = metrics_data_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut line = record.to_string();
    line.push('\n');
    use std::io::Write as _;
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

/// Transient state for the Ctrl+P file-context picker overlay in the agent panel.
/// `None` when the picker is closed; `Some` while it is open.
#[derive(Debug, Clone)]
pub struct AtPickerState {
    /// Search query as the user types.
    pub query: String,
    /// Fuzzy-filtered results: (absolute path, matched char indices for highlighting).
    /// Recomputed on the editor side each time `query` changes.
    pub results: Vec<(PathBuf, Vec<usize>)>,
    /// Index of the currently highlighted row.
    pub selected: usize,
}

pub struct AgentPanel {
    pub visible: bool,
    pub focused: bool,
    /// Active provider.  Set at startup from config; never changed at runtime.
    pub provider: ProviderKind,
    /// Base URL of the Ollama server (e.g. `"http://localhost:11434"`).
    /// Only used when `provider == ProviderKind::Ollama`.
    pub ollama_base_url: String,
    /// Explicit `num_ctx` for Ollama requests.  Pins the active KV-cache size
    /// on the server so history truncation and Ollama are in sync.
    pub ollama_context_length: Option<u32>,
    /// Whether to enable tool calling for Ollama (default: false).
    /// Set from `[provider.ollama] tool_calls = true` in config.
    pub ollama_tool_calls: bool,
    pub messages: Vec<ChatMessage>,
    /// Messages from sessions that have been compressed by the Auto-Janitor.
    /// Rendered above the live session in a dimmed style so the user can still
    /// scroll back to them.  Excluded from the API history sent in `submit()`.
    /// Cleared by `new_conversation()`.
    pub archived_messages: Vec<ChatMessage>,
    pub input: String,
    pub scroll: usize,
    token: Option<CopilotApiToken>,
    pub streaming_reply: Option<String>,
    pub stream_rx: Option<mpsc::UnboundedReceiver<StreamEvent>>,
    /// Channel to send continuation decisions back to the agentic loop.
    /// When the loop hits max rounds, it sends AwaitingContinuation and waits on this channel.
    pub continuation_tx: Option<mpsc::UnboundedSender<bool>>,
    /// Paths (project-relative) of files modified by the agent this frame.
    /// The editor drains this each tick to reload open buffers.
    pub pending_reloads: Vec<String>,
    /// Planned steps for the current agent session (shown as a strip in the panel).
    pub tasks: Vec<AgentTask>,
    /// Models fetched from GET /models (lazily populated on first submit).
    /// Each entry holds the API `id` (sent in requests) and human-readable `name` (shown in UI).
    pub available_models: Vec<ModelVersion>,
    /// Index into available_models for the currently selected model.
    pub selected_model: usize,
    /// Current agentic loop round (for UI display).
    pub current_round: usize,
    /// Maximum rounds configured.
    pub max_rounds: usize,
    /// Whether the agent is paused waiting for user to approve continuation.
    pub awaiting_continuation: bool,
    /// Live status of the background Copilot task (shown in the panel title).
    pub status: AgentStatus,
    /// Set by `poll_stream()` when a `StreamEvent::Error` arrives so the
    /// editor run-loop can forward it to the status bar.  Cleared on read.
    pub last_error: Option<String>,
    /// Token counts from the last API response (0 = not yet received).
    pub last_prompt_tokens: u32,
    pub last_completion_tokens: u32,
    /// Tokens served from the provider's prompt cache in the last response.
    pub last_cached_tokens: u32,
    /// Cumulative prompt + completion tokens for the current conversation session.
    /// NOTE: prompt total is a re-send cost (history is re-sent each round), not
    /// a count of unique tokens. Divide by session_rounds for average per-invocation cost.
    /// Reset by `new_conversation()`. Shown in the SPC d diagnostics overlay.
    pub total_session_prompt_tokens: u32,
    pub total_session_completion_tokens: u32,
    /// Number of completed agent invocations in this conversation session.
    /// Incremented on each StreamEvent::Done. Reset by new_conversation().
    pub session_rounds: u32,
    /// Cycle index for the Ctrl+K copy-code-block command.
    pub code_block_idx: usize,
    /// Cycle index for the Ctrl+M view-mermaid-diagram command.
    pub mermaid_block_idx: usize,
    /// Pasted content blocks captured via bracketed paste; shown as summary lines in the input box.
    /// Each entry is `(text, line_count)` — the count is pre-computed at paste time so the
    /// render path never has to scan the text again.
    pub pasted_blocks: Vec<(String, usize)>,
    /// MCP manager shared with the agentic loop.  Set by the editor at startup
    /// after loading the config and spawning MCP server processes.
    pub mcp_manager: Option<Arc<McpManager>>,
    /// Optional prompt framework (e.g. spec-kit) that intercepts `/command` input
    /// and injects structured prompt templates before submission.
    pub spec_framework: Option<SpecFramework>,
    /// Oneshot sender that aborts the running agentic loop when dropped or fired.
    /// `None` when no stream is active.
    abort_tx: Option<oneshot::Sender<()>>,
    /// Channel to send the user's answer back to the agentic loop when ask_user is active.
    pub question_tx: Option<mpsc::UnboundedSender<String>>,
    /// Set when the agent has asked a question and is waiting for the user to respond.
    pub asking_user: Option<AskUserState>,
    /// Slash-command autocomplete dropdown state. Some while the user is typing a `/` command.
    pub slash_menu: Option<SlashMenuState>,
    /// Context-budget snapshot from the most recent `submit()` call.
    /// Correlated with `StreamEvent::Usage` to write per-invocation metrics.
    pub last_submit_ctx: Option<SubmitCtx>,
    /// Per-segment token breakdown from the most recent `submit()` call.
    /// Drives the `SPC d` Context Breakdown section and the status-bar fuel gauge.
    pub last_breakdown: Option<ContextBreakdown>,
    /// Model ID used in the most recent `submit()` call (e.g. "claude-sonnet-4").
    pub last_submit_model: String,
    /// Images captured from the system clipboard via Ctrl+V.
    /// Each entry is a pre-encoded PNG ready for submission.
    /// Cleared by `submit()` via `std::mem::take`.
    pub image_blocks: Vec<ClipboardImage>,
    /// Files attached via Ctrl+P picker: (display_name, content, line_count).
    /// display_name is the cwd-relative path shown as a badge.
    /// content is the (possibly truncated) file text injected at submit time.
    /// Cleared by `submit()` via `std::mem::take`.
    pub file_blocks: Vec<(String, String, usize)>,
    /// Ctrl+P file-context picker state. `Some` while the overlay is open.
    pub at_picker: Option<AtPickerState>,
    /// Set by `compress_history()` to signal that the in-flight submit is a
    /// janitor summarisation round.  Cleared in `poll_stream()` when `Done`
    /// arrives, after the summary has replaced the message history.
    pub janitor_compressing: bool,
    /// Set by `poll_stream()` when a round completes and `total_session_prompt_tokens`
    /// exceeds the configured threshold.  The editor tick-loop reads this flag
    /// and triggers `Action::AgentJanitorCompress` automatically.
    pub pending_janitor: bool,
    /// Cached project file-tree string (depth 2), rebuilt at most once every 30 s.
    /// Avoids a full filesystem walk on every `submit()` call.
    /// Cleared by `new_conversation()` to force a fresh tree on the next session.
    pub cached_project_tree: Option<(String, std::time::Instant)>,
    /// Original file contents captured before the agent first modifies each file in
    /// the current session.  Used by `revert_session()` (`SPC a u`) to restore all
    /// agent-touched files to their pre-session state.
    /// Keys are project-relative paths; values are the file contents before the
    /// agent's first edit.  Cleared by `new_conversation()`.
    pub session_snapshots: std::collections::HashMap<String, String>,
}

/// A model returned by the Copilot `/models` endpoint.
/// `id` is the model identifier sent in API requests (e.g. "claude-sonnet-4", "gpt-5.1");
/// `version` is informational metadata (e.g. "gpt-4o-2024-11-20");
/// `name` is the human-readable display label shown in the UI.
#[derive(Debug, Clone)]
pub struct ModelVersion {
    pub id: String,
    pub version: String,
    pub name: String,
    /// Context window size in tokens, parsed from `capabilities.limits.max_context_window_tokens`.
    pub context_window: u32,
}

/// What the Copilot background task is actively doing right now.
/// Used to render a live status indicator in the agent panel title.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum AgentStatus {
    #[default]
    Idle,
    /// API request sent for `round`; waiting for the first token.
    WaitingForResponse { round: usize },
    /// Tokens are actively streaming in for `round`.
    Streaming { round: usize },
    /// A tool is executing synchronously between rounds.
    CallingTool { round: usize, name: String },
    /// API call failed; retrying with exponential backoff.
    Retrying { attempt: usize, max: usize },
}

impl AgentStatus {
    /// Short label shown in the panel title.
    pub fn label(&self, max_rounds: usize) -> Option<String> {
        match self {
            AgentStatus::Idle => None,
            AgentStatus::WaitingForResponse { round } => {
                Some(format!("waiting… [{round}/{max_rounds}]"))
            },
            AgentStatus::Streaming { round } => Some(format!("streaming [{round}/{max_rounds}]")),
            AgentStatus::CallingTool { round, name } => {
                Some(format!("{name} [{round}/{max_rounds}]"))
            },
            AgentStatus::Retrying { attempt, max } => Some(format!("retrying ({attempt}/{max})…")),
        }
    }
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Token(String),
    ToolStart {
        name: String,
        args_summary: String,
    },
    ToolDone {
        name: String,
        result_summary: String,
    },
    /// A file was successfully written or edited by a tool.
    /// The path is project-relative (as passed to the tool).
    FileModified {
        path: String,
    },
    /// Sent BEFORE the first write/edit of a file in a session so the panel
    /// can store the original content for session-level undo (`SPC a u`).
    FileSnapshot {
        path: String,
        original: String,
    },
    /// A task was created by the agent via the create_task tool.
    TaskCreated {
        title: String,
    },
    /// A task was marked done by the agent via the complete_task tool.
    TaskCompleted {
        title: String,
    },
    /// Progress indicator: current round and max rounds.
    RoundProgress {
        current: usize,
        max: usize,
    },
    /// Warning that the max rounds limit is approaching.
    /// The loop will pause after this round and wait for user input.
    MaxRoundsWarning {
        current: usize,
        max: usize,
        remaining: usize,
    },
    /// Request user decision on whether to continue.
    /// The loop is paused and waiting for a response via the continuation channel.
    AwaitingContinuation,
    /// Agent is asking the user a question and waiting for their choice.
    /// The loop is paused; the answer is returned via the question channel.
    AskingUser {
        question: String,
        options: Vec<String>,
    },
    Done,
    Error(String),
    /// API call failed and the loop is about to sleep before retrying.
    Retrying {
        attempt: usize,
        max: usize,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
        /// Tokens served from the provider's prompt cache (0 if caching not active).
        cached_tokens: u32,
    },
    /// The API silently routed the request to a different model (e.g. premium quota exceeded).
    ModelSwitched {
        from: String,
        to: String,
    },
}
