use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use super::agentic_loop::agentic_loop;
use super::auth::{exchange_token, load_oauth_token};
use super::models::fetch_models_for_provider;
use super::provider::{ProviderKind, ProviderSettings};
use super::{
    append_session_metric, AgentPanel, AgentStatus, AgentTask, AskUserInputState, AskUserState,
    ChatMessage, ClipboardImage, ModelVersion, Role, SlashMenuState, StreamEvent, SubmitCtx,
};

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
    // Pre-build indent once per level (depth ≤ max_depth, typically ≤ 2).
    let indent = "  ".repeat(depth);
    for entry in items {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name.starts_with('.') {
            continue; // skip hidden
        }
        if matches!(name.as_ref(), "target" | "node_modules" | "dist" | "build" | ".git") {
            continue;
        }
        if entry.path().is_dir() {
            // Push directly onto `out` — avoids the intermediate String that
            // `format!("{indent}{name}/\n")` would allocate.
            out.push_str(&indent);
            out.push_str(&name);
            out.push_str("/\n");
            tree_recursive(root, &entry.path(), depth + 1, max_depth, out);
        } else {
            out.push_str(&indent);
            out.push_str(&name);
            out.push('\n');
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Structural project map builder (Phase 2.1 — Aider pattern)
// ─────────────────────────────────────────────────────────────────────────────

/// Build a compact structural map of `src/` files: one line per file listing
/// up to `MAX_NAMES` top-level symbol names.  Gives the model symbol-level
/// orientation at ≈200–400 tokens instead of 300–500 for the filename tree,
/// saving 1–2 `read_file` discovery round-trips per session.
fn build_structural_map(root: &std::path::Path) -> String {
    use super::tools::extract_symbols;
    const MAX_NAMES: usize = 8;

    let src_root = root.join("src");
    let mut lines =
        vec!["Structural map (src/ — call get_file_outline for full details):".to_string()];

    // Collect all .rs files under src/ (depth-unlimited but src/ is small).
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    collect_rs_files_recursive(&src_root, &mut paths);
    paths.sort();

    for path in paths {
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        let symbols = extract_symbols(&src);
        if symbols.is_empty() {
            continue;
        }
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        let shown = &names[..names.len().min(MAX_NAMES)];
        let extra = names.len().saturating_sub(MAX_NAMES);
        let suffix = if extra > 0 { format!(" … +{extra}") } else { String::new() };
        lines.push(format!("  {} — {}{}", rel.display(), shown.join(", "), suffix));
    }
    lines.join("\n")
}

fn collect_rs_files_recursive(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files_recursive(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AgentPanel impl
// ─────────────────────────────────────────────────────────────────────────────

impl AgentPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            focused: false,
            provider: ProviderKind::Copilot,
            ollama_base_url: "http://localhost:11434".to_string(),
            ollama_context_length: None,
            ollama_tool_calls: false,
            ollama_planning_tools: false,
            api_key: String::new(),
            openai_base_url: "https://api.openai.com/v1".to_string(),
            openrouter_site_url: String::new(),
            openrouter_app_name: String::new(),
            messages: Vec::new(),
            archived_messages: Vec::new(),
            input: String::new(),
            input_history: Vec::new(),
            history_idx: None,
            input_saved: String::new(),
            input_cursor: 0,
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
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
            last_cached_tokens: 0,
            total_session_prompt_tokens: 0,
            total_session_completion_tokens: 0,
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
            session_rounds: 0,
            session_start_secs: 0,
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
        let ollama_base_url = self.ollama_base_url.clone();
        let ollama_ctx = self.ollama_context_length;
        let openai_base_url = self.openai_base_url.clone();
        match fetch_models_for_provider(
            &provider,
            &api_token,
            &ollama_base_url,
            ollama_ctx,
            &openai_base_url,
        )
        .await
        {
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
        let ollama_base_url = self.ollama_base_url.clone();
        let ollama_ctx = self.ollama_context_length;
        let openai_base_url = self.openai_base_url.clone();
        match fetch_models_for_provider(
            &provider,
            &api_token,
            &ollama_base_url,
            ollama_ctx,
            &openai_base_url,
        )
        .await
        {
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
    fn set_models(&mut self, models: Vec<ModelVersion>, preferred_model: &str) {
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
        self.input.insert(self.input_cursor, ch);
        self.input_cursor += ch.len_utf8();
    }

    pub fn input_backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let prev =
            self.input[..self.input_cursor].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
        self.input.remove(prev);
        self.input_cursor = prev;
    }

    pub fn input_newline(&mut self) {
        self.input.insert(self.input_cursor, '\n');
        self.input_cursor += 1;
    }

    pub fn cursor_left(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.input_cursor =
            self.input[..self.input_cursor].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
    }

    pub fn cursor_right(&mut self) {
        if self.input_cursor >= self.input.len() {
            return;
        }
        let ch = self.input[self.input_cursor..].chars().next().unwrap();
        self.input_cursor += ch.len_utf8();
    }

    pub fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        match self.history_idx {
            None => {
                self.input_saved = self.input.clone();
                let idx = self.input_history.len() - 1;
                self.history_idx = Some(idx);
                self.input = self.input_history[idx].clone();
            },
            Some(0) => {},
            Some(i) => {
                self.history_idx = Some(i - 1);
                self.input = self.input_history[i - 1].clone();
            },
        }
        self.input_cursor = self.input.len();
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            None => {},
            Some(i) if i + 1 >= self.input_history.len() => {
                self.history_idx = None;
                self.input = std::mem::take(&mut self.input_saved);
                self.input_cursor = self.input.len();
            },
            Some(i) => {
                self.history_idx = Some(i + 1);
                self.input = self.input_history[i + 1].clone();
                self.input_cursor = self.input.len();
            },
        }
    }

    /// Returns true when there is anything to submit: typed text, pasted blocks,
    /// file attachments, or images.  Used in the submit() early-return guard and
    /// in the janitor Done handler to decide whether an auto-resubmit is needed.
    pub fn has_pending_content(&self) -> bool {
        !self.input.trim().is_empty()
            || !self.pasted_blocks.is_empty()
            || !self.file_blocks.is_empty()
            || !self.image_blocks.is_empty()
    }

    /// Discard all pending user input (typed text, pastes, file attachments, images).
    /// Does NOT clear conversation history — use `new_conversation()` for that.
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.history_idx = None;
        self.input_saved.clear();
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
    /// Call this whenever `self.input` changes in Agent mode.
    pub fn update_slash_menu(&mut self) {
        // Only active when the input starts with '/'
        if !self.input.starts_with('/') {
            self.slash_menu = None;
            return;
        }
        let prefix = self.input.trim_start_matches('/');
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
                if let Some(ref fw) = self.spec_framework {
                    menu.description =
                        fw.describe(menu.items[menu.selected].as_str()).map(str::to_string);
                }
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
        // Write a session-end efficiency record for the conversation that is ending.
        if self.session_rounds > 0 {
            let files_changed = self.session_snapshots.len() + self.session_created_files.len();
            super::append_session_end_record(
                &self.last_submit_model,
                self.total_session_prompt_tokens,
                self.total_session_completion_tokens,
                self.session_rounds,
                files_changed,
                "new_conversation",
            );
        }
        // Compute adaptive round-limit hint for the incoming session.
        self.round_hint = super::suggest_max_rounds(self.selected_model_id());

        self.messages.clear();
        self.archived_messages.clear();
        self.tasks.clear();
        self.streaming_reply = None;
        self.total_session_prompt_tokens = 0;
        self.total_session_completion_tokens = 0;
        self.session_rounds = 0;
        self.session_start_secs = 0;
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
        self.messages.push(ChatMessage {
            role: Role::System,
            content: format!("── New conversation · {model_name} ──"),
            images: vec![],
        });
    }

    /// Submit input, launching the agentic tool-calling loop in the background.
    #[allow(clippy::too_many_arguments)]
    pub async fn submit(
        &mut self,
        context: Option<String>,
        project_root: PathBuf,
        max_rounds: usize,
        warning_threshold: usize,
        preferred_model: &str,
        auto_compress: bool,
        observation_mask_threshold_chars: usize,
        expand_threshold_chars: usize,
    ) -> Result<()> {
        if !self.has_pending_content() {
            return Ok(());
        }

        // Record the conversation start time on the first submit.
        if self.session_start_secs == 0 {
            self.session_start_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
        }

        // Save non-empty input to history before consuming it.
        let trimmed = self.input.trim().to_string();
        if !trimmed.is_empty() {
            self.input_history.push(trimmed);
            if self.input_history.len() > 50 {
                self.input_history.remove(0);
            }
        }
        self.input_cursor = 0;
        self.history_idx = None;
        self.input_saved.clear();

        let typed_text = std::mem::take(&mut self.input);
        let pasted = std::mem::take(&mut self.pasted_blocks);
        let files = std::mem::take(&mut self.file_blocks);
        let images = std::mem::take(&mut self.image_blocks);

        // Resolve context window early so the file-block cap below can use it.
        let context_limit = self.context_window_size();

        // Assemble user text: file blocks first (structured context), then pasted
        // blocks (ad-hoc snippets), then typed input.  Each section separated by \n\n.
        //
        // Cap total injected file content so the prompt never exceeds the model
        // window.  Each file is already truncated to AT_PICKER_MAX_LINES lines by the
        // picker; this enforces an aggregate ceiling.  Budget = 50% of the context
        // window, leaving room for the system prompt, history, and typed instruction.
        let max_file_tokens: usize = (context_limit as usize) / 2;
        let mut used_file_tokens: usize = 0;
        let mut parts: Vec<String> = Vec::new();
        for (name, content, _) in &files {
            let file_tokens = content.len() / 4;
            if used_file_tokens >= max_file_tokens {
                parts.push(format!(
                    "File: {name}\n\n[omitted — aggregate file context limit \
                     ({max_file_tokens} tokens) reached; use read_file to access this file]"
                ));
                continue;
            }
            let remaining_chars = (max_file_tokens - used_file_tokens) * 4;
            if content.len() > remaining_chars {
                let truncated: String = content
                    .lines()
                    .scan(0usize, |acc, line| {
                        *acc += line.len() + 1;
                        Some((*acc, line))
                    })
                    .take_while(|(acc, _)| *acc <= remaining_chars)
                    .map(|(_, line)| line)
                    .collect::<Vec<_>>()
                    .join("\n");
                used_file_tokens += truncated.len() / 4;
                parts.push(format!(
                    "File: {name}\n\n```\n{truncated}\n```\n\
                     [truncated — aggregate file context limit reached; \
                     use read_file for the rest]"
                ));
            } else {
                used_file_tokens += file_tokens;
                parts.push(format!("File: {name}\n\n```\n{content}\n```"));
            }
        }
        if used_file_tokens >= max_file_tokens {
            warn!(
                "[ctx] File block budget ({max_file_tokens}t) exhausted — \
                 some attached files were omitted or truncated. \
                 Select fewer files or use read_file in your instruction."
            );
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
        //
        // When the command sets clears_context (all built-in spec-kit phases do),
        // a new conversation is started automatically so the phase template runs
        // against a clean context window — no prior-phase history to re-send.
        //
        // Resolve into owned Strings first so the immutable borrow on self.spec_framework
        // is released before the potential mutable new_conversation() call.
        //
        // Phase 2 (ADR 0100): capture the raw command name and feature arg before
        // template expansion so the SpecSlicer can inject a virtual context block
        // for speckit.implement and speckit.tasks without reparsing user_text.
        let spec_cmd_ctx: Option<(String, String)> =
            user_text.trim_start().strip_prefix('/').and_then(|s| {
                let cmd = s.split_whitespace().next()?;
                if cmd.starts_with("speckit.") {
                    let rest = s[cmd.len()..].trim_start().to_string();
                    Some((cmd.to_string(), rest))
                } else {
                    None
                }
            });
        let resolved: Option<(String, String, bool)> =
            self.spec_framework.as_ref().and_then(|fw| {
                fw.resolve(&user_text)
                    .map(|(tmpl, rest, clears)| (tmpl.to_string(), rest.to_string(), clears))
            });
        let user_text = if let Some((template, rest, clears_context)) = resolved {
            if clears_context {
                let model_display = self.selected_model_display().to_string();
                let cmd =
                    user_text.trim_start_matches('/').split_whitespace().next().unwrap_or("?");
                info!("[spec] auto-clearing conversation before /{cmd} (clears_context = true)");
                self.new_conversation(&model_display);
            }
            // Append whatever the user typed after the command as context.
            if rest.is_empty() {
                template
            } else {
                format!("{template}{rest}")
            }
        } else {
            user_text
        };
        // Phase 2 — SpecSlicer (ADR 0100): for speckit.implement and speckit.tasks,
        // inject a pre-extracted virtual context block (active task + relevant spec
        // sections) to save the model a full-file read round on every implement turn.
        let user_text = if matches!(
            spec_cmd_ctx.as_ref().map(|(c, _)| c.as_str()),
            Some("speckit.implement") | Some("speckit.tasks")
        ) {
            let feature =
                spec_cmd_ctx.as_ref().and_then(|(_, r)| r.split_whitespace().next()).unwrap_or("");
            if !feature.is_empty() {
                let feature_dir = project_root.join("docs/spec/features").join(feature);
                match crate::spec_framework::spec_slicer::SpecSlicer::build(&feature_dir) {
                    Some(vctx) => {
                        let block = vctx.to_prompt_block();
                        info!(
                            "[spec] SpecSlicer: injected virtual context ({} t, task: {:?})",
                            super::token_count::count(&block),
                            vctx.active_task.title
                        );
                        format!("{user_text}\n\n{block}")
                    },
                    None => user_text,
                }
            } else {
                user_text
            }
        } else {
            user_text
        };

        // ── Resolve token + model before computing the context budget ────────
        // Fetching models first ensures context_window_size() returns the real
        // limit rather than the 128k fallback, so history truncation is correct
        // even on the very first message of a session (Fix for ADR 0087 §2).
        let api_token = self.ensure_token().await?;
        if self.available_models.is_empty() {
            let provider = self.provider.clone();
            let ollama_base_url = self.ollama_base_url.clone();
            let ollama_ctx = self.ollama_context_length;
            let openai_base_url = self.openai_base_url.clone();
            match fetch_models_for_provider(
                &provider,
                &api_token,
                &ollama_base_url,
                ollama_ctx,
                &openai_base_url,
            )
            .await
            {
                Ok(models) if !models.is_empty() => {
                    info!("Fetched {} models from {}", models.len(), provider.display_name());
                    self.set_models(models, preferred_model);
                },
                Ok(_) => warn!("[{}] model list was empty", provider.display_name()),
                Err(e) => warn!("Could not fetch {} model list: {e}", provider.display_name()),
            }
        }
        let model_id = self.selected_model_id().to_string();
        self.last_submit_model = model_id.clone();

        // Write a session_start record on the very first submit of a new
        // conversation (session_rounds == 0 before the first Done increments it).
        if self.session_rounds == 0 {
            super::append_session_start_record(
                &model_id,
                self.provider.display_name(),
                project_root.to_str().unwrap_or(""),
            );
        }

        // ── Build provider settings for this invocation ──────────────────────
        let chat_endpoint = match &self.provider {
            ProviderKind::Copilot => "https://api.githubcopilot.com/chat/completions".to_string(),
            ProviderKind::Ollama => format!("{}/v1/chat/completions", self.ollama_base_url),
            ProviderKind::Anthropic => "https://api.anthropic.com/v1/chat/completions".to_string(),
            ProviderKind::OpenAi => {
                format!("{}/chat/completions", self.openai_base_url)
            },
            ProviderKind::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
                    .to_string()
            },
            ProviderKind::OpenRouter => "https://openrouter.ai/api/v1/chat/completions".to_string(),
        };
        // For non-Copilot, non-Ollama providers the resolved api_key stored on
        // the panel is used directly.  For Copilot we use the OAuth token just
        // acquired.  For Ollama the token is empty (no auth).
        let effective_token = match self.provider {
            ProviderKind::Copilot => api_token.clone(),
            ProviderKind::Ollama => String::new(),
            _ => self.api_key.clone(),
        };
        let provider_settings = ProviderSettings {
            kind: self.provider.clone(),
            api_token: effective_token.clone(),
            chat_endpoint: chat_endpoint.clone(),
            num_ctx: if self.provider == ProviderKind::Ollama {
                self.ollama_context_length
            } else {
                None
            },
            supports_tool_calls: match self.provider {
                ProviderKind::Ollama => self.ollama_tool_calls,
                _ => true,
            },
            planning_tools: match self.provider {
                ProviderKind::Ollama => self.ollama_planning_tools,
                _ => true,
            },
            openrouter_site_url: self.openrouter_site_url.clone(),
            openrouter_app_name: self.openrouter_app_name.clone(),
        };

        let root_display = project_root.display().to_string();

        // ── Codified Context (ADR 0132) ──────────────────────────────────────
        // Reload on first submit or when the project root changes.
        if self.codified_context_enabled {
            let needs_reload = match &self.codified_context {
                None => true,
                Some(cc) => cc.forgiven_dir.parent() != Some(&project_root),
            };
            if needs_reload {
                use crate::config::CodifiedContextConfig;
                let cfg = CodifiedContextConfig {
                    enabled: true,
                    directory: ".forgiven".to_string(),
                    constitution_max_tokens: self.codified_context_constitution_max_tokens,
                    max_specialists_per_turn: self.codified_context_max_specialists,
                    knowledge_fetch_max_bytes: self.codified_context_knowledge_max_bytes,
                };
                self.codified_context =
                    crate::agent::codified_context::CodifiedContext::load(&project_root, &cfg);
            }
        }

        // Tip: show once per session when .forgiven/ is absent (enabled or not).
        if !self.codified_context_tip_shown && self.session_rounds == 0 {
            let forgiven_present = project_root.join(".forgiven").is_dir();
            if !forgiven_present {
                self.codified_context_tip_shown = true;
                self.messages.push(ChatMessage {
                    role: Role::System,
                    content: "[tip] Create .forgiven/constitution.md to improve agent \
                              consistency across sessions."
                        .to_string(),
                    images: vec![],
                });
            }
        }

        // ── Cap open-file context injection ─────────────────────────────────
        // The active buffer is useful orientation for small files, but sending
        // tens-of-thousands of tokens on every round (even unrelated rounds)
        // is the primary driver of context bloat (ADR 0087).
        // Cap to MAX_CTX_LINES; the model can call read_file for the rest.
        //
        // Phase 3.1: suppress injection entirely for specKit sessions — the
        // model works from TASKS.md/SPEC.md, not the active buffer.  Chat-mode
        // rounds keep the snippet for passive orientation.
        const MAX_CTX_LINES: usize = 150;
        let ctx_total_lines = context.as_ref().map(|c| c.lines().count()).unwrap_or(0);
        let suppress_ctx =
            spec_cmd_ctx.as_ref().map(|(cmd, _)| cmd.starts_with("speckit.")).unwrap_or(false);
        let context_snippet: Option<String> = if suppress_ctx {
            None
        } else {
            context.as_ref().map(|raw| {
                if ctx_total_lines > MAX_CTX_LINES {
                    raw.lines().take(MAX_CTX_LINES).collect::<Vec<_>>().join("\n")
                } else {
                    raw.clone()
                }
            })
        };

        // Build a structural map of src/ symbols (Phase 2.1).  On round 1 this
        // replaces the old filename tree and gives the model symbol-level
        // orientation without extra read_file discovery round-trips.
        // Cached for 30 s alongside cached_project_tree.
        const TREE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
        // Keep project_tree around for the round-2+ stub hint (no filesystem cost).
        let _project_tree = match self.cached_project_tree.as_ref() {
            Some((tree, ts)) if ts.elapsed() < TREE_TTL => tree.clone(),
            _ => {
                let tree = build_project_tree(&project_root, 2);
                self.cached_project_tree = Some((tree.clone(), std::time::Instant::now()));
                tree
            },
        };
        let structural_map = match self.cached_structural_map.as_ref() {
            Some((map, ts)) if ts.elapsed() < TREE_TTL => map.clone(),
            _ => {
                let map = build_structural_map(&project_root);
                self.cached_structural_map = Some((map.clone(), std::time::Instant::now()));
                map
            },
        };

        let use_planning = match self.provider {
            ProviderKind::Ollama => self.ollama_planning_tools,
            _ => true,
        };

        let planning_conventions = if use_planning {
            "- Tasks (≥3 distinct file ops): create_task per step before work; complete_task after.\n\
- ask_user: only for ambiguous destructive actions or mutually exclusive design choices.\n"
        } else {
            ""
        };

        let planning_tool_entries = if use_planning {
            "- create_task, complete_task — plan/track multi-step jobs\n\
- ask_user, ask_user_input — ask user a question or collect free-text input\n"
        } else {
            ""
        };

        let tool_rules = format!(
            "CONVENTIONS:\n\
- Symbol tools first: get_file_outline → get_symbol_context before read_file.\n\
  Use read_file only when you need more than 3 symbols from the same file.\n\
- Edits: edit_file over write_file; copy old_str verbatim; retry with fresh read on mismatch.\n\
- Batch: read_files([…]) over repeated read_file; search_files over read_file+scan.\n\
- Work silently; write one concise summary after all tools finish.\n\
{planning_conventions}\
- Memory (when tools available): search_nodes(\"project context\") on first call;\n\
  add_observations for non-obvious discoveries; persist key facts at session end.\n\
\n\
Tools:\n\
- get_file_outline, get_symbol_context — symbol-level retrieval (prefer these)\n\
- read_file — full file (expensive; use when >3 symbols needed)\n\
- read_files, search_files — batch reads and pattern search\n\
- write_file, edit_file — create/overwrite or surgical find-and-replace\n\
- list_directory — list directory contents\n\
- expand_result(id) — retrieve full content of a truncated tool result\n\
{planning_tool_entries}"
        );

        // Only include the full project tree on round 1 (session_rounds == 0).
        // Round 1: inject structural map (symbol names per file) instead of the
        // old filename-only tree.  Gives the model symbol-level orientation
        // (≈200–400 t) so it can skip discovery read_file calls.
        // Subsequent rounds: one-line stub saves 300–500 tokens per round.
        let tree_block = if self.session_rounds == 0 {
            format!("{structural_map}\n\n")
        } else {
            String::from("[Project tree omitted after round 1 — call list_directory if needed]\n\n")
        };

        // Codified context block (constitution + triggered specialists + knowledge catalogue).
        // Empty string when disabled or .forgiven/ absent.
        // Derive open file path from context header ("File: path/to/file\n\n...").
        let open_file_path = context
            .as_ref()
            .and_then(|s| s.lines().next())
            .and_then(|line| line.strip_prefix("File: "))
            .unwrap_or("")
            .to_string();
        let codified_block = if self.codified_context_enabled {
            self.codified_context
                .as_ref()
                .map(|cc| cc.system_prompt_block(&open_file_path, &user_text))
                .unwrap_or_default()
        } else {
            String::new()
        };

        let system = if let Some(ref ctx) = context_snippet {
            let truncation_note = if ctx_total_lines > MAX_CTX_LINES {
                format!(
                    "\n[Showing first {MAX_CTX_LINES} of {ctx_total_lines} lines — \
                     call read_file for the full content]"
                )
            } else {
                String::new()
            };
            format!(
                "You are an agentic coding assistant embedded in the 'forgiven' terminal editor.\n\
                 Project root: {root_display}\n\n\
                 {tree_block}\
                 {codified_block}\
                 {tool_rules}\n\
                 Currently open file (use read_file for full content):\n\
                 ```\n{ctx}{truncation_note}\n```"
            )
        } else {
            format!(
                "You are an agentic coding assistant embedded in the 'forgiven' terminal editor.\n\
                 Project root: {root_display}\n\n\
                 {tree_block}\
                 {codified_block}\
                 {tool_rules}"
            )
        };

        // For the Anthropic provider, split the system prompt into a stable
        // prefix (preamble + structural map + tool_rules) and the volatile
        // context_snippet so that the stable prefix is eligible for prompt
        // caching.  Other providers (Copilot, OpenAI) cache automatically on
        // the prefix — plain string content is correct for them.
        let system_message = if self.provider == ProviderKind::Anthropic {
            if let Some(ref _ctx) = context_snippet {
                // Find where context_snippet starts in the assembled system
                // string and split there.
                let ctx_marker = "Currently open file";
                if let Some(split_pos) = system.find(ctx_marker) {
                    let stable = &system[..split_pos];
                    let volatile = &system[split_pos..];
                    serde_json::json!({
                        "role": "system",
                        "content": [
                            {
                                "type": "text",
                                "text": stable,
                                "cache_control": { "type": "ephemeral" }
                            },
                            { "type": "text", "text": volatile }
                        ]
                    })
                } else {
                    // Couldn't find split point — cache the whole thing.
                    serde_json::json!({
                        "role": "system",
                        "content": [
                            {
                                "type": "text",
                                "text": system,
                                "cache_control": { "type": "ephemeral" }
                            }
                        ]
                    })
                }
            } else {
                // No context_snippet — entire system prompt is stable.
                serde_json::json!({
                    "role": "system",
                    "content": [
                        {
                            "type": "text",
                            "text": system,
                            "cache_control": { "type": "ephemeral" }
                        }
                    ]
                })
            }
        } else {
            serde_json::json!({ "role": "system", "content": system })
        };
        let mut send_messages: Vec<serde_json::Value> = vec![system_message];

        // ── Token-aware history truncation with importance scoring ───────────
        // Estimate tokens using the chars/4 approximation (1 token ≈ 4 chars).
        // Budget is 80% of the model's context window minus an estimate for the
        // system prompt, so we never approach the hard API limit.
        // context_limit already resolved above (before file-block assembly).
        let system_tokens = (system.len() / 4) as u32;
        let budget = (context_limit * 4 / 5).saturating_sub(system_tokens);

        // Snapshot for per-invocation metrics written on Done.
        self.last_submit_ctx = Some(SubmitCtx {
            ctx_window: context_limit,
            sys_tokens: system_tokens,
            budget_for_history: budget,
        });

        // ── Context budget audit log ─────────────────────────────────────────
        // Visible in SPC d → Recent Logs. One line per submission so you can
        // track how much of the window each component consumes.
        let ctx_file_tokens = context_snippet.as_ref().map(|c| c.len() / 4).unwrap_or(0);
        info!(
            "[ctx] window={}t  sys={}t (rules≈{}t + file≈{}t{})  history_msgs={}  budget_for_history={}t",
            context_limit,
            system_tokens,
            (system.len() - context_snippet.as_ref().map(|c| c.len()).unwrap_or(0)) / 4,
            ctx_file_tokens,
            if ctx_total_lines > MAX_CTX_LINES { format!(" [{}/{}lines]", MAX_CTX_LINES, ctx_total_lines) } else { String::new() },
            self.messages.iter().filter(|m| !matches!(m.role, Role::System)).count(),
            budget,
        );
        if system_tokens > context_limit / 2 {
            warn!(
                "[ctx] System prompt alone ({system_tokens}t) exceeds 50% of context window \
                 ({context_limit}t) — the open file ({ctx_file_tokens}t) is the likely cause. \
                 Close the file or switch to a model with a larger context window."
            );
        }

        // ── Phase 1: always keep the most recent MIN_RECENT non-system messages.
        const MIN_RECENT: usize = 4;
        let non_system: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| !matches!(m.role, Role::System))
            .map(|(i, _)| i)
            .collect();
        let recent_start_idx = non_system.len().saturating_sub(MIN_RECENT);
        let recent_indices: std::collections::HashSet<usize> =
            non_system[recent_start_idx..].iter().copied().collect();

        // Token cost of the guaranteed-recent slice.
        let recent_tokens: u32 =
            recent_indices.iter().map(|&i| (self.messages[i].content.len() / 4) as u32 + 4).sum();
        let older_budget = budget.saturating_sub(recent_tokens);

        // ── Phase 2: from older messages, greedily include highest-importance
        //    ones first until the older_budget is exhausted, then reassemble in
        //    original order so conversation coherence is preserved.
        let mut candidates: Vec<(usize, u32, u32)> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(i, m)| !matches!(m.role, Role::System) && !recent_indices.contains(i))
            .map(|(i, m)| {
                let tokens = (m.content.len() / 4) as u32 + 4;
                let score = super::message_importance(m);
                (i, tokens, score)
            })
            .collect();

        // Sort by score descending so high-value messages are included first.
        candidates.sort_by(|a, b| b.2.cmp(&a.2).then(b.0.cmp(&a.0)));

        let mut used: u32 = 0;
        let mut included: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (i, tokens, _) in &candidates {
            if used + tokens > older_budget {
                continue; // skip this one — too large — try smaller/higher-scored ones
            }
            used += tokens;
            included.insert(*i);
        }

        // Emit messages in original order (older included + recent).
        // Apply observation masking: non-recent assistant messages longer than
        // the threshold are replaced with a stub to reduce re-send token cost.
        // The display history (self.messages) is never modified.
        for (i, msg) in self.messages.iter().enumerate() {
            if matches!(msg.role, Role::System) {
                continue;
            }
            if !included.contains(&i) && !recent_indices.contains(&i) {
                continue;
            }
            let content = if observation_mask_threshold_chars > 0
                && !recent_indices.contains(&i)
                && matches!(msg.role, Role::Assistant)
                && msg.content.len() > observation_mask_threshold_chars
            {
                let tok_est = msg.content.len() / 4;
                format!(
                    "[assistant output: ~{tok_est} tokens — \
                     truncated for re-send; call the relevant tool again if needed]"
                )
            } else {
                msg.content.clone()
            };
            send_messages.push(serde_json::json!({
                "role": msg.role.as_str(),
                "content": content
            }));
        }
        // When images are attached, use the OpenAI content-array format so the
        // model receives both text and vision inputs.  Otherwise use a plain string.
        let image_dims: Vec<(u32, u32)> =
            images.iter().map(|img| (img.width, img.height)).collect();

        // ── Intent translation (SPC a t / [agent.intent_translator]) ─────────
        // Translate the raw message into a structured task spec before dispatch.
        // Falls through silently (uses raw message) on timeout, error, or short message.
        let (api_user_text, intent_preamble): (String, Option<String>) =
            if self.intent_translator_enabled {
                let open_file_hint = context
                    .as_ref()
                    .and_then(|c| c.strip_prefix("File: "))
                    .and_then(|c| c.lines().next());
                let language_hint = open_file_hint
                    .and_then(|p| std::path::Path::new(p).extension())
                    .and_then(|e| e.to_str())
                    .map(|ext| match ext {
                        "rs" => "Rust",
                        "ts" | "tsx" => "TypeScript",
                        "py" => "Python",
                        "go" => "Go",
                        "js" | "jsx" => "JavaScript",
                        "cpp" | "cc" | "cxx" => "C++",
                        "c" | "h" => "C",
                        "java" => "Java",
                        "rb" => "Ruby",
                        "swift" => "Swift",
                        _ => "unknown",
                    });
                let tx_ctx = super::intent::TranslationContext {
                    open_file: open_file_hint,
                    recent_files: &[],
                    project_root: &project_root,
                    language_hint,
                };
                // Resolve endpoint/token/model for the translator's chosen provider.
                // "ollama" → local Ollama, no auth; "active" → reuse main provider.
                let (tx_endpoint, tx_token, tx_model, tx_kind) =
                    if self.intent_translator_provider == "ollama" {
                        let ep = format!("{}/v1/chat/completions", self.ollama_base_url);
                        (
                            ep,
                            String::new(),
                            self.intent_translator_ollama_model.clone(),
                            ProviderKind::Ollama,
                        )
                    } else {
                        (
                            chat_endpoint.clone(),
                            effective_token.clone(),
                            self.intent_translator_model.clone(),
                            self.provider.clone(),
                        )
                    };
                let tx_settings = super::intent::IntentCallSettings {
                    endpoint: &tx_endpoint,
                    api_token: &tx_token,
                    model: &tx_model,
                    provider_kind: &tx_kind,
                    timeout_ms: self.intent_translator_timeout_ms,
                    min_chars_to_translate: self.intent_translator_min_chars,
                    skip_patterns: &self.intent_translator_skip_patterns,
                    openrouter_site_url: &self.openrouter_site_url,
                    openrouter_app_name: &self.openrouter_app_name,
                };
                match super::intent::translate_intent(&user_text, &tx_ctx, &tx_settings).await {
                    Some(intent) if !intent.ambiguities.is_empty() => {
                        // Ambiguous: show clarifying questions and bail out without
                        // dispatching the agent loop. The user refines and resubmits.
                        let questions = intent.ambiguities.join("\n• ");
                        self.messages.push(ChatMessage {
                            role: Role::System,
                            content: format!(
                                "Intent unclear — please clarify before submitting:\n• {questions}"
                            ),
                            images: vec![],
                        });
                        self.messages.push(ChatMessage {
                            role: Role::User,
                            content: user_text,
                            images: image_dims,
                        });
                        self.scroll = 0;
                        self.status = AgentStatus::Idle;
                        return Ok(());
                    },
                    Some(intent) if !intent.structured_prompt.is_empty() => {
                        let preamble = super::intent::format_preamble(&intent);
                        (intent.structured_prompt, Some(preamble))
                    },
                    _ => (user_text.clone(), None),
                }
            } else {
                (user_text.clone(), None)
            };

        let user_msg = if images.is_empty() {
            serde_json::json!({ "role": "user", "content": api_user_text.clone() })
        } else {
            let mut content_parts: Vec<serde_json::Value> = Vec::new();
            if !api_user_text.trim().is_empty() {
                content_parts.push(serde_json::json!({
                    "type": "text",
                    "text": api_user_text.clone()
                }));
            }
            for img in &images {
                content_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": img.data_uri, "detail": "auto" }
                }));
            }
            serde_json::json!({ "role": "user", "content": content_parts })
        };
        send_messages.push(user_msg);

        // ── Context breakdown for diagnostics / fuel gauge (Phase 1) ─────────
        // Compute per-segment token counts now that send_messages is fully assembled.
        // history = send_messages[1..n-1] (everything except system[0] and user[-1]).
        //
        // Uses the same len/4 approximation as history truncation (above) rather
        // than calling tiktoken here.  This keeps the breakdown numbers consistent
        // with the budget the truncation algorithm actually used, and avoids
        // O(N × tokeniser) cost on every submit for display-only numbers.
        let history_t: u32 = send_messages[1..send_messages.len().saturating_sub(1)]
            .iter()
            .map(|v| (v["content"].as_str().unwrap_or("").len() / 4) as u32)
            .sum();
        let ctx_file_t = context_snippet.as_ref().map(|c| (c.len() / 4) as u32).unwrap_or(0);
        // system_t is already computed via len/4 above (system_tokens); reuse it.
        let system_t = system_tokens;
        let user_msg_t = (user_text.len() / 4) as u32;
        self.last_breakdown = Some(super::ContextBreakdown {
            sys_rules_t: system_t.saturating_sub(ctx_file_t),
            ctx_file_t,
            history_t,
            user_msg_t,
            ctx_window: context_limit,
        });

        // Show the intent preamble (dim System line) above the user message.
        if let Some(preamble) = intent_preamble {
            self.messages.push(ChatMessage {
                role: Role::System,
                content: preamble,
                images: vec![],
            });
        }
        self.messages.push(ChatMessage {
            role: Role::User,
            content: user_text,
            images: image_dims,
        });

        self.scroll = 0;
        // Pre-allocate enough capacity for a typical streaming response to avoid
        // repeated heap reallocations as tokens accumulate via push_str.
        self.streaming_reply = Some(String::with_capacity(4096));
        self.tasks.clear();

        self.status = if self.janitor_compressing {
            AgentStatus::Compressing
        } else {
            AgentStatus::WaitingForResponse { round: 1 }
        };

        let (tx, rx) = mpsc::channel::<StreamEvent>(128);
        self.stream_rx = Some(rx);
        self.usage_received_this_round = false;

        let (cont_tx, cont_rx) = mpsc::unbounded_channel::<bool>();
        self.continuation_tx = Some(cont_tx);

        let (question_tx, question_rx) = mpsc::unbounded_channel::<String>();
        self.question_tx = Some(question_tx);

        let (abort_tx, abort_rx) = oneshot::channel::<()>();
        self.abort_tx = Some(abort_tx);

        let mcp = self.mcp_manager.as_ref().map(Arc::clone);
        let knowledge_docs: Vec<(String, PathBuf)> = self
            .codified_context
            .as_ref()
            .map(|cc| cc.knowledge_docs.iter().map(|d| (d.name.clone(), d.path.clone())).collect())
            .unwrap_or_default();
        let knowledge_fetch_max_bytes = self.codified_context_knowledge_max_bytes;
        tokio::spawn(agentic_loop(
            provider_settings,
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
            auto_compress,
            expand_threshold_chars,
            knowledge_docs,
            knowledge_fetch_max_bytes,
        ));
        Ok(())
    }

    /// Launch a single-round, no-tool LLM request for the inline assistant (ADR 0111).
    ///
    /// Builds a minimal 2-message conversation (system + user), spawns `agentic_loop`
    /// with `max_rounds = 1` and tool calling disabled, and returns the streaming channel
    /// pair.  The caller (Editor) wraps these in an `InlineAssistState`.
    pub async fn start_inline_assist(
        &mut self,
        selection_text: String,
        prompt: String,
        project_root: PathBuf,
        language: Option<String>,
    ) -> Result<(mpsc::Receiver<StreamEvent>, oneshot::Sender<()>)> {
        let api_token = self.ensure_token().await?;
        let model_id = self.selected_model_id().to_string();

        let chat_endpoint = match &self.provider {
            ProviderKind::Copilot => "https://api.githubcopilot.com/chat/completions".to_string(),
            ProviderKind::Ollama => format!("{}/v1/chat/completions", self.ollama_base_url),
            ProviderKind::Anthropic => "https://api.anthropic.com/v1/chat/completions".to_string(),
            ProviderKind::OpenAi => format!("{}/chat/completions", self.openai_base_url),
            ProviderKind::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
                    .to_string()
            },
            ProviderKind::OpenRouter => "https://openrouter.ai/api/v1/chat/completions".to_string(),
        };
        let effective_token = match self.provider {
            ProviderKind::Copilot => api_token,
            ProviderKind::Ollama => String::new(),
            _ => self.api_key.clone(),
        };

        // Disable tool calling — inline assist is pure text generation.
        let provider_settings = super::provider::ProviderSettings {
            kind: self.provider.clone(),
            api_token: effective_token,
            chat_endpoint,
            num_ctx: if self.provider == ProviderKind::Ollama {
                self.ollama_context_length
            } else {
                None
            },
            supports_tool_calls: false,
            planning_tools: false,
            openrouter_site_url: self.openrouter_site_url.clone(),
            openrouter_app_name: self.openrouter_app_name.clone(),
        };

        let lang_str = language.as_deref().unwrap_or("code");
        let system_prompt = format!(
            "You are a {lang_str} code transformation engine. \
            You receive a CODE block and a DIRECTIVE. \
            You output ONLY the transformed {lang_str} code — \
            no conversation, no explanation, no markdown fences, no preamble. \
            If the code is empty, output only what was asked for. \
            Preserve the original indentation."
        );

        let user_content = if selection_text.is_empty() {
            format!("CODE:\n(none)\n\nDIRECTIVE: {prompt}")
        } else {
            format!("CODE:\n{selection_text}\n\nDIRECTIVE: {prompt}")
        };

        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt }),
            serde_json::json!({ "role": "user", "content": user_content }),
        ];

        let (tx, rx) = mpsc::channel::<StreamEvent>(128);
        let (abort_tx, abort_rx) = oneshot::channel::<()>();
        // Dummy continuation + question channels — inline assist never uses them.
        let (_cont_tx, cont_rx) = mpsc::unbounded_channel::<bool>();
        let (_question_tx, question_rx) = mpsc::unbounded_channel::<String>();

        tokio::spawn(agentic_loop(
            provider_settings,
            messages,
            project_root,
            tx,
            model_id,
            1, // max_rounds
            0, // warning_threshold
            cont_rx,
            question_rx,
            abort_rx,
            None,  // no MCP manager
            false, // auto_compress
            0,     // expand_threshold — disabled for inline assist
            vec![],
            0,
        ));

        Ok((rx, abort_tx))
    }

    /// Spin up a single-round, tool-enabled investigation subagent (Phase 3.3).
    ///
    /// The query comes from `self.input`.  The agentic loop runs for at most one
    /// round with all tools available so the model can explore the codebase.
    /// Tokens are collected in `self.investigation_buf`; when `Done` arrives in
    /// `poll_stream()` the summary is injected as a System message into the main
    /// session and `investigation_rx` is cleared.
    ///
    /// Callers must ensure `self.input` is non-empty before calling.
    pub async fn start_investigation_agent(
        &mut self,
        project_root: PathBuf,
        preferred_model: &str,
    ) -> Result<()> {
        if self.input.trim().is_empty() {
            return Ok(());
        }

        let api_token = self.ensure_token().await?;
        let model_id = self.selected_model_id_with_fallback(preferred_model).to_string();

        let chat_endpoint = match &self.provider {
            ProviderKind::Copilot => "https://api.githubcopilot.com/chat/completions".to_string(),
            ProviderKind::Ollama => format!("{}/v1/chat/completions", self.ollama_base_url),
            ProviderKind::Anthropic => "https://api.anthropic.com/v1/chat/completions".to_string(),
            ProviderKind::OpenAi => format!("{}/chat/completions", self.openai_base_url),
            ProviderKind::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
                    .to_string()
            },
            ProviderKind::OpenRouter => "https://openrouter.ai/api/v1/chat/completions".to_string(),
        };
        let effective_token = match self.provider {
            ProviderKind::Copilot => api_token,
            ProviderKind::Ollama => String::new(),
            _ => self.api_key.clone(),
        };

        let provider_settings = super::provider::ProviderSettings {
            kind: self.provider.clone(),
            api_token: effective_token,
            chat_endpoint,
            num_ctx: if self.provider == ProviderKind::Ollama {
                self.ollama_context_length
            } else {
                None
            },
            supports_tool_calls: true,
            planning_tools: false, // investigation doesn't create tasks
            openrouter_site_url: self.openrouter_site_url.clone(),
            openrouter_app_name: self.openrouter_app_name.clone(),
        };

        let root_display = project_root.display().to_string();
        let system_prompt = format!(
            "You are a code investigator for the 'forgiven' terminal editor.\n\
             Project root: {root_display}\n\n\
             INVESTIGATION RULES:\n\
             - Explore the codebase using get_file_outline, get_symbol_context, search_files, \
               and read_file as needed.\n\
             - Make NO edits — this is read-only exploration.\n\
             - After exploring, output a COMPACT SUMMARY (max 200 words) covering:\n\
               * Which files/functions are involved\n\
               * Key call paths or data flow\n\
               * Any non-obvious facts the developer should know\n\
             - No preamble, no pleasantries. Start directly with the findings."
        );

        let query = std::mem::take(&mut self.input);
        self.input_cursor = 0;

        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt }),
            serde_json::json!({ "role": "user", "content": query }),
        ];

        let (tx, rx) = mpsc::channel::<StreamEvent>(128);
        let (_abort_tx, abort_rx) = oneshot::channel::<()>();
        let (_cont_tx, cont_rx) = mpsc::unbounded_channel::<bool>();
        let (_question_tx, question_rx) = mpsc::unbounded_channel::<String>();

        let mcp = self.mcp_manager.clone();
        tokio::spawn(agentic_loop(
            provider_settings,
            messages,
            project_root,
            tx,
            model_id,
            1, // max_rounds — single exploration pass
            0, // warning_threshold
            cont_rx,
            question_rx,
            abort_rx,
            mcp,
            false, // auto_compress
            0,     // expand_threshold — disabled for investigation subagent
            vec![],
            0,
        ));

        self.investigation_rx = Some(rx);
        self.investigation_buf.clear();
        self.status = AgentStatus::Investigating;
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
                        self.status = if self.janitor_compressing {
                            AgentStatus::Compressing
                        } else {
                            AgentStatus::Streaming { round: self.current_round }
                        };
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
                    Ok(StreamEvent::ToolDone { name, result_summary, success }) => {
                        active = true;
                        self.status = AgentStatus::WaitingForResponse { round: self.current_round };
                        self.pending_tool_calls.push((name.clone(), success));
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
                    Ok(StreamEvent::FileSnapshot { path, original }) => {
                        active = true;
                        // Only store the first snapshot per path per session.
                        self.session_snapshots.entry(path).or_insert(original);
                    },
                    Ok(StreamEvent::FileCreated { path }) => {
                        active = true;
                        if !self.session_created_files.contains(&path) {
                            self.session_created_files.push(path);
                        }
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
                    Ok(StreamEvent::AskingUserInput { question, placeholder }) => {
                        active = true;
                        self.asking_user_input = Some(AskUserInputState {
                            question,
                            placeholder,
                            input: String::new(),
                            cursor: 0,
                        });
                    },
                    Ok(StreamEvent::Retrying { attempt, max }) => {
                        active = true;
                        self.status = AgentStatus::Retrying { attempt, max };
                    },
                    Ok(StreamEvent::Usage { prompt_tokens, completion_tokens, cached_tokens }) => {
                        self.last_prompt_tokens = prompt_tokens;
                        self.last_completion_tokens = completion_tokens;
                        self.last_cached_tokens = cached_tokens;
                        self.usage_received_this_round = true;
                        self.total_session_prompt_tokens =
                            self.total_session_prompt_tokens.saturating_add(prompt_tokens);
                        self.total_session_completion_tokens =
                            self.total_session_completion_tokens.saturating_add(completion_tokens);
                        let window = if self.available_models.is_empty() {
                            128_000u32
                        } else {
                            self.available_models
                                [self.selected_model.min(self.available_models.len() - 1)]
                            .context_window
                        }
                        .max(1);
                        let pct = prompt_tokens * 100 / window;
                        let cached_note = if cached_tokens > 0 {
                            format!("  cached={cached_tokens}t")
                        } else {
                            String::new()
                        };
                        if pct >= 80 {
                            warn!(
                                "[usage] prompt={prompt_tokens}t ({pct}% of {window}t window)  \
                                 completion={completion_tokens}t{cached_note}  \
                                 session_total={}t",
                                self.total_session_prompt_tokens
                            );
                        } else {
                            info!(
                                "[usage] prompt={prompt_tokens}t ({pct}% of {window}t window)  \
                                 completion={completion_tokens}t{cached_note}  \
                                 session_total={}t",
                                self.total_session_prompt_tokens
                            );
                        }
                    },
                    Ok(StreamEvent::ModelSwitched { from, to }) => {
                        active = true;
                        // Unexpected switch = premium quota exceeded; update selection and warn.
                        if let Some(idx) =
                            self.available_models.iter().position(|m| m.id == to || m.version == to)
                        {
                            self.selected_model = idx;
                        }
                        let notice = format!(
                            "\n\n> ⚠  Copilot switched model: **{from}** → **{to}** (premium quota exceeded)\n\n"
                        );
                        match self.streaming_reply.as_mut() {
                            Some(r) => r.push_str(&notice),
                            None => self.streaming_reply = Some(notice),
                        }
                    },
                    Ok(StreamEvent::Done) => {
                        // Flush accumulated tool calls to history JSONL before
                        // the assistant reply is pushed, so the record is ordered
                        // before the final text response in the file.
                        let round_tools = std::mem::take(&mut self.pending_tool_calls);
                        if !round_tools.is_empty() {
                            super::append_round_tools(self.session_start_secs, &round_tools);
                            for (name, success) in &round_tools {
                                if *success {
                                    match name.as_str() {
                                        "read_file" | "read_files" => {
                                            self.session_read_file_count += 1
                                        },
                                        "get_symbol_context" => self.session_symbol_count += 1,
                                        "get_file_outline" => self.session_outline_count += 1,
                                        _ => {},
                                    }
                                }
                            }
                        }
                        if let Some(text) = self.streaming_reply.take() {
                            if !text.is_empty() {
                                self.messages.push(ChatMessage {
                                    role: Role::Assistant,
                                    content: text,
                                    images: vec![],
                                });
                            }
                        }
                        // ── Auto-Janitor: apply summary if this was a compression round ──
                        if self.janitor_compressing {
                            self.janitor_compressing = false;
                            // The last message is the LLM's summary — extract and rebuild history.
                            let summary = self
                                .messages
                                .last()
                                .filter(|m| matches!(m.role, Role::Assistant))
                                .map(|m| m.content.clone())
                                .unwrap_or_default();
                            // Write session-end record before clearing counters.
                            if self.session_rounds > 0 {
                                let files_changed =
                                    self.session_snapshots.len() + self.session_created_files.len();
                                super::append_session_end_record(
                                    &self.last_submit_model,
                                    self.total_session_prompt_tokens,
                                    self.total_session_completion_tokens,
                                    self.session_rounds,
                                    files_changed,
                                    "janitor",
                                );
                            }
                            // Original messages were already archived in compress_history().
                            // Discard the janitor round (prompt + response) — it's a
                            // technical artifact, not a real conversation turn.
                            self.messages.clear();
                            self.total_session_prompt_tokens = 0;
                            self.total_session_completion_tokens = 0;
                            self.session_rounds = 0;
                            self.messages.push(ChatMessage {
                                role: Role::System,
                                content: "── Context compressed by Auto-Janitor ──".to_string(),
                                images: vec![],
                            });
                            if !summary.is_empty() {
                                // Store as a User/Assistant exchange so the model treats
                                // the summary as part of its own conversational memory
                                // rather than as an external system instruction.  This
                                // prevents "context amnesia" where the model ignores the
                                // summary and acts as if starting fresh.
                                self.messages.push(ChatMessage {
                                    role: Role::User,
                                    content: "Briefly recap what we accomplished.".to_string(),
                                    images: vec![],
                                });
                                self.messages.push(ChatMessage {
                                    role: Role::Assistant,
                                    content: summary,
                                    images: vec![],
                                });
                            }
                            // Skip metrics append and the threshold check below — the session
                            // counters were just reset.
                            self.code_block_idx = 0;
                            self.mermaid_block_idx = 0;
                            self.scroll = 0;
                            self.stream_rx = None;
                            self.continuation_tx = None;
                            self.question_tx = None;
                            self.asking_user = None;
                            self.asking_user_input = None;
                            self.awaiting_continuation = false;
                            self.current_round = 0;
                            self.status = AgentStatus::JanitorDone;
                            break;
                        }
                        // ── Token estimation fallback (Ollama + providers without usage events) ──
                        // If no StreamEvent::Usage arrived this round, estimate from message
                        // content so the janitor threshold can still fire.
                        if !self.usage_received_this_round {
                            let estimated: u32 = self
                                .messages
                                .iter()
                                .map(|m| (m.content.len() / 4 + 4) as u32)
                                .sum::<u32>()
                                .max(1);
                            self.total_session_prompt_tokens =
                                self.total_session_prompt_tokens.saturating_add(estimated);
                        }
                        self.usage_received_this_round = false;
                        self.round_hint = None; // hint served its purpose after first round
                                                // ── Persist invocation metrics ───────────────────────
                        self.session_rounds = self.session_rounds.saturating_add(1);
                        if self.last_prompt_tokens > 0 {
                            let ts = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let (ctx_window, sys_tokens, budget) = self
                                .last_submit_ctx
                                .map(|c| (c.ctx_window, c.sys_tokens, c.budget_for_history))
                                .unwrap_or((128_000, 0, 0));
                            let pct = self.last_prompt_tokens * 100 / ctx_window.max(1);
                            append_session_metric(&serde_json::json!({
                                "ts": ts,
                                "model": self.last_submit_model,
                                "prompt_tokens": self.last_prompt_tokens,
                                "completion_tokens": self.last_completion_tokens,
                                "cached_tokens": self.last_cached_tokens,
                                "ctx_window": ctx_window,
                                "sys_tokens": sys_tokens,
                                "budget_for_history": budget,
                                "session_prompt_total": self.total_session_prompt_tokens,
                                "session_completion_total": self.total_session_completion_tokens,
                                "pct": pct,
                            }));
                        }
                        // ── 100 k session-total warning ──────────────────────
                        // Fires once per conversation when cumulative re-send cost
                        // crosses 100k tokens — earlier than the 90% per-round check,
                        // giving the user a softer nudge to run the janitor soon.
                        if self.total_session_prompt_tokens > 100_000
                            && !self.session_total_100k_warned
                        {
                            self.session_total_100k_warned = true;
                            self.messages.push(ChatMessage {
                                role: Role::System,
                                content: format!(
                                    "\u{2139}  Session total: {}k tokens. \
                                     Consider running SPC a j before your next task.",
                                    self.total_session_prompt_tokens / 1_000
                                ),
                                images: vec![],
                            });
                        }
                        // ── 90 % context-window warning ──────────────────────
                        // Post a visible chat message the first time a round's prompt
                        // reaches 90 % of the model's context window.  The fuel gauge
                        // in the panel title already turns red at 80 %; this fires a
                        // more actionable in-chat nudge at the higher threshold so the
                        // user knows to run SPC a j before the session hits the limit.
                        if self.last_prompt_tokens > 0 && !self.context_near_limit_warned {
                            let window = self.context_window_size();
                            let pct = self.last_prompt_tokens * 100 / window.max(1);
                            if pct >= 90 {
                                self.context_near_limit_warned = true;
                                self.messages.push(ChatMessage {
                                    role: Role::System,
                                    content: format!(
                                        "⚠\u{fe0f}  Context {pct}% full — \
                                         press SPC a j to compress history \
                                         before your next message."
                                    ),
                                    images: vec![],
                                });
                            }
                        }
                        self.code_block_idx = 0;
                        self.mermaid_block_idx = 0;
                        self.scroll = 0;
                        self.stream_rx = None;
                        self.continuation_tx = None;
                        self.question_tx = None;
                        self.asking_user = None;
                        self.asking_user_input = None;
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
                            images: vec![],
                        });
                        self.last_error = Some(e);
                        self.streaming_reply = None;
                        self.stream_rx = None;
                        self.continuation_tx = None;
                        self.question_tx = None;
                        self.asking_user = None;
                        self.asking_user_input = None;
                        self.awaiting_continuation = false;
                        self.current_round = 0;
                        self.status = AgentStatus::Idle;
                        break;
                    },
                    Err(_) => break,
                }
            }
        }

        // ── Investigation subagent drain ─────────────────────────────────────
        // Drain the investigation stream independently of the main stream.
        // On Done, inject the collected summary as a System message so the user
        // can see it and the main session has it in context for the next round.
        if let Some(rx) = self.investigation_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(StreamEvent::Token(t)) => {
                        active = true;
                        self.investigation_buf.push_str(&t);
                    },
                    Ok(StreamEvent::Done) => {
                        let summary = std::mem::take(&mut self.investigation_buf);
                        if !summary.trim().is_empty() {
                            self.messages.push(ChatMessage {
                                role: Role::System,
                                content: format!("🔍 Investigation result:\n{summary}"),
                                images: vec![],
                            });
                        }
                        self.investigation_rx = None;
                        self.status = AgentStatus::Idle;
                        active = true;
                        break;
                    },
                    Ok(StreamEvent::Error(e)) => {
                        self.messages.push(ChatMessage {
                            role: Role::System,
                            content: format!("🔍 Investigation error: {e}"),
                            images: vec![],
                        });
                        self.investigation_rx = None;
                        self.status = AgentStatus::Idle;
                        active = true;
                        break;
                    },
                    Ok(_) => {}, // tool events — investigation is read-only; ignore
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
                self.messages.push(ChatMessage { role: Role::Assistant, content, images: vec![] });
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
        self.messages.iter().rev().find(|m| m.role == Role::Assistant).map(|m| m.content.clone())
    }

    async fn ensure_token(&mut self) -> Result<String> {
        match self.provider {
            ProviderKind::Ollama => {
                // No authentication needed.
                Ok(String::new())
            },
            ProviderKind::Anthropic
            | ProviderKind::OpenAi
            | ProviderKind::Gemini
            | ProviderKind::OpenRouter => {
                // Pre-resolved api_key stored on the panel at startup.
                // Return it here so model-fetch calls can use it.
                Ok(self.api_key.clone())
            },
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
                self.token = Some(api_token);
                Ok(tok)
            },
        }
    }
}
