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
    append_session_metric, AgentPanel, AgentStatus, AgentTask, AskUserState, ChatMessage,
    ClipboardImage, ModelVersion, Role, SlashMenuState, StreamEvent, SubmitCtx,
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
            question_tx: None,
            asking_user: None,
            slash_menu: None,
            file_blocks: Vec::new(),
            at_picker: None,
            janitor_compressing: false,
            pending_janitor: false,
            cached_project_tree: None,
            session_snapshots: std::collections::HashMap::new(),
            session_created_files: Vec::new(),
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
        self.input.push(ch);
    }

    pub fn input_backspace(&mut self) {
        self.input.pop();
    }

    pub fn input_newline(&mut self) {
        self.input.push('\n');
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
                let desc = fw.describe(menu.items[menu.selected].as_str()).map(str::to_string);
                menu.description = desc;
            },
            None => {
                let description =
                    items.first().and_then(|cmd| fw.describe(cmd.as_str())).map(str::to_string);
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
        self.messages.clear();
        self.archived_messages.clear();
        self.tasks.clear();
        self.streaming_reply = None;
        self.total_session_prompt_tokens = 0;
        self.total_session_completion_tokens = 0;
        self.session_rounds = 0;
        self.cached_project_tree = None;
        self.session_snapshots.clear();
        self.session_created_files.clear();
        self.messages.push(ChatMessage {
            role: Role::System,
            content: format!("── New conversation · {model_name} ──"),
            images: vec![],
        });
    }

    /// Prepare a janitor summarisation round.
    ///
    /// Serialises the current non-separator conversation into the user input field
    /// as a single summarisation prompt, clears the message history so the outgoing
    /// API call carries no prior context, and sets `janitor_compressing = true` so
    /// `poll_stream()` knows to replace history with the returned summary.
    ///
    /// The caller is responsible for immediately calling `submit()` after this.
    pub fn compress_history(&mut self) {
        // Collect all non-separator messages (skip Role::System separators like
        // "── New conversation · …" and "── Context compressed · …").
        let history_text: String = self
            .messages
            .iter()
            .filter(|m| {
                // Keep user/assistant messages; drop system separator lines.
                !matches!(m.role, Role::System)
            })
            .map(|m| {
                let label = match m.role {
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                    Role::System => "System",
                };
                format!("**{}:** {}", label, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        if history_text.is_empty() {
            // Nothing to compress.
            return;
        }

        let prompt = format!(
            "Summarise the technical decisions, key findings, and important context \
             from the conversation below into a concise bulleted list. \
             Discard chit-chat and completed throwaway tasks. \
             Focus on what would be expensive to re-discover.\n\n\
             <conversation>\n{history_text}\n</conversation>"
        );

        // Drop existing history — the janitor round goes out bare (no prior context).
        self.messages.clear();
        self.tasks.clear();
        self.input = prompt;
        self.janitor_compressing = true;
    }

    /// Submit input, launching the agentic tool-calling loop in the background.
    pub async fn submit(
        &mut self,
        context: Option<String>,
        project_root: PathBuf,
        max_rounds: usize,
        warning_threshold: usize,
        preferred_model: &str,
        auto_compress: bool,
    ) -> Result<()> {
        if self.input.trim().is_empty()
            && self.pasted_blocks.is_empty()
            && self.file_blocks.is_empty()
            && self.image_blocks.is_empty()
        {
            return Ok(());
        }

        let typed_text = std::mem::take(&mut self.input);
        let pasted = std::mem::take(&mut self.pasted_blocks);
        let files = std::mem::take(&mut self.file_blocks);
        let images = std::mem::take(&mut self.image_blocks);

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
            api_token: effective_token,
            chat_endpoint,
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

        // ── Cap open-file context injection ─────────────────────────────────
        // The active buffer is useful orientation for small files, but sending
        // tens-of-thousands of tokens on every round (even unrelated rounds)
        // is the primary driver of context bloat (ADR 0087).
        // Cap to MAX_CTX_LINES; the model can call read_file for the rest.
        const MAX_CTX_LINES: usize = 150;
        let ctx_total_lines = context.as_ref().map(|c| c.lines().count()).unwrap_or(0);
        let context_snippet: Option<String> = context.as_ref().map(|raw| {
            if ctx_total_lines > MAX_CTX_LINES {
                raw.lines().take(MAX_CTX_LINES).collect::<Vec<_>>().join("\n")
            } else {
                raw.clone()
            }
        });

        // Build a shallow file tree so the model knows the project layout upfront
        // and never needs to burn rounds on list_directory exploration.
        // The tree is cached for up to 30 s to avoid a full filesystem walk on
        // every submit (the tree rarely changes within a single session).
        const TREE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
        let project_tree = match self.cached_project_tree.as_ref() {
            Some((tree, ts)) if ts.elapsed() < TREE_TTL => tree.clone(),
            _ => {
                let tree = build_project_tree(&project_root, 2);
                self.cached_project_tree = Some((tree.clone(), std::time::Instant::now()));
                tree
            },
        };

        let use_planning = match self.provider {
            ProviderKind::Ollama => self.ollama_planning_tools,
            _ => true,
        };

        let planning_rules = if use_planning {
            "TASK PLANNING RULES:\n\
0. Use create_task / complete_task ONLY when the job involves 3 or more distinct\n\
   file operations (creates, rewrites, or edits across different files), OR when\n\
   the user explicitly asks you to plan or list steps.\n\
   Do NOT plan for: questions, explanations, single-file edits, or any task\n\
   completable in 1-2 tool calls. Reading a file before editing it does NOT count\n\
   as a separate step.\n\
   When planning IS needed, call create_task ONCE per step BEFORE any file work,\n\
   keep titles short and imperative (e.g. 'Create Program.cs'), and call\n\
   complete_task with the exact same title after finishing each step.\n\n"
        } else {
            ""
        };

        let ask_user_rule = if use_planning {
            "7. Use ask_user ONLY when you genuinely cannot proceed without clarification —\n\
   e.g., ambiguous destructive actions or mutually exclusive design choices.\n\
   Do NOT use it to confirm routine read/write operations.\n\n"
        } else {
            ""
        };

        let planning_tool_entries = if use_planning {
            "- create_task          Register a planned step (call once per step before file work).\n\
- complete_task        Mark a step done (call after finishing each step).\n\
- ask_user             Show the user a question dialog and wait for their choice.\n"
        } else {
            ""
        };

        let tool_rules = format!(
            "MANDATORY PROTOCOL — follow these rules without exception:\n\
\n\
{planning_rules}\
COMMUNICATION RULES:\n\
6. Do NOT output any text while working through tool calls. Work silently.\n\
   After ALL tools have finished, ALWAYS write a concise summary of what was\n\
   accomplished (files changed, what was added/removed/fixed, and any caveats).\n\
   Do not narrate steps, explain retries, or announce what you are about to do.\n\
7. Be maximally concise in every response. No filler phrases, no hedging, no\n\
   pleasantries. If the answer is one sentence, write one sentence. Never use\n\
   'Certainly!', 'Of course', 'I'll now...', or similar preamble. State only\n\
   what changed and why — nothing else.\n\
{ask_user_rule}\
FILE EDITING RULES:\n\
1. Before editing a file, prefer get_file_outline to understand its structure,\n\
   then get_symbol_context to get the specific symbol you need. Only fall back\n\
   to read_file when you need the full contents (e.g. for a new file or a\n\
   write_file rewrite). This saves tokens.\n\
2. Copy old_str VERBATIM from the tool output, including all whitespace,\n\
   indentation, and surrounding lines needed to make it unique in the file.\n\
3. If edit_file returns an error, call get_symbol_context or read_file again\n\
   to get the current content and retry with the correct old_str.\n\
   Do NOT retry with the same old_str.\n\
4. Prefer edit_file over write_file for any change to an existing file.\n\
5. Use list_directory only if the project tree above is insufficient.\n\
6. When you need several files, use read_files([...]) instead of multiple\n\
   read_file calls. Use search_files(pattern, [...]) instead of read_file + scan.\n\
\n\
MEMORY RULES (only when memory tools are available):\n\
- At the START of a new session, call search_nodes with query 'project context'\n\
  to retrieve any facts stored from prior sessions before asking the user.\n\
- During work, call add_observations when you discover non-obvious facts about\n\
  the codebase (architecture decisions, gotchas, key file locations).\n\
- At the END of a significant session, call create_entities + add_observations\n\
  to persist what you learned for future sessions.\n\
\n\
Available tools:\n\
{planning_tool_entries}\
- get_file_outline     List all top-level definitions in a file (signatures only, no bodies).\n\
                       Use this first to find where a symbol lives — much cheaper than read_file.\n\
- get_symbol_context   Get the full body of one symbol + signatures of what it calls.\n\
                       Use after get_file_outline to get focused context before an edit.\n\
- read_file            Read a file's full line-numbered content. Use when full content is needed.\n\
- read_files           Read multiple files in one call (preferred over repeated read_file).\n\
- search_files         Search for a pattern across files/directories (returns file:line: text).\n\
- write_file           Write a complete file (for new files or full rewrites only).\n\
- edit_file            Surgical find-and-replace. old_str must match EXACTLY once.\n\
- list_directory       List a directory's contents.\n"
        );

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
                 Project file tree (depth 2 — use read_file to see contents):\n\
                 ```\n{project_tree}```\n\n\
                 {tool_rules}\n\
                 Currently open file (use read_file for full content):\n\
                 ```\n{ctx}{truncation_note}\n```"
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

        // ── Token-aware history truncation with importance scoring ───────────
        // Estimate tokens using the chars/4 approximation (1 token ≈ 4 chars).
        // Budget is 80% of the model's context window minus an estimate for the
        // system prompt, so we never approach the hard API limit.
        let context_limit = self.context_window_size();
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
        for (i, msg) in self.messages.iter().enumerate() {
            if matches!(msg.role, Role::System) {
                continue;
            }
            if !included.contains(&i) && !recent_indices.contains(&i) {
                continue;
            }
            send_messages.push(serde_json::json!({
                "role": msg.role.as_str(),
                "content": msg.content
            }));
        }
        // When images are attached, use the OpenAI content-array format so the
        // model receives both text and vision inputs.  Otherwise use a plain string.
        let image_dims: Vec<(u32, u32)> =
            images.iter().map(|img| (img.width, img.height)).collect();

        let user_msg = if images.is_empty() {
            serde_json::json!({ "role": "user", "content": user_text.clone() })
        } else {
            let mut content_parts: Vec<serde_json::Value> = Vec::new();
            if !user_text.trim().is_empty() {
                content_parts.push(serde_json::json!({
                    "type": "text",
                    "text": user_text.clone()
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

        self.messages.push(ChatMessage {
            role: Role::User,
            content: user_text,
            images: image_dims,
        });

        self.scroll = 0;
        self.streaming_reply = Some(String::new());
        self.tasks.clear();

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
    ) -> Result<(mpsc::UnboundedReceiver<StreamEvent>, oneshot::Sender<()>)> {
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

        let (tx, rx) = mpsc::unbounded_channel::<StreamEvent>();
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
        ));

        Ok((rx, abort_tx))
    }

    pub fn poll_stream(&mut self, janitor_threshold: u32) -> bool {
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
                    Ok(StreamEvent::Retrying { attempt, max }) => {
                        active = true;
                        self.status = AgentStatus::Retrying { attempt, max };
                    },
                    Ok(StreamEvent::Usage { prompt_tokens, completion_tokens, cached_tokens }) => {
                        self.last_prompt_tokens = prompt_tokens;
                        self.last_completion_tokens = completion_tokens;
                        self.last_cached_tokens = cached_tokens;
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
                            // Move live messages to the archive so the user
                            // can still scroll up to them; don't discard.
                            self.archived_messages.extend(std::mem::take(&mut self.messages));
                            self.total_session_prompt_tokens = 0;
                            self.total_session_completion_tokens = 0;
                            self.session_rounds = 0;
                            self.messages.push(ChatMessage {
                                role: Role::System,
                                content: "── Context compressed by Auto-Janitor ──".to_string(),
                                images: vec![],
                            });
                            if !summary.is_empty() {
                                self.messages.push(ChatMessage {
                                    role: Role::System,
                                    content: format!(
                                        "**Session summary (Auto-Janitor):**\n\n{summary}"
                                    ),
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
                            self.awaiting_continuation = false;
                            self.current_round = 0;
                            self.status = AgentStatus::Idle;
                            break;
                        }
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
                        // ── Auto-Janitor threshold check ─────────────────────
                        // Require at least 2 completed rounds before auto-triggering:
                        // a single-round session has almost no history worth compressing.
                        if janitor_threshold > 0
                            && self.session_rounds >= 2
                            && self.total_session_prompt_tokens >= janitor_threshold
                        {
                            self.pending_janitor = true;
                        }
                        self.code_block_idx = 0;
                        self.mermaid_block_idx = 0;
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
                            images: vec![],
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
                self.messages.push(ChatMessage { role: Role::Assistant, content, images: vec![] });
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

    /// Returns `true` when the agent has modified or created at least one file
    /// this session and `SPC a u` can revert.
    pub fn has_checkpoint(&self) -> bool {
        !self.session_snapshots.is_empty() || !self.session_created_files.is_empty()
    }

    /// Restore all agent-touched files to their pre-session content and delete
    /// any files the agent created from scratch.
    ///
    /// Returns `(restored, deleted)` counts so the caller can build a status message.
    /// Clears both `session_snapshots` and `session_created_files` on completion.
    /// The caller should push `restored_paths` into `pending_reloads` so open
    /// buffers are refreshed.
    pub fn revert_session(&mut self, project_root: &std::path::Path) -> (Vec<String>, Vec<String>) {
        let mut restored = Vec::new();
        for (rel_path, original) in &self.session_snapshots {
            let abs = project_root.join(rel_path);
            if let Some(parent) = abs.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&abs, original) {
                Ok(()) => restored.push(rel_path.clone()),
                Err(e) => {
                    tracing::warn!("[checkpoint] failed to restore {rel_path}: {e}");
                },
            }
        }
        self.session_snapshots.clear();

        let mut deleted = Vec::new();
        for rel_path in &self.session_created_files {
            let abs = project_root.join(rel_path);
            match std::fs::remove_file(&abs) {
                Ok(()) => deleted.push(rel_path.clone()),
                Err(e) => {
                    tracing::warn!("[checkpoint] failed to delete created file {rel_path}: {e}");
                },
            }
        }
        self.session_created_files.clear();

        (restored, deleted)
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
