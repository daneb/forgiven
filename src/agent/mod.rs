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
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::mcp::McpManager;
use crate::spec_framework::SpecFramework;

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
}

/// Maximum number of lines included from a file attached via the Ctrl+P picker.
/// Files exceeding this limit are truncated and a warning is appended.
pub const AT_PICKER_MAX_LINES: usize = 500;

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
    pub messages: Vec<ChatMessage>,
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
    /// Cycle index for the 'c' copy-code-block command.
    pub code_block_idx: usize,
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
    /// Files attached via Ctrl+P picker: (display_name, content, line_count).
    /// display_name is the cwd-relative path shown as a badge.
    /// content is the (possibly truncated) file text injected at submit time.
    /// Cleared by `submit()` via `std::mem::take`.
    pub file_blocks: Vec<(String, String, usize)>,
    /// Ctrl+P file-context picker state. `Some` while the overlay is open.
    pub at_picker: Option<AtPickerState>,
}

/// A model returned by the Copilot `/models` endpoint.
/// `id` is the API alias (e.g. "gpt-4o"); `version` is the pinned build sent in requests
/// (e.g. "gpt-4o-2024-11-20"); `name` is the human-readable display label.
#[derive(Debug, Clone)]
pub struct ModelVersion {
    pub id: String,
    pub version: String,
    pub name: String,
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
    },
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
            continuation_tx: None,
            pending_reloads: Vec::new(),
            tasks: Vec::new(),
            available_models: Vec::new(),
            selected_model: 0,
            current_round: 0,
            max_rounds: 20,
            awaiting_continuation: false,
            status: AgentStatus::Idle,
            last_error: None,
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
            code_block_idx: 0,
            pasted_blocks: Vec::new(),
            mcp_manager: None,
            spec_framework: None,
            abort_tx: None,
            question_tx: None,
            asking_user: None,
            slash_menu: None,
            file_blocks: Vec::new(),
            at_picker: None,
        }
    }

    /// The pinned model version to send in API requests.
    /// Using `version` (e.g. "gpt-4o-2024-11-20") rather than the alias `id` ("gpt-4o")
    /// ensures the Copilot API routes to the exact build, not an internal default.
    /// Falls back to "gpt-4o" before the models list has been fetched.
    pub fn selected_model_id(&self) -> &str {
        if self.available_models.is_empty() {
            return "gpt-4o";
        }
        &self.available_models[self.selected_model.min(self.available_models.len() - 1)].id
    }

    /// The human-readable display name for the selected model (shown in the UI).
    pub fn selected_model_display(&self) -> &str {
        if self.available_models.is_empty() {
            return "gpt-4o";
        }
        &self.available_models[self.selected_model.min(self.available_models.len() - 1)].name
    }

    /// Returns the known context-window size (in tokens) for the selected model.
    pub fn context_window_size(&self) -> u32 {
        let id = self.selected_model_id();
        if id.starts_with("gpt-4o")
            || id.starts_with("gpt-4")
            || id.starts_with("o1")
            || id.starts_with("o3")
        {
            128_000
        } else if id.starts_with("claude") {
            200_000
        } else {
            128_000
        }
    }

    /// Advance to the next model in the list (wraps around).
    pub fn cycle_model(&mut self) {
        if !self.available_models.is_empty() {
            self.selected_model = (self.selected_model + 1) % self.available_models.len();
        }
    }

    /// Ensure the model list is populated.  Fetches from /models if it hasn't
    /// been loaded yet.  Safe to call multiple times — no-op after first load.
    pub async fn ensure_models(&mut self, preferred_model: &str) -> Result<()> {
        if !self.available_models.is_empty() {
            return Ok(());
        }
        let api_token = self.ensure_token().await?;
        match fetch_models(&api_token).await {
            Ok(models) if !models.is_empty() => {
                self.set_models(models, preferred_model);
            },
            Ok(_) => warn!("Copilot /models returned an empty list"),
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Refresh the model list from the API, preserving the current selection if possible.
    /// Use this to pick up newly released models or remove deprecated ones.
    pub async fn refresh_models(&mut self, preferred_model: &str) -> Result<()> {
        let current_id = if !self.available_models.is_empty() {
            Some(self.available_models[self.selected_model].id.clone())
        } else {
            None
        };

        let api_token = self.ensure_token().await?;
        match fetch_models(&api_token).await {
            Ok(models) if !models.is_empty() => {
                let preferred = current_id.as_deref().unwrap_or(preferred_model);
                self.set_models(models, preferred);
                info!(
                    "Refreshed model list, selected: {} ({})",
                    self.selected_model_display(),
                    self.selected_model_id()
                );
            },
            Ok(_) => warn!("Copilot /models returned an empty list"),
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Set the available models and select the preferred one (or fallback).
    /// Matches `preferred_model` against `id` first, then `version` (so configs that stored
    /// a versioned ID like "gpt-4o-2024-11-20" still resolve correctly).
    fn set_models(&mut self, models: Vec<ModelVersion>, preferred_model: &str) {
        let found =
            models.iter().position(|m| m.id == preferred_model || m.version == preferred_model);
        if found.is_none() && !preferred_model.is_empty() {
            warn!(
                "Preferred model '{}' not found in model list; falling back. Available ids: {:?}",
                preferred_model,
                models.iter().map(|m| &m.id).collect::<Vec<_>>()
            );
        }
        let default_idx =
            found.or_else(|| models.iter().position(|m| m.id == "gpt-4o")).unwrap_or(0);
        self.available_models = models;
        self.selected_model = default_idx;
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        self.focused = self.visible;
    }

    pub fn focus(&mut self) {
        self.focused = true;
    }
    pub fn blur(&mut self) {
        self.focused = false;
    }
    pub fn input_char(&mut self, ch: char) {
        self.input.push(ch);
    }
    pub fn input_backspace(&mut self) {
        self.input.pop();
    }
    pub fn input_newline(&mut self) {
        self.input.push('\n');
    }

    /// Recompute the slash-command dropdown based on the current input.
    /// Call this whenever `self.input` changes in Agent mode.
    pub fn update_slash_menu(&mut self) {
        let Some(ref fw) = self.spec_framework else {
            self.slash_menu = None;
            return;
        };
        // Only active when the input starts with '/'
        if !self.input.starts_with('/') {
            self.slash_menu = None;
            return;
        }
        let prefix = self.input.trim_start_matches('/');
        let all = fw.commands();
        let items: Vec<String> =
            all.into_iter().filter(|cmd| cmd.starts_with(prefix)).map(|s| s.to_string()).collect();

        if items.is_empty() {
            self.slash_menu = None;
            return;
        }

        match self.slash_menu.as_mut() {
            Some(menu) => {
                // Preserve selection index if still valid, otherwise reset.
                let prev = menu.selected;
                menu.items = items;
                menu.selected = prev.min(menu.items.len().saturating_sub(1));
            },
            None => {
                self.slash_menu = Some(SlashMenuState { items, selected: 0 });
            },
        }
    }

    /// Move the slash-menu selection by `delta` (+1 = down, -1 = up).
    pub fn move_slash_selection(&mut self, delta: i32) {
        if let Some(ref mut menu) = self.slash_menu {
            let n = menu.items.len();
            if n > 0 {
                menu.selected = (menu.selected as i32 + delta).rem_euclid(n as i32) as usize;
            }
        }
    }

    /// Complete the selected slash-command into `self.input`.
    /// Any text after the command name (context) is preserved.
    pub fn complete_slash_selection(&mut self) {
        let Some(ref menu) = self.slash_menu else { return };
        let cmd = match menu.items.get(menu.selected) {
            Some(c) => c.clone(),
            None => return,
        };
        // Preserve any context text typed after the slash-command.
        let rest = self
            .input
            .trim_start_matches('/')
            .split_once(char::is_whitespace)
            .map(|(_, r)| format!(" {}", r.trim_start()))
            .unwrap_or_default();
        self.input = format!("/{cmd}{rest}");
        self.slash_menu = None;
    }

    /// Clear conversation history and insert a visual divider showing the new model.
    /// Called when the user switches models via Ctrl+T so the new model receives a
    /// clean context — not the prior conversation from a different model.
    pub fn new_conversation(&mut self, model_name: &str) {
        self.messages.clear();
        self.tasks.clear();
        self.streaming_reply = None;
        self.messages.push(ChatMessage {
            role: Role::System,
            content: format!("── New conversation · {model_name} ──"),
        });
    }

    /// Submit input, launching the agentic tool-calling loop in the background.
    pub async fn submit(
        &mut self,
        context: Option<String>,
        project_root: PathBuf,
        max_rounds: usize,
        warning_threshold: usize,
        preferred_model: &str,
    ) -> Result<()> {
        if self.input.trim().is_empty()
            && self.pasted_blocks.is_empty()
            && self.file_blocks.is_empty()
        {
            return Ok(());
        }

        let typed_text = std::mem::take(&mut self.input);
        let pasted = std::mem::take(&mut self.pasted_blocks);
        let files = std::mem::take(&mut self.file_blocks);

        // Assemble user text: file blocks first (structured context), then pasted
        // blocks (ad-hoc snippets), then typed input.  Each section separated by \n\n.
        let mut parts: Vec<String> = Vec::new();
        for (name, content, _) in &files {
            parts.push(format!("File: {name}\n\n```\n{content}\n```"));
        }
        for (text, _) in &pasted {
            parts.push(text.clone());
        }
        let user_text = if parts.is_empty() {
            typed_text
        } else {
            let mut combined = parts.join("\n\n");
            if !typed_text.trim().is_empty() {
                combined.push_str("\n\n");
                combined.push_str(&typed_text);
            }
            combined
        };
        // Slash-command interception: if a prompt framework is active and the user
        // typed "/<command> [context]", resolve the template and rebuild the message.
        // The template becomes the structured instruction; any trailing text is
        // appended as "user context" and the combined string is sent as the user turn.
        let user_text = if let Some(ref fw) = self.spec_framework {
            if let Some((template, rest)) = fw.resolve(&user_text) {
                // Append whatever the user typed after the command as context.
                if rest.is_empty() {
                    template.to_string()
                } else {
                    format!("{template}{rest}")
                }
            } else {
                user_text
            }
        } else {
            user_text
        };

        let root_display = project_root.display().to_string();

        // Build a shallow file tree so the model knows the project layout upfront
        // and never needs to burn rounds on list_directory exploration.
        let project_tree = build_project_tree(&project_root, 2);

        let tool_rules = "\
MANDATORY PROTOCOL — follow these rules without exception:\n\
\n\
TASK PLANNING RULES:\n\
0. Use create_task / complete_task ONLY when the job involves 3 or more distinct\n\
   file operations (creates, rewrites, or edits across different files), OR when\n\
   the user explicitly asks you to plan or list steps.\n\
   Do NOT plan for: questions, explanations, single-file edits, or any task\n\
   completable in 1-2 tool calls. Reading a file before editing it does NOT count\n\
   as a separate step.\n\
   When planning IS needed, call create_task ONCE per step BEFORE any file work,\n\
   keep titles short and imperative (e.g. 'Create Program.cs'), and call\n\
   complete_task with the exact same title after finishing each step.\n\
\n\
COMMUNICATION RULES:\n\
6. Do NOT output any text while working through tool calls. Work silently.\n\
   After ALL tools have finished, ALWAYS write a concise summary of what was\n\
   accomplished (files changed, what was added/removed/fixed, and any caveats).\n\
   Do not narrate steps, explain retries, or announce what you are about to do.\n\
7. Use ask_user ONLY when you genuinely cannot proceed without clarification —\n\
   e.g., ambiguous destructive actions or mutually exclusive design choices.\n\
   Do NOT use it to confirm routine read/write operations.\n\
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
- create_task     Register a planned step (call once per step before file work).\n\
- complete_task   Mark a step done (call after finishing each step).\n\
- read_file       Read a file (returns line-numbered content). REQUIRED before edits.\n\
- write_file      Write a complete file (for new files or full rewrites only).\n\
- edit_file       Surgical find-and-replace. old_str must match EXACTLY once.\n\
- list_directory  List a directory's contents.\n\
- ask_user        Show the user a question dialog and wait for their choice.\n";

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

        let mut send_messages: Vec<serde_json::Value> =
            vec![serde_json::json!({ "role": "system", "content": system })];

        let history_start = self.messages.len().saturating_sub(20);
        for msg in &self.messages[history_start..] {
            // Skip System-role entries — they are display-only dividers inserted by
            // new_conversation() and must not be forwarded to the API as context.
            if matches!(msg.role, Role::System) {
                continue;
            }
            send_messages.push(serde_json::json!({
                "role": msg.role.as_str(),
                "content": msg.content
            }));
        }
        send_messages.push(serde_json::json!({ "role": "user", "content": user_text.clone() }));
        self.messages.push(ChatMessage { role: Role::User, content: user_text });

        self.scroll = 0;
        self.streaming_reply = Some(String::new());
        self.tasks.clear();

        let api_token = self.ensure_token().await?;

        // Lazily populate the model list on first submit (or after a token refresh
        // that cleared it).  Failure is non-fatal — we just keep the fallback.
        if self.available_models.is_empty() {
            match fetch_models(&api_token).await {
                Ok(models) if !models.is_empty() => {
                    info!("Fetched {} models from Copilot API", models.len());
                    // Select user's preferred model from config
                    self.set_models(models, preferred_model);
                },
                Ok(_) => warn!("Copilot /models returned an empty list"),
                Err(e) => warn!("Could not fetch Copilot model list: {e}"),
            }
        }

        let model_id = self.selected_model_id().to_string();

        self.status = AgentStatus::WaitingForResponse { round: 1 };

        let (tx, rx) = mpsc::unbounded_channel::<StreamEvent>();
        self.stream_rx = Some(rx);

        let (cont_tx, cont_rx) = mpsc::unbounded_channel::<bool>();
        self.continuation_tx = Some(cont_tx);

        let (question_tx, question_rx) = mpsc::unbounded_channel::<String>();
        self.question_tx = Some(question_tx);

        let (abort_tx, abort_rx) = oneshot::channel::<()>();
        self.abort_tx = Some(abort_tx);

        let mcp = self.mcp_manager.as_ref().map(Arc::clone);
        tokio::spawn(agentic_loop(
            api_token,
            send_messages,
            project_root,
            tx,
            model_id,
            max_rounds,
            warning_threshold,
            cont_rx,
            question_rx,
            abort_rx,
            mcp,
        ));
        Ok(())
    }

    pub fn poll_stream(&mut self) -> bool {
        // Process at most this many tokens per frame to avoid stalling the render loop
        // when the LLM is streaming a large response at high speed.
        const MAX_TOKENS_PER_FRAME: usize = 64;
        let mut active = false;
        let mut token_count = 0usize;
        if let Some(rx) = self.stream_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(StreamEvent::Token(t)) => {
                        active = true;
                        self.status = AgentStatus::Streaming { round: self.current_round };
                        if let Some(r) = self.streaming_reply.as_mut() {
                            r.push_str(&t);
                        }
                        token_count += 1;
                        if token_count >= MAX_TOKENS_PER_FRAME {
                            break;
                        }
                    },
                    Ok(StreamEvent::ToolStart { name, args_summary }) => {
                        active = true;
                        self.status = AgentStatus::CallingTool {
                            round: self.current_round,
                            name: name.clone(),
                        };
                        // Task lifecycle tools are shown in the plan strip — skip them here.
                        if !matches!(name.as_str(), "create_task" | "complete_task") {
                            // Double newline = paragraph break in CommonMark, so each
                            // tool call renders on its own line rather than running together.
                            let line = format!("\n\n⚙  {name}({args_summary})");
                            match self.streaming_reply.as_mut() {
                                Some(r) => r.push_str(&line),
                                None => self.streaming_reply = Some(line),
                            }
                        }
                    },
                    Ok(StreamEvent::ToolDone { name, result_summary }) => {
                        active = true;
                        self.status = AgentStatus::WaitingForResponse { round: self.current_round };
                        if !matches!(name.as_str(), "create_task" | "complete_task") {
                            if let Some(r) = self.streaming_reply.as_mut() {
                                r.push_str(&format!(" → {result_summary}"));
                            }
                        }
                    },
                    Ok(StreamEvent::FileModified { path }) => {
                        active = true;
                        self.pending_reloads.push(path);
                    },
                    Ok(StreamEvent::TaskCreated { title }) => {
                        active = true;
                        self.tasks.push(AgentTask { title, done: false });
                    },
                    Ok(StreamEvent::TaskCompleted { title }) => {
                        active = true;
                        if let Some(t) = self.tasks.iter_mut().find(|t| t.title == title) {
                            t.done = true;
                        }
                    },
                    Ok(StreamEvent::RoundProgress { current, max }) => {
                        active = true;
                        self.current_round = current;
                        self.max_rounds = max;
                        self.status = AgentStatus::WaitingForResponse { round: current };
                    },
                    Ok(StreamEvent::MaxRoundsWarning { current, max, remaining }) => {
                        active = true;
                        let warning = format!(
                            "\n⚠  Agent: {} of {} rounds complete ({} remaining)",
                            current, max, remaining
                        );
                        if let Some(r) = self.streaming_reply.as_mut() {
                            r.push_str(&warning);
                        }
                    },
                    Ok(StreamEvent::AwaitingContinuation) => {
                        active = true;
                        self.awaiting_continuation = true;
                    },
                    Ok(StreamEvent::AskingUser { question, options }) => {
                        active = true;
                        self.asking_user = Some(AskUserState { question, options, selected: 0 });
                    },
                    Ok(StreamEvent::Retrying { attempt, max }) => {
                        active = true;
                        self.status = AgentStatus::Retrying { attempt, max };
                    },
                    Ok(StreamEvent::Usage { prompt_tokens, completion_tokens }) => {
                        self.last_prompt_tokens = prompt_tokens;
                        self.last_completion_tokens = completion_tokens;
                    },
                    Ok(StreamEvent::Done) => {
                        if let Some(text) = self.streaming_reply.take() {
                            if !text.is_empty() {
                                self.messages
                                    .push(ChatMessage { role: Role::Assistant, content: text });
                            }
                        }
                        self.code_block_idx = 0;
                        self.scroll = 0;
                        self.stream_rx = None;
                        self.continuation_tx = None;
                        self.question_tx = None;
                        self.asking_user = None;
                        self.awaiting_continuation = false;
                        self.current_round = 0;
                        self.status = AgentStatus::Idle;
                        break;
                    },
                    Ok(StreamEvent::Error(e)) => {
                        warn!("Copilot Chat stream error: {}", e);
                        self.messages.push(ChatMessage {
                            role: Role::Assistant,
                            content: format!("[Error: {e}]"),
                        });
                        self.last_error = Some(e);
                        self.streaming_reply = None;
                        self.stream_rx = None;
                        self.continuation_tx = None;
                        self.question_tx = None;
                        self.asking_user = None;
                        self.awaiting_continuation = false;
                        self.current_round = 0;
                        self.status = AgentStatus::Idle;
                        break;
                    },
                    Err(_) => break,
                }
            }
        }
        active
    }

    /// Approve continuation when the agent is awaiting user decision.
    pub fn approve_continuation(&mut self) {
        if self.awaiting_continuation {
            if let Some(tx) = &self.continuation_tx {
                let _ = tx.send(true);
                self.awaiting_continuation = false;
                // Update the reply to remove the prompt
                if let Some(r) = self.streaming_reply.as_mut() {
                    if let Some(pos) = r.rfind("\n\n⏸  Maximum rounds reached") {
                        r.truncate(pos);
                        r.push_str("\n\n✓ Continuing...");
                    }
                }
            }
        }
    }

    /// Deny continuation when the agent is awaiting user decision.
    pub fn deny_continuation(&mut self) {
        if self.awaiting_continuation {
            if let Some(tx) = &self.continuation_tx {
                let _ = tx.send(false);
                self.awaiting_continuation = false;
                // Update the reply to remove the prompt
                if let Some(r) = self.streaming_reply.as_mut() {
                    if let Some(pos) = r.rfind("\n\n⏸  Maximum rounds reached") {
                        r.truncate(pos);
                        r.push_str("\n\n✗ Stopped by user");
                    }
                }
            }
        }
    }

    /// Confirm the currently-selected answer in the ask_user dialog.
    pub fn confirm_user_question(&mut self) {
        if let Some(ref state) = self.asking_user.take() {
            if let Some(ref tx) = self.question_tx {
                let answer =
                    state.options.get(state.selected).cloned().unwrap_or_else(|| "Yes".to_string());
                // Echo the choice into the reply so the user sees it in history.
                let echo = format!("\n\n→ **{}**", answer);
                match self.streaming_reply.as_mut() {
                    Some(r) => r.push_str(&echo),
                    None => self.streaming_reply = Some(echo),
                }
                let _ = tx.send(answer);
            }
        }
    }

    /// Abort the running agentic loop immediately.
    ///
    /// Drops the oneshot sender — the agentic task receives the cancellation at
    /// its next `tokio::select!` checkpoint and exits.  Any partial streaming
    /// reply is committed to history so it isn't lost.
    pub fn cancel_stream(&mut self) {
        if self.stream_rx.is_none() {
            return; // nothing running
        }
        // Fire the abort signal.  Dropping the sender is equivalent to sending ().
        self.abort_tx.take();

        // Commit whatever has been streamed so far so the user can read it.
        if let Some(text) = self.streaming_reply.take() {
            if !text.trim().is_empty() {
                let mut content = text;
                content.push_str("\n\n*⏹ Stopped*");
                self.messages.push(ChatMessage { role: Role::Assistant, content });
            }
        }
        // Reset all stream-related state.
        self.stream_rx = None;
        self.continuation_tx = None;
        self.question_tx = None;
        self.asking_user = None;
        self.awaiting_continuation = false;
        self.current_round = 0;
        self.status = AgentStatus::Idle;
    }

    /// Cancel the ask_user dialog (picks the last option, typically "No"/"Cancel").
    pub fn cancel_user_question(&mut self) {
        if let Some(ref state) = self.asking_user.take() {
            if let Some(ref tx) = self.question_tx {
                let answer = state.options.last().cloned().unwrap_or_else(|| "No".to_string());
                let echo = format!("\n\n→ **{}** (cancelled)", answer);
                match self.streaming_reply.as_mut() {
                    Some(r) => r.push_str(&echo),
                    None => self.streaming_reply = Some(echo),
                }
                let _ = tx.send(answer);
            }
        }
    }

    /// Move the selection up or down in the ask_user dialog.
    pub fn move_question_selection(&mut self, delta: i32) {
        if let Some(ref mut state) = self.asking_user {
            let n = state.options.len();
            if n > 0 {
                state.selected = (state.selected as i32 + delta).rem_euclid(n as i32) as usize;
            }
        }
    }

    pub fn scroll_up(&mut self) {
        self.scroll += 3;
    }
    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_sub(3);
    }
    #[allow(dead_code)]
    pub fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
    }

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
                    if !current.is_empty() {
                        blocks.push(current.join("\n"));
                    }
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
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .and_then(|m| Self::extract_code_blocks(&m.content).into_iter().next())
    }

    pub fn has_code_to_apply(&self) -> bool {
        self.get_code_to_apply().is_some()
    }

    /// Returns (path_hint, code_content) for the first code block.
    /// path_hint resolution order:
    ///   1. Fence info string tokens containing '/' or '\' (not http)
    ///   2. Backtick-quoted tokens in up to 3 prose lines before the fence
    pub fn extract_first_code_block_with_path(
        text: &str,
    ) -> Option<(Option<std::path::PathBuf>, String)> {
        let lines: Vec<&str> = text.lines().collect();
        let mut in_block = false;
        let mut current: Vec<&str> = Vec::new();
        let mut path_hint: Option<std::path::PathBuf> = None;

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                if in_block {
                    while current.last().map(|l: &&str| l.trim().is_empty()).unwrap_or(false) {
                        current.pop();
                    }
                    if !current.is_empty() {
                        return Some((path_hint, current.join("\n")));
                    }
                    return None;
                } else {
                    in_block = true;
                    let info = trimmed.trim_start_matches('`').trim();
                    // Check fence info string for a path-like token
                    path_hint = info
                        .split_whitespace()
                        .find(|t| (t.contains('/') || t.contains('\\')) && !t.starts_with("http"))
                        .map(std::path::PathBuf::from);
                    // Fall back: scan up to 3 preceding prose lines for `backtick/path`
                    if path_hint.is_none() {
                        'outer: for &prev in lines[..idx].iter().rev().take(3) {
                            let parts: Vec<&str> = prev.split('`').collect();
                            for chunk in parts.iter().skip(1).step_by(2) {
                                if (chunk.contains('/') || chunk.contains('\\'))
                                    && !chunk.starts_with("http")
                                    && !chunk.is_empty()
                                {
                                    path_hint = Some(std::path::PathBuf::from(chunk));
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            } else if in_block {
                current.push(line);
            }
        }
        None
    }

    pub fn get_apply_candidate(&self) -> Option<(Option<std::path::PathBuf>, String)> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .and_then(|m| Self::extract_first_code_block_with_path(&m.content))
    }

    /// Return the full text of the most recent assistant message, if any.
    pub fn last_assistant_reply(&self) -> Option<String> {
        self.messages.iter().rev().find(|m| m.role == Role::Assistant).map(|m| m.content.clone())
    }

    async fn ensure_token(&mut self) -> Result<String> {
        if let Some(ref t) = self.token {
            if !t.is_expired() {
                info!("Using cached token, expires at: {}", t.expires_at);
                return Ok(t.token.clone());
            } else {
                warn!("Cached token expired, refreshing...");
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
// Agentic loop (runs in a background tokio task)
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn agentic_loop(
    api_token: String,
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
        let response = tokio::select! {
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
            ) => match res {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(format!("{e}")));
                    return;
                },
            }
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
                                // Log the actual model the API used on the first chunk (may differ from what we requested).
                                if text_buf.is_empty() && partial_tools.is_empty() {
                                    if let Some(actual) = val.get("model").and_then(|v| v.as_str())
                                    {
                                        info!("[stream] API routed request to model={actual:?}  (requested={model_id:?})");
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
                                    if p > 0 || c > 0 {
                                        let _ = tx.send(StreamEvent::Usage {
                                            prompt_tokens: p,
                                            completion_tokens: c,
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
// Project tree builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a compact, indented file tree of `root` to `max_depth` levels.
/// Hidden files and noisy directories (target, node_modules, …) are skipped.
fn build_project_tree(root: &std::path::Path, max_depth: usize) -> String {
    let mut out = String::new();
    tree_recursive(root, root, 0, max_depth, &mut out);
    out
}

#[allow(clippy::only_used_in_recursion)]
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
        if matches!(name.as_str(), "target" | "node_modules" | "dist" | "build" | ".git") {
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
                let body = response.text().await.unwrap_or_default();
                // 4xx errors (except 429 rate-limit) are permanent — don't retry.
                if status.is_client_error() && status.as_u16() != 429 {
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

// ─────────────────────────────────────────────────────────────────────────────
// Model discovery
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch chat-capable models from the Copilot `/models` endpoint.
/// Returns `ModelVersion` pairs (id + display name) sorted with gpt-4o first, then alphabetically by id.
async fn fetch_models(api_token: &str) -> Result<Vec<ModelVersion>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let mut retry_attempts = 0;
    let max_retries = 5;
    let mut delay = tokio::time::Duration::from_secs(1);

    loop {
        let resp = client
            .get("https://api.githubcopilot.com/models")
            .header("Authorization", format!("Bearer {api_token}"))
            .header("User-Agent", "forgiven/0.1.0")
            .header("Copilot-Integration-Id", "vscode-chat")
            .header("editor-version", "forgiven/0.1.0")
            .header("editor-plugin-version", "forgiven-copilot/0.1.0")
            .send()
            .await;

        match resp {
            Ok(response) if response.status().is_success() => {
                let body: serde_json::Value =
                    response.json().await.context("/models response is not JSON")?;

                // Log every model entry from the raw response so mismatches are diagnosable.
                if let Some(arr) = body.get("data").and_then(|d| d.as_array()) {
                    for v in arr {
                        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
                        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                        let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
                        let cap_type =
                            v.pointer("/capabilities/type").and_then(|x| x.as_str()).unwrap_or("?");
                        info!("[models] id={id:?} name={name:?} version={version:?} cap_type={cap_type:?}");
                    }
                }

                let mut models: Vec<ModelVersion> = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                let id = v.get("id")?.as_str()?.to_string();
                                // Filter models that don't support /chat/completions.
                                // Embedding, TTS, image, and codex/code-completion models all fail with
                                // unsupported_api_for_model when sent to the chat endpoint.
                                if id.contains("embed")
                                    || id.contains("whisper")
                                    || id.contains("tts")
                                    || id.contains("dall")
                                    || id.contains("codex")
                                {
                                    return None;
                                }
                                // Also filter by capabilities.type if present: only keep "chat" models.
                                if let Some(cap_type) =
                                    v.pointer("/capabilities/type").and_then(|x| x.as_str())
                                {
                                    if cap_type != "chat" {
                                        return None;
                                    }
                                }
                                // `version` is the pinned build string; fall back to id if absent.
                                let version = v
                                    .get("version")
                                    .and_then(|x| x.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(&id)
                                    .to_string();
                                // Use the human-readable `name` for display; fall back to the id.
                                let name = v
                                    .get("name")
                                    .and_then(|x| x.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(&id)
                                    .to_string();
                                Some(ModelVersion { id, version, name })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                models.sort_by(|a, b| {
                    let a_pref = if a.id == "gpt-4o" { 0 } else { 1 };
                    let b_pref = if b.id == "gpt-4o" { 0 } else { 1 };
                    a_pref.cmp(&b_pref).then(a.id.cmp(&b.id))
                });
                // The API sometimes returns duplicate IDs; deduplicate after sorting.
                models.dedup_by(|a, b| a.id == b.id);

                info!(
                    "Filtered+sorted model list: {:?}",
                    models.iter().map(|m| format!("{} ({})", m.name, m.id)).collect::<Vec<_>>()
                );
                return Ok(models);
            },
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                // 4xx errors (except 429 rate-limit) are permanent — don't retry.
                if status.is_client_error() && status.as_u16() != 429 {
                    return Err(anyhow::anyhow!("/models API error ({status}): {body}"));
                }
                warn!("Retrying due to API error ({status}): {body}");
            },
            Err(e) => {
                warn!("Retrying due to network error: {e}");
            },
        }

        retry_attempts += 1;
        if retry_attempts >= max_retries {
            return Err(anyhow::anyhow!("Max retries reached for Copilot /models API"));
        }

        tokio::time::sleep(delay).await;
        delay *= 2; // Exponential backoff
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Auth
// ─────────────────────────────────────────────────────────────────────────────

fn load_oauth_token() -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = format!("{home}/.config/github-copilot/apps.json");
    let raw = std::fs::read_to_string(&path).with_context(|| format!("Cannot read {path}"))?;
    let val: serde_json::Value =
        serde_json::from_str(&raw).context("apps.json is not valid JSON")?;
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

/// Load the OAuth token and exchange it for a Copilot API token.
/// Convenience wrapper for callers that don't have access to an `AgentPanel`.
pub async fn acquire_copilot_token() -> Result<String> {
    let oauth = load_oauth_token()?;
    let api_token = exchange_token(&oauth).await?;
    Ok(api_token.token)
}

/// Single non-streaming Copilot completion — for short one-shot tasks such as
/// generating a commit message. Returns the assistant reply as a plain `String`.
pub async fn one_shot_complete(
    api_token: &str,
    model_id: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model_id,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user",   "content": user   }
        ],
        "stream": false,
        "temperature": 0.3,
        "max_tokens": max_tokens
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let resp = client
        .post("https://api.githubcopilot.com/chat/completions")
        .header("Authorization", format!("Bearer {api_token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Copilot-Integration-Id", "vscode-chat")
        .header("editor-version", "forgiven/0.1.0")
        .header("editor-plugin-version", "forgiven-copilot/0.1.0")
        .header("openai-intent", "conversation-panel")
        .header("User-Agent", "forgiven/0.1.0")
        .json(&body)
        .send()
        .await
        .context("one_shot_complete: request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Copilot API error ({status}): {text}"));
    }

    let val: serde_json::Value =
        resp.json().await.context("one_shot_complete: response not JSON")?;
    let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
    Ok(content)
}

async fn exchange_token(oauth_token: &str) -> Result<CopilotApiToken> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let mut retry_attempts = 0;
    let max_retries = 3;
    let mut delay = tokio::time::Duration::from_secs(1);

    let (status, body_text) = loop {
        match client
            .get("https://api.github.com/copilot_internal/v2/token")
            .header("Authorization", format!("token {oauth_token}"))
            .header("User-Agent", "forgiven/0.1.0")
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(resp) => {
                let s = resp.status();
                let b = resp.text().await.unwrap_or_default();
                debug!("Token exchange response ({s}): {b}");
                // Only retry on server errors or rate limits; fail immediately on 4xx auth errors.
                if s.is_success() || (s.is_client_error() && s.as_u16() != 429) {
                    break (s, b);
                }
                warn!("Token exchange retrying due to server error ({s})");
            },
            Err(e) => {
                warn!("Token exchange retrying due to network error: {e}");
            },
        }
        retry_attempts += 1;
        if retry_attempts >= max_retries {
            return Err(anyhow::anyhow!("Token exchange failed after {max_retries} attempts"));
        }
        tokio::time::sleep(delay).await;
        delay *= 2;
    };

    if !status.is_success() {
        return Err(anyhow::anyhow!("Token exchange failed ({status}): {body_text}"));
    }

    let val: serde_json::Value = serde_json::from_str(&body_text)
        .with_context(|| format!("Token response is not JSON: {body_text}"))?;
    info!("Token response keys: {:?}", val.as_object().map(|o| o.keys().collect::<Vec<_>>()));

    let token_str = val
        .get("token")
        .and_then(|v| v.as_str())
        .with_context(|| format!("No 'token' field in response: {body_text}"))?
        .to_string();
    let expires_at_str = val.get("expires_at").and_then(|v| v.as_str()).map(|s| s.to_string());
    debug!("Copilot API token acquired (expires_at={:?})", expires_at_str);

    let tr = TokenResponse { token: token_str, expires_at: expires_at_str };
    let expires_at = tr.expires_at.as_deref().and_then(chrono_unix_from_iso).unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() + 1800)
            .unwrap_or(1800)
    });

    Ok(CopilotApiToken { token: tr.token, expires_at })
}

fn chrono_unix_from_iso(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let s = if let Some(pos) = s.find('+') { &s[..pos] } else { s };
    let s = if let Some(pos) = s.rfind('-') {
        if pos > 10 {
            &s[..pos]
        } else {
            s
        }
    } else {
        s
    };
    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time: Vec<u64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();
    if date.len() < 3 || time.len() < 3 {
        return None;
    }
    let y = date[0].saturating_sub(1970);
    let days = y * 365 + y / 4 + days_before_month(date[1], date[0]) + date[2] - 1;
    Some(days * 86400 + time[0] * 3600 + time[1] * 60 + time[2])
}

fn days_before_month(month: u64, year: u64) -> u64 {
    let dim = [0u64, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    {
        1
    } else {
        0
    };
    let mut total = 0;
    for m in 1..month.min(13) {
        total += dim[m as usize];
        if m == 2 {
            total += leap;
        }
    }
    total
}
