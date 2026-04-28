use anyhow::{Context, Result};
use tracing::{info, warn};

use super::auth::{exchange_token, load_oauth_token};
use super::models::fetch_models_for_provider;
use super::provider::{ProviderConfig, ProviderKind};
use super::{
    AgentPanel, AgentStatus, ChatMessage, ClipboardImage, ModelVersion, Role, SlashMenuState,
};

// ─────────────────────────────────────────────────────────────────────────────
// AgentPanel impl
// ─────────────────────────────────────────────────────────────────────────────

impl AgentPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            focused: false,
            provider: ProviderKind::Copilot,
            provider_config: ProviderConfig::default(),
            conversation: super::Conversation::new(),
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
            max_rounds: 10,
            awaiting_continuation: false,
            status: AgentStatus::Idle,
            last_error: None,
            code_block_idx: 0,
            mermaid_block_idx: 0,
            pasted_blocks: Vec::new(),
            image_blocks: Vec::new(),
            mcp_manager: None,
            spec_framework: None,
            abort_tx: None,
            last_submit_ctx: None,
            last_breakdown: None,
            last_submit_model: String::new(),
            question_tx: None,
            asking_user: None,
            asking_user_input: None,
            slash_menu: None,
            file_blocks: Vec::new(),
            at_picker: None,
            janitor_compressing: false,
            usage_received_this_round: false,
            context_near_limit_warned: false,
            session_total_100k_warned: false,
            intent_translator_enabled: false,
            intent_translator_provider: "ollama".to_string(),
            intent_translator_ollama_model: "qwen2.5-coder:7b".to_string(),
            intent_translator_model: "claude-haiku-4-5-20251001".to_string(),
            intent_translator_min_chars: 40,
            intent_translator_timeout_ms: 10000,
            intent_translator_skip_patterns: Vec::new(),
            cached_project_tree: None,
            cached_structural_map: None,
            session_snapshots: std::collections::HashMap::new(),
            session_created_files: Vec::new(),
            investigation_rx: None,
            investigation_buf: String::new(),
            round_hint: None,
            pending_tool_calls: Vec::new(),
            session_read_file_count: 0,
            session_symbol_count: 0,
            session_outline_count: 0,
            codified_context_enabled: false,
            codified_context: None,
            codified_context_tip_shown: false,
            codified_context_constitution_max_tokens: 500,
            codified_context_max_specialists: 2,
            codified_context_knowledge_max_bytes: 8192,
            copilot_quota: None,
            copilot_api_base: "https://api.githubcopilot.com".to_string(),
        }
    }

    /// The model `id` to send in API requests (e.g. "claude-sonnet-4", "gpt-5.1").
    /// The Copilot API matches on this `id` field for routing.
    /// Falls back to "claude-sonnet-4" before the models list has been fetched.
    pub fn selected_model_id(&self) -> &str {
        if self.available_models.is_empty() {
            return "claude-sonnet-4";
        }
        &self.available_models[self.selected_model.min(self.available_models.len() - 1)].id
    }

    /// Like `selected_model_id` but uses `fallback` when the models list hasn't loaded yet.
    /// Use this anywhere the config's preferred model should be honoured before the list arrives.
    pub fn selected_model_id_with_fallback<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.available_models.is_empty() {
            return fallback;
        }
        &self.available_models[self.selected_model.min(self.available_models.len() - 1)].id
    }

    /// The human-readable display name for the selected model (shown in the UI).
    pub fn selected_model_display(&self) -> &str {
        if self.available_models.is_empty() {
            return "Claude Sonnet 4";
        }
        &self.available_models[self.selected_model.min(self.available_models.len() - 1)].name
    }

    /// Returns the context-window size (in tokens) for the selected model.
    /// Uses the value reported by the Copilot `/models` API; falls back to 128k.
    pub fn context_window_size(&self) -> u32 {
        if self.available_models.is_empty() {
            return 128_000;
        }
        self.available_models[self.selected_model.min(self.available_models.len() - 1)]
            .context_window
    }

    /// Advance to the next model in the list (wraps around).
    pub fn cycle_model(&mut self) {
        if !self.available_models.is_empty() {
            self.selected_model = (self.selected_model + 1) % self.available_models.len();
        }
    }

    /// Human-readable name placed after the AI emoji in message headers.
    ///
    /// - Copilot: `"Copilot"`
    /// - Ollama: model base name without the tag (e.g. `"qwen2.5-coder"`)
    pub fn ai_label_name(&self) -> String {
        match self.provider {
            ProviderKind::Copilot => "Copilot".to_string(),
            ProviderKind::Ollama => {
                // "qwen2.5-coder:14b" → "qwen2.5-coder"
                self.selected_model_id().split(':').next().unwrap_or("Ollama").to_string()
            },
            ProviderKind::Anthropic => "Claude".to_string(),
            ProviderKind::OpenAi => "GPT".to_string(),
            ProviderKind::Gemini => "Gemini".to_string(),
            ProviderKind::OpenRouter => {
                // "anthropic/claude-sonnet-4-5" → "claude-sonnet-4-5"
                self.selected_model_id().split('/').next_back().unwrap_or("OpenRouter").to_string()
            },
        }
    }

    /// Ensure the model list is populated.  Fetches from the active provider's
    /// model endpoint if not yet loaded.  Safe to call multiple times — no-op
    /// after first load.
    pub async fn ensure_models(&mut self, preferred_model: &str) -> Result<()> {
        if !self.available_models.is_empty() {
            return Ok(());
        }
        let api_token = self.ensure_token().await?;
        let provider = self.provider.clone();
        let config = self.provider_config.clone();
        let copilot_api_base = self.copilot_api_base.clone();
        match fetch_models_for_provider(&provider, &config, &api_token, &copilot_api_base).await {
            Ok(models) if !models.is_empty() => self.set_models(models, preferred_model),
            Ok(_) => warn!("[{}] model list was empty", provider.display_name()),
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Refresh the model list from the active provider, preserving the current
    /// selection if possible.  Use this to pick up newly pulled Ollama models
    /// or newly released Copilot models without restarting.
    pub async fn refresh_models(&mut self, preferred_model: &str) -> Result<()> {
        let current_id = if !self.available_models.is_empty() {
            Some(self.available_models[self.selected_model].id.clone())
        } else {
            None
        };
        let preferred = current_id.as_deref().unwrap_or(preferred_model);

        let api_token = self.ensure_token().await?;
        let provider = self.provider.clone();
        let config = self.provider_config.clone();
        let copilot_api_base = self.copilot_api_base.clone();
        match fetch_models_for_provider(&provider, &config, &api_token, &copilot_api_base).await {
            Ok(models) if !models.is_empty() => {
                self.set_models(models, preferred);
                info!(
                    "Refreshed {} model list, selected: {} ({})",
                    provider.display_name(),
                    self.selected_model_display(),
                    self.selected_model_id()
                );
            },
            Ok(_) => warn!("[{}] model list was empty on refresh", provider.display_name()),
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Set the available models and select the preferred one (or fallback).
    /// Matches `preferred_model` against `id` first, then `version` (so configs that stored
    /// a versioned ID like "gpt-4o-2024-11-20" still resolve correctly).
    pub(super) fn set_models(&mut self, models: Vec<ModelVersion>, preferred_model: &str) {
        // NOTE: "auto" was previously prepended here as a synthetic Copilot entry under
        // the assumption that sending model:"auto" triggers server-side routing like VS Code.
        // Testing confirmed the Copilot API returns 400 model_not_supported for that value —
        // the routing is a VS Code-side abstraction, not an API feature.
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
            found.or_else(|| models.iter().position(|m| m.id == "claude-sonnet-4")).unwrap_or(0);
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
        self.conversation.input_char(ch);
    }

    pub fn input_backspace(&mut self) {
        if self.conversation.input_cursor == 0 {
            // No typed text to delete — pop the last paste/image/file block instead.
            if !self.pasted_blocks.is_empty() {
                self.pasted_blocks.pop();
            } else if !self.image_blocks.is_empty() {
                self.image_blocks.pop();
            } else if !self.file_blocks.is_empty() {
                self.file_blocks.pop();
            }
            return;
        }
        let prev = self.conversation.input[..self.conversation.input_cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.conversation.input.remove(prev);
        self.conversation.input_cursor = prev;
    }

    pub fn input_newline(&mut self) {
        self.conversation.input_newline();
    }

    pub fn cursor_left(&mut self) {
        self.conversation.cursor_left();
    }

    pub fn cursor_right(&mut self) {
        self.conversation.cursor_right();
    }

    pub fn history_up(&mut self) {
        self.conversation.history_up();
    }

    pub fn history_down(&mut self) {
        self.conversation.history_down();
    }

    /// Returns true when there is anything to submit: typed text, pasted blocks,
    /// file attachments, or images.  Used in the submit() early-return guard and
    /// in the janitor Done handler to decide whether an auto-resubmit is needed.
    pub fn has_pending_content(&self) -> bool {
        !self.conversation.input.trim().is_empty()
            || !self.pasted_blocks.is_empty()
            || !self.file_blocks.is_empty()
            || !self.image_blocks.is_empty()
    }

    /// Discard all pending user input (typed text, pastes, file attachments, images).
    /// Does NOT clear conversation history — use `new_conversation()` for that.
    pub fn clear_input(&mut self) {
        self.conversation.input.clear();
        self.conversation.input_cursor = 0;
        self.conversation.history_idx = None;
        self.conversation.input_saved.clear();
        self.pasted_blocks.clear();
        self.image_blocks.clear();
        self.file_blocks.clear();
    }

    /// Attempt to read an image from the system clipboard.
    /// Returns `Ok(Some(img))` if an image was captured, `Ok(None)` if no image
    /// was available, and `Err` only on encoding failure.
    pub fn try_paste_image() -> Result<Option<ClipboardImage>> {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(cb) => cb,
            Err(_) => return Ok(None),
        };
        let img_data = match clipboard.get_image() {
            Ok(data) => data,
            Err(_) => return Ok(None),
        };

        let width = img_data.width as u32;
        let height = img_data.height as u32;

        // Convert RGBA bytes to PNG.
        let rgba = image::RgbaImage::from_raw(width, height, img_data.bytes.into_owned())
            .context("clipboard image has invalid dimensions")?;

        let mut png_bytes: Vec<u8> = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
        use image::ImageEncoder;
        encoder
            .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
            .context("PNG encoding failed")?;

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let data_uri = format!("data:image/png;base64,{b64}");

        // Reject images that would produce an excessively large payload.
        const MAX_DATA_URI_BYTES: usize = 20 * 1024 * 1024;
        if data_uri.len() > MAX_DATA_URI_BYTES {
            anyhow::bail!(
                "Image too large ({:.1} MB, max {:.1} MB)",
                data_uri.len() as f64 / 1_048_576.0,
                MAX_DATA_URI_BYTES as f64 / 1_048_576.0,
            );
        }

        Ok(Some(ClipboardImage { width, height, data_uri }))
    }

    /// Built-in action slash commands that are always available regardless of spec_framework.
    const BUILTIN_SLASH_COMMANDS: &'static [(&'static str, &'static str)] = &[
        ("compress", "Summarise and compress chat history (free a slot in the context window)"),
        ("translate", "Toggle intent translator for this session"),
    ];

    /// Recompute the slash-command dropdown based on the current input.
    /// Call this whenever `self.conversation.input` changes in Agent mode.
    pub fn update_slash_menu(&mut self) {
        // Only active when the input starts with '/'
        if !self.conversation.input.starts_with('/') {
            self.slash_menu = None;
            return;
        }
        let prefix = self.conversation.input.trim_start_matches('/');
        // Strip any trailing context (e.g. "/compress extra" → prefix is "compress extra",
        // but we match on the command part only).
        let cmd_prefix = prefix.split_whitespace().next().unwrap_or(prefix);

        // Built-in action commands (always available).
        let mut items: Vec<String> = Self::BUILTIN_SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(cmd_prefix))
            .map(|(cmd, _)| cmd.to_string())
            .collect();

        // Framework template commands (when a framework is loaded).
        if let Some(ref fw) = self.spec_framework {
            let fw_items: Vec<String> = fw
                .commands()
                .into_iter()
                .filter(|cmd| cmd.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            items.extend(fw_items);
        }

        if items.is_empty() {
            self.slash_menu = None;
            return;
        }

        let describe = |cmd: &str| -> Option<String> {
            // Check built-ins first, then fall back to framework descriptions.
            if let Some((_, desc)) = Self::BUILTIN_SLASH_COMMANDS.iter().find(|(c, _)| *c == cmd) {
                return Some(desc.to_string());
            }
            self.spec_framework.as_ref().and_then(|fw| fw.describe(cmd)).map(str::to_string)
        };

        match self.slash_menu.as_mut() {
            Some(menu) => {
                let prev = menu.selected;
                menu.items = items;
                menu.selected = prev.min(menu.items.len().saturating_sub(1));
                menu.description = describe(menu.items[menu.selected].as_str());
            },
            None => {
                let description = items.first().and_then(|cmd| describe(cmd.as_str()));
                self.slash_menu = Some(SlashMenuState { items, selected: 0, description });
            },
        }
    }

    /// Move the slash-menu selection by `delta` (+1 = down, -1 = up).
    pub fn move_slash_selection(&mut self, delta: i32) {
        if let Some(ref mut menu) = self.slash_menu {
            let n = menu.items.len();
            if n > 0 {
                menu.selected = (menu.selected as i32 + delta).rem_euclid(n as i32) as usize;
                let cmd = menu.items[menu.selected].clone();
                menu.description = Self::BUILTIN_SLASH_COMMANDS
                    .iter()
                    .find(|(c, _)| *c == cmd.as_str())
                    .map(|(_, d)| d.to_string())
                    .or_else(|| {
                        self.spec_framework
                            .as_ref()
                            .and_then(|fw| fw.describe(cmd.as_str()))
                            .map(str::to_string)
                    });
            }
        }
    }

    /// Complete the selected slash-command into `self.conversation.input`.
    /// Any text after the command name (context) is preserved.
    pub fn complete_slash_selection(&mut self) {
        let Some(ref menu) = self.slash_menu else { return };
        let cmd = match menu.items.get(menu.selected) {
            Some(c) => c.clone(),
            None => return,
        };
        // Preserve any context text typed after the slash-command.
        let rest = self
            .conversation
            .input
            .trim_start_matches('/')
            .split_once(char::is_whitespace)
            .map(|(_, r)| format!(" {}", r.trim_start()))
            .unwrap_or_default();
        self.conversation.input = format!("/{cmd}{rest}");
        self.slash_menu = None;
    }

    /// Clear conversation history and insert a visual divider showing the new model.
    /// Called when the user switches models via Ctrl+T so the new model receives a
    /// clean context — not the prior conversation from a different model.
    pub fn new_conversation(&mut self, model_name: &str) {
        // Write a session-end efficiency record for the conversation that is ending.
        if self.conversation.session_rounds > 0 {
            let files_changed = self.session_snapshots.len() + self.session_created_files.len();
            super::append_session_end_record(
                &self.last_submit_model,
                self.conversation.total_session_prompt_tokens,
                self.conversation.total_session_completion_tokens,
                self.conversation.session_rounds,
                files_changed,
                "new_conversation",
            );
        }
        // Compute adaptive round-limit hint for the incoming session.
        self.round_hint = super::suggest_max_rounds(self.selected_model_id());

        self.conversation.messages.clear();
        self.conversation.archived_messages.clear();
        self.tasks.clear();
        self.streaming_reply = None;
        self.conversation.total_session_prompt_tokens = 0;
        self.conversation.total_session_completion_tokens = 0;
        self.conversation.session_rounds = 0;
        self.conversation.session_start_secs = 0;
        self.cached_project_tree = None;
        self.cached_structural_map = None;
        self.session_snapshots.clear();
        self.session_created_files.clear();
        self.context_near_limit_warned = false;
        self.session_total_100k_warned = false;
        self.pending_tool_calls.clear();
        self.session_read_file_count = 0;
        self.session_symbol_count = 0;
        self.session_outline_count = 0;
        self.codified_context_tip_shown = false;
        self.conversation.messages.push(ChatMessage {
            role: Role::System,
            content: format!("── New conversation · {model_name} ──"),
            images: vec![],
        });
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
                self.conversation.messages.push(ChatMessage {
                    role: Role::Assistant,
                    content,
                    images: vec![],
                });
            }
        }
        // Reset all stream-related state.
        self.stream_rx = None;
        self.continuation_tx = None;
        self.question_tx = None;
        self.asking_user = None;
        self.asking_user_input = None;
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

    /// Insert a character at the current cursor position in the ask_user_input field.
    pub fn type_char_to_input(&mut self, c: char) {
        if let Some(ref mut state) = self.asking_user_input {
            state.input.insert(state.cursor, c);
            state.cursor += c.len_utf8();
        }
    }

    /// Delete the character immediately before the cursor in the ask_user_input field.
    pub fn backspace_input(&mut self) {
        if let Some(ref mut state) = self.asking_user_input {
            if state.cursor > 0 {
                // Find the start of the previous character (handles multi-byte).
                let prev =
                    state.input[..state.cursor].char_indices().last().map(|(i, _)| i).unwrap_or(0);
                state.input.remove(prev);
                state.cursor = prev;
            }
        }
    }

    /// Move the cursor left or right in the ask_user_input field.
    pub fn move_input_cursor(&mut self, delta: i32) {
        if let Some(ref mut state) = self.asking_user_input {
            if delta < 0 {
                // Move left: find previous char boundary.
                if state.cursor > 0 {
                    state.cursor = state.input[..state.cursor]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            } else {
                // Move right: advance by next char length.
                if state.cursor < state.input.len() {
                    let ch = state.input[state.cursor..].chars().next().unwrap();
                    state.cursor += ch.len_utf8();
                }
            }
        }
    }

    /// Confirm the typed text and send it back to the agentic loop.
    pub fn confirm_user_input(&mut self) {
        if let Some(ref state) = self.asking_user_input.take() {
            if let Some(ref tx) = self.question_tx {
                let answer = state.input.trim().to_string();
                let echo =
                    format!("\n\n→ **{}**", if answer.is_empty() { "(empty)" } else { &answer });
                match self.streaming_reply.as_mut() {
                    Some(r) => r.push_str(&echo),
                    None => self.streaming_reply = Some(echo),
                }
                let _ = tx.send(answer);
            }
        }
    }

    /// Cancel the ask_user_input dialog; sends an empty string to the agentic loop.
    pub fn cancel_user_input(&mut self) {
        if self.asking_user_input.take().is_some() {
            if let Some(ref tx) = self.question_tx {
                let echo = "\n\n→ *(cancelled)*".to_string();
                match self.streaming_reply.as_mut() {
                    Some(r) => r.push_str(&echo),
                    None => self.streaming_reply = Some(echo),
                }
                let _ = tx.send(String::new());
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

    /// Extract only fenced blocks whose language tag is `mermaid`.
    /// Returns the raw diagram source (without the fence lines).
    pub fn extract_mermaid_blocks(text: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut in_mermaid = false;
        let mut current: Vec<&str> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                if in_mermaid {
                    while current.last().map(|l: &&str| l.trim().is_empty()).unwrap_or(false) {
                        current.pop();
                    }
                    if !current.is_empty() {
                        blocks.push(current.join("\n"));
                    }
                    current.clear();
                    in_mermaid = false;
                } else {
                    let lang = trimmed.trim_start_matches('`').trim();
                    if lang.eq_ignore_ascii_case("mermaid") {
                        in_mermaid = true;
                    }
                }
            } else if in_mermaid {
                current.push(line);
            }
        }
        blocks
    }

    /// Return the full text of the most recent assistant message, if any.
    pub fn last_assistant_reply(&self) -> Option<String> {
        self.conversation
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .map(|m| m.content.clone())
    }

    pub(super) async fn ensure_token(&mut self) -> Result<String> {
        match self.provider {
            ProviderKind::Ollama => {
                // No authentication needed.
                Ok(String::new())
            },
            ProviderKind::Anthropic
            | ProviderKind::OpenAi
            | ProviderKind::Gemini
            | ProviderKind::OpenRouter => Ok(self.provider_config.api_key.clone()),
            ProviderKind::Copilot => {
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
                if let Some(ref url) = api_token.business_api_url {
                    self.copilot_api_base = url.clone();
                }
                self.token = Some(api_token);
                self.copilot_quota = crate::agent::auth::fetch_copilot_quota(&oauth).await;
                Ok(tok)
            },
        }
    }
}
