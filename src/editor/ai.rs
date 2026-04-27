use anyhow::{Context, Result};
use crossterm::{
    event::{KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tokio::sync::oneshot;

use super::Editor;
use crate::agent::ProviderKind;
use crate::keymap::Mode;
use crate::lsp::LspManager;

// =============================================================================
// Provider-aware one-shot completion
// =============================================================================

/// Non-streaming single-turn completion using the active provider.
///
/// Used for short generation tasks (commit messages, release notes) that don't
/// need the full agentic loop.  Acquires a Copilot token when needed; for Ollama
/// uses the OpenAI-compatible endpoint with no auth.
#[allow(clippy::too_many_arguments)]
async fn one_shot_with_provider(
    provider: &ProviderKind,
    ollama_base_url: &str,
    api_key: &str,
    openai_base_url: &str,
    openrouter_site_url: &str,
    openrouter_app_name: &str,
    model_id: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String> {
    let (api_token, endpoint, need_copilot_headers) = match provider {
        ProviderKind::Copilot => {
            let token = crate::agent::acquire_copilot_token().await?;
            (token, "https://api.githubcopilot.com/chat/completions".to_string(), true)
        },
        ProviderKind::Ollama => {
            (String::new(), format!("{ollama_base_url}/v1/chat/completions"), false)
        },
        ProviderKind::Anthropic => (
            api_key.to_string(),
            "https://api.anthropic.com/v1/chat/completions".to_string(),
            false,
        ),
        ProviderKind::OpenAi => {
            (api_key.to_string(), format!("{openai_base_url}/chat/completions"), false)
        },
        ProviderKind::Gemini => (
            api_key.to_string(),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions".to_string(),
            false,
        ),
        ProviderKind::OpenRouter => (
            api_key.to_string(),
            "https://openrouter.ai/api/v1/chat/completions".to_string(),
            false,
        ),
    };

    // For Ollama reasoning models (e.g. gemma4:e4b), disable the thinking/reasoning
    // chain for one-shot tasks — it adds minutes of latency for no benefit on simple,
    // well-defined prompts like commit message generation.
    let body = if matches!(provider, ProviderKind::Ollama) {
        serde_json::json!({
            "model": model_id,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user",   "content": user   }
            ],
            "stream": false,
            "temperature": 0.3,
            "max_tokens": max_tokens,
            "think": false
        })
    } else {
        serde_json::json!({
            "model": model_id,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user",   "content": user   }
            ],
            "stream": false,
            "temperature": 0.3,
            "max_tokens": max_tokens
        })
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_default();

    let mut req = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "forgiven/0.1.0");

    if !api_token.is_empty() {
        req = req.header("Authorization", format!("Bearer {api_token}"));
    }
    if need_copilot_headers {
        req = req
            .header("Copilot-Integration-Id", "vscode-chat")
            .header("editor-version", "forgiven/0.1.0")
            .header("editor-plugin-version", "forgiven-copilot/0.1.0")
            .header("openai-intent", "conversation-panel");
    }
    if matches!(provider, ProviderKind::OpenRouter) {
        if !openrouter_site_url.is_empty() {
            req = req.header("HTTP-Referer", openrouter_site_url);
        }
        if !openrouter_app_name.is_empty() {
            req = req.header("X-Title", openrouter_app_name);
        }
    }

    tracing::info!(provider = %provider.display_name(), model = model_id, "one_shot request sending");

    let resp =
        req.json(&body).send().await.map_err(|e| anyhow::anyhow!("one_shot_with_provider: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        tracing::warn!(provider = %provider.display_name(), %status, "one_shot API error: {text}");
        return Err(anyhow::anyhow!("{} API error ({status}): {text}", provider.display_name()));
    }

    let val: serde_json::Value =
        resp.json().await.context("one_shot_with_provider: response not JSON")?;
    tracing::debug!("one_shot raw response: {val}");
    let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
    if content.is_empty() {
        tracing::warn!(provider = %provider.display_name(), model = model_id, "one_shot returned empty content; full response: {val}");
    } else {
        tracing::info!(provider = %provider.display_name(), model = model_id, len = content.len(), "one_shot completed");
    }
    Ok(content)
}

// =============================================================================
// Free functions
// =============================================================================

/// Strip JSON wrapping from a model-generated commit message.
///
/// Some models (e.g. qwen via Ollama) ignore the plain-text instruction and
/// return JSON like `{"commit_message": "..."}` or wrap the output in a
/// markdown code fence. This function attempts to recover the plain-text
/// message, falling back to the raw string if it cannot.
pub(super) fn strip_json_commit_msg(raw: &str) -> String {
    // 1. Strip surrounding markdown code fences (```json ... ``` or ``` ... ```)
    let stripped = {
        let trimmed = raw.trim();
        let inner = if let Some(rest) = trimmed.strip_prefix("```") {
            // skip the optional language tag on the opening fence line
            let after_tag = rest.trim_start_matches(|c: char| c.is_alphabetic());
            if let Some(body) =
                after_tag.strip_prefix('\n').or_else(|| after_tag.strip_prefix('\r'))
            {
                body.trim_end_matches("```").trim()
            } else {
                trimmed
            }
        } else {
            trimmed
        };
        inner
    };

    // 2. Try to parse as JSON and extract common commit-message fields.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(stripped) {
        // Try single-field variants first
        for key in &["commit_message", "message", "commit_msg", "text", "content", "response"] {
            if let Some(s) = val[key].as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        // subject + optional body
        if let Some(subject) = val["subject"].as_str().filter(|s| !s.trim().is_empty()) {
            if let Some(body) = val["body"].as_str().filter(|s| !s.trim().is_empty()) {
                return format!("{}\n\n{}", subject.trim(), body.trim());
            }
            return subject.trim().to_string();
        }
        // Last resort: pick the longest non-empty string value from any top-level key.
        if let Some(obj) = val.as_object() {
            if let Some(best) = obj
                .values()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .max_by_key(|s| s.len())
            {
                return best.to_string();
            }
        }
    }

    // 3. Not JSON (or no recognised field) — return stripped plain text.
    stripped.to_string()
}

/// Fix unquoted node labels containing parentheses in Mermaid source.
///
/// Mermaid breaks when a square-bracket label contains `(` or `)` without
/// surrounding quotes.  AI models frequently generate this form, e.g.:
///   `K[UseHttpMetrics (Prometheus)]`
///
/// The fix is to wrap the label in double-quotes:
///   `K["UseHttpMetrics (Prometheus)"]`
///
/// This function scans each line character-by-character and applies the
/// transformation only to `[…]` groups that (a) contain `(` or `)` and
/// (b) are not already quoted.
pub(super) fn fix_mermaid_parens(source: &str) -> String {
    let mut result = String::with_capacity(source.len() + 64);
    for line in source.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '[' {
                // Find the matching `]`, tracking depth.
                let start = i;
                let mut j = i + 1;
                let mut depth = 1usize;
                while j < chars.len() && depth > 0 {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {},
                    }
                    j += 1;
                }
                if depth == 0 {
                    // chars[start+1 .. j-1] is the inner label content.
                    let inner: String = chars[start + 1..j - 1].iter().collect();
                    if (inner.contains('(') || inner.contains(')')) && !inner.starts_with('"') {
                        result.push('[');
                        result.push('"');
                        result.push_str(&inner);
                        result.push('"');
                        result.push(']');
                    } else {
                        result.extend(chars[start..j].iter());
                    }
                    i = j;
                    continue;
                }
            }
            result.push(chars[i]);
            i += 1;
        }
        result.push('\n');
    }
    // Preserve original trailing-newline behaviour.
    if !source.ends_with('\n') {
        result.pop();
    }
    result
}

/// Strip a wrapping code fence that the AI may add despite being told not to.
/// Handles ` ```markdown `, ` ```md `, and plain ` ``` ` opening fences.
pub(super) fn strip_markdown_fence(s: &str) -> String {
    let trimmed = s.trim();
    let after_open = trimmed
        .strip_prefix("```markdown")
        .or_else(|| trimmed.strip_prefix("```md"))
        .or_else(|| trimmed.strip_prefix("```"));
    if let Some(rest) = after_open {
        // Strip the newline immediately after the opening fence, then the closing fence.
        let body = rest.strip_prefix('\n').unwrap_or(rest);
        if let Some(stripped) = body.strip_suffix("```") {
            return stripped.trim_end().to_string();
        }
    }
    trimmed.to_string()
}

// =============================================================================
// impl Editor — AI / git integration methods
// =============================================================================

impl Editor {
    /// Send a `textDocument/inlineCompletion` request to any available LSP client.
    /// Tries the file's language client first, then falls back to "copilot".
    pub(super) fn request_inline_completion(&mut self) {
        let buf = match self.current_buffer() {
            Some(b) => b,
            None => return,
        };
        let path = match buf.file_path.clone() {
            Some(p) => p,
            None => return,
        };
        let row = buf.cursor.row as u32;
        let col = buf.cursor.col as u32;

        let uri = match LspManager::path_to_uri(&path) {
            Ok(u) => u,
            Err(_) => return,
        };

        // Always prefer the "copilot" client for inline completions — language servers
        // like rust-analyzer don't support textDocument/inlineCompletion.
        // Fall back to the file-language client only if no Copilot client is registered.
        // Two separate lookups avoid a double-mutable-borrow on lsp_manager.
        let language = LspManager::language_from_path(&path);
        let has_copilot = self.lsp.manager.get_client("copilot").is_some();
        let client = if has_copilot {
            self.lsp.manager.get_client("copilot")
        } else {
            self.lsp.manager.get_client(&language)
        };

        if let Some(client) = client {
            match client.inline_completion(&uri, row, col) {
                Ok(rx) => self.pending_completion = Some(rx),
                Err(e) => tracing::debug!("inline_completion request failed: {}", e),
            }
        }
    }

    /// Suspend the TUI, open lazygit, then restore the TUI.
    ///
    /// The suspend/resume pattern:
    ///   1. Leave alternate screen + disable raw mode  → terminal is back to normal
    ///   2. Spawn `lazygit` as a child process with inherited stdio  → lazygit takes over
    ///   3. Wait for lazygit to exit
    ///   4. Re-enter alternate screen + re-enable raw mode  → our TUI resumes
    ///   5. Force a full redraw so no lazygit artifacts remain
    ///   6. Reload every open buffer from disk (git ops may have changed files)
    pub(super) fn open_lazygit(&mut self) -> Result<()> {
        // ── Suspend TUI ──────────────────────────────────────────────────────
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;

        // ── Run lazygit ───────────────────────────────────────────────────────
        // lazygit is itself a full-screen TUI; it inherits our stdin/stdout/stderr.
        let result = std::process::Command::new("lazygit").status();

        // ── Restore TUI (always, even if lazygit errored) ─────────────────────
        enable_raw_mode()?;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        // Force ratatui to repaint every cell — clears any lazygit residue.
        self.terminal.clear()?;

        // ── Handle outcome ────────────────────────────────────────────────────
        match result {
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.set_status(
                    "lazygit not found — install it (e.g. brew install lazygit)".to_string(),
                );
            },
            Err(e) => {
                self.set_status(format!("lazygit error: {e}"));
            },
            Ok(_) => {
                // Reload every open buffer — a git pull/checkout/rebase may have
                // changed files on disk that are currently open in the editor.
                for buf in &mut self.buffers {
                    if buf.file_path.is_some() {
                        let _ = buf.reload_from_disk();
                    }
                }
                self.set_status("Returned from lazygit".to_string());
            },
        }

        Ok(())
    }

    // ── Commit-message generation ──────────────────────────────────────────────

    /// Kick off a background AI task to generate a commit message.
    /// `from_staged = true`  → use `git diff --staged`
    /// `from_staged = false` → use `git show HEAD --stat -p`
    pub(super) fn start_commit_msg(&mut self, from_staged: bool) {
        // Run the git command synchronously (it's fast).
        let diff_cmd = if from_staged {
            std::process::Command::new("git").args(["diff", "--staged"]).output()
        } else {
            std::process::Command::new("git").args(["show", "HEAD", "--stat", "-p"]).output()
        };

        let diff_text = match diff_cmd {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
            Err(e) => {
                self.set_status(format!("git error: {e}"));
                return;
            },
        };

        if diff_text.trim().is_empty() {
            let msg = if from_staged {
                "No staged changes — stage files first (git add)".to_string()
            } else {
                "No commits found".to_string()
            };
            self.set_status(msg);
            return;
        }

        let model_id = self
            .agent_panel
            .selected_model_id_with_fallback(self.config.active_default_model())
            .to_string();
        let provider_kind = self.agent_panel.provider.clone();
        let ollama_base_url = self.agent_panel.ollama_base_url.clone();
        let api_key = self.agent_panel.api_key.clone();
        let openai_base_url = self.agent_panel.openai_base_url.clone();
        let openrouter_site_url = self.agent_panel.openrouter_site_url.clone();
        let openrouter_app_name = self.agent_panel.openrouter_app_name.clone();

        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let system = "You are a concise git commit message writer. \
                Given a git diff, output ONLY a single commit message: \
                a short subject line (≤72 chars, imperative mood), then a blank line, \
                then an optional bullet-point body with the key changes. \
                No preamble, no explanation, just the commit message.";
            let user = format!("Write a commit message for this diff:\n\n```\n{diff_text}\n```");
            let result = one_shot_with_provider(
                &provider_kind,
                &ollama_base_url,
                &api_key,
                &openai_base_url,
                &openrouter_site_url,
                &openrouter_app_name,
                &model_id,
                system,
                &user,
                2048,
            )
            .await
            .map(|s| strip_json_commit_msg(&s));
            let _ = tx.send(result);
        });

        self.commit_msg.rx = Some(rx);
        self.commit_msg.from_staged = from_staged;
        self.commit_msg.buffer = String::new();
        self.mode = Mode::CommitMsg;
        self.set_status("Generating commit message…".to_string());
    }

    /// Handle key events while in Mode::CommitMsg.
    pub(super) fn handle_commit_msg_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // Esc — discard
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.commit_msg.buffer.clear();
                self.commit_msg.cursor = 0;
                self.commit_msg.rx = None;
                self.set_status("Commit message discarded".to_string());
            },
            // Enter — commit with the current message
            KeyCode::Enter => {
                let msg = self.commit_msg.buffer.trim().to_string();
                if msg.is_empty() {
                    self.set_status("Commit message is empty".to_string());
                    return Ok(());
                }
                let out = std::process::Command::new("git").args(["commit", "-m", &msg]).output();
                self.mode = Mode::Normal;
                self.commit_msg.buffer.clear();
                self.commit_msg.cursor = 0;
                self.commit_msg.rx = None;
                match out {
                    Ok(o) if o.status.success() => {
                        self.set_status("Committed successfully".to_string());
                    },
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr).to_string();
                        self.set_status(format!("git commit failed: {err}"));
                    },
                    Err(e) => self.set_status(format!("git error: {e}")),
                }
            },
            // Backspace — delete char before cursor
            KeyCode::Backspace => {
                let pos = self.commit_msg.cursor;
                if pos > 0 {
                    // Find the start of the previous char (handle multi-byte UTF-8)
                    let prev = self.commit_msg.buffer[..pos]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.commit_msg.buffer.remove(prev);
                    self.commit_msg.cursor = prev;
                }
            },
            // Left arrow — move cursor one char left
            KeyCode::Left => {
                let pos = self.commit_msg.cursor;
                if pos > 0 {
                    let prev = self.commit_msg.buffer[..pos]
                        .char_indices()
                        .last()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.commit_msg.cursor = prev;
                }
            },
            // Right arrow — move cursor one char right
            KeyCode::Right => {
                let pos = self.commit_msg.cursor;
                if pos < self.commit_msg.buffer.len() {
                    let next = self.commit_msg.buffer[pos..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| pos + i)
                        .unwrap_or(self.commit_msg.buffer.len());
                    self.commit_msg.cursor = next;
                }
            },
            // Home — jump to beginning
            KeyCode::Home => {
                self.commit_msg.cursor = 0;
            },
            // End — jump to end
            KeyCode::End => {
                self.commit_msg.cursor = self.commit_msg.buffer.len();
            },
            // Regular characters — insert at cursor
            KeyCode::Char(ch) => {
                let pos = self.commit_msg.cursor;
                self.commit_msg.buffer.insert(pos, ch);
                self.commit_msg.cursor += ch.len_utf8();
            },
            _ => {},
        }
        Ok(())
    }

    /// Enter `Mode::ReleaseNotes` — count-input phase.
    pub(super) fn start_release_notes(&mut self) {
        self.release_notes.buffer.clear();
        self.release_notes.count_input = String::from("10");
        self.release_notes.scroll = 0;
        self.mode = Mode::ReleaseNotes;
        self.set_status(
            "Enter number of commits (default 10) then press Enter to generate release notes"
                .to_string(),
        );
    }

    /// Run git log, spawn AI task, transition to generating phase.
    pub(super) fn trigger_release_notes_generation(&mut self) {
        let count: usize =
            self.release_notes.count_input.trim().parse::<usize>().unwrap_or(10).clamp(1, 200);

        let log_output = std::process::Command::new("git")
            .args(["log", "--format=%H%n%s%n%b%n---", &format!("-{count}")])
            .output();

        let log_text = match log_output {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
            Err(e) => {
                self.mode = Mode::Normal;
                self.set_status(format!("git error: {e}"));
                return;
            },
        };

        if log_text.trim().is_empty() {
            self.set_status("No commits found".to_string());
            return;
        }

        let model_id = self
            .agent_panel
            .selected_model_id_with_fallback(self.config.active_default_model())
            .to_string();
        let provider_kind = self.agent_panel.provider.clone();
        let ollama_base_url = self.agent_panel.ollama_base_url.clone();
        let api_key = self.agent_panel.api_key.clone();
        let openai_base_url = self.agent_panel.openai_base_url.clone();
        let openrouter_site_url = self.agent_panel.openrouter_site_url.clone();
        let openrouter_app_name = self.agent_panel.openrouter_app_name.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let system = "You are a technical writer creating release notes for a software project. \
                Given a list of git commits, produce clean, user-friendly release notes in markdown. \
                Group related changes under headings like '### Features', '### Bug Fixes', '### Improvements'. \
                Be concise but descriptive. Omit merge commits and trivial chore changes. \
                Output markdown only — no preamble, no explanation.";
            let user = format!(
                "Generate release notes from these {count} commits:\n\n```\n{log_text}\n```"
            );
            let result = one_shot_with_provider(
                &provider_kind,
                &ollama_base_url,
                &api_key,
                &openai_base_url,
                &openrouter_site_url,
                &openrouter_app_name,
                &model_id,
                system,
                &user,
                4096,
            )
            .await;
            let _ = tx.send(result);
        });

        self.release_notes.rx = Some(rx);
        self.set_status(format!("Generating release notes from {count} commits…"));
    }

    /// Handle key events while in `Mode::ReleaseNotes`.
    pub(super) fn handle_release_notes_mode(&mut self, key: KeyEvent) -> Result<()> {
        // Phase 2: generating — only Esc cancels.
        if self.release_notes.rx.is_some() {
            if key.code == KeyCode::Esc {
                self.release_notes.rx = None;
                self.mode = Mode::Normal;
                self.set_status("Release notes cancelled".to_string());
            }
            return Ok(());
        }

        // Phase 3: displaying the result.
        if !self.release_notes.buffer.is_empty() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                    self.release_notes.buffer.clear();
                    self.release_notes.scroll = 0;
                    self.set_status("Release notes closed".to_string());
                },
                KeyCode::Char('y') => {
                    let text = self.release_notes.buffer.clone();
                    self.sync_system_clipboard(&text);
                    self.set_status("Release notes copied to clipboard".to_string());
                },
                KeyCode::Char('j') | KeyCode::Down => {
                    self.release_notes.scroll = self.release_notes.scroll.saturating_add(1);
                },
                KeyCode::Char('k') | KeyCode::Up => {
                    self.release_notes.scroll = self.release_notes.scroll.saturating_sub(1);
                },
                _ => {},
            }
            return Ok(());
        }

        // Phase 1: count-entry input.
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.release_notes.count_input = String::from("10");
                self.set_status("Cancelled".to_string());
            },
            KeyCode::Enter => {
                self.trigger_release_notes_generation();
            },
            KeyCode::Backspace => {
                self.release_notes.count_input.pop();
            },
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                self.release_notes.count_input.push(ch);
            },
            _ => {},
        }
        Ok(())
    }

    /// Render the current buffer as HTML and open it in the system browser.
    ///
    /// Writes a self-contained HTML file to the OS temp directory and spawns
    /// the platform opener (`open` on macOS, `xdg-open` on Linux).  The opener
    /// runs detached — the TUI stays alive and no suspend/restore is needed.
    pub(super) fn open_markdown_in_browser(&mut self) {
        let content = match self.current_buffer() {
            Some(buf) => buf.lines().join("\n"),
            None => {
                self.set_status("No buffer open".to_string());
                return;
            },
        };

        let file_stem = self
            .current_buffer()
            .and_then(|b| b.file_path.as_ref())
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("preview")
            .to_string();

        // ── If the file is already HTML, open it directly ─────────────────────
        let is_html = self
            .current_buffer()
            .and_then(|b| b.file_path.as_ref())
            .and_then(|p| p.extension())
            .map(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
            .unwrap_or(false);

        if is_html {
            let path = std::env::temp_dir().join(format!("forgiven_{file_stem}.html"));
            if let Err(e) = std::fs::write(&path, &content) {
                self.set_status(format!("Failed to write temp file: {e}"));
                return;
            }
            #[cfg(target_os = "macos")]
            let opener = "open";
            #[cfg(target_os = "linux")]
            let opener = "xdg-open";
            #[cfg(target_os = "windows")]
            let opener = "explorer";
            match std::process::Command::new(opener).arg(&path).spawn() {
                Ok(_) => self.set_status(format!("Opened {file_stem}.html in browser")),
                Err(e) => self.set_status(format!("Failed to open browser: {e}")),
            }
            return;
        }

        // ── Render markdown → HTML body ───────────────────────────────────────
        let parser = pulldown_cmark::Parser::new_ext(&content, pulldown_cmark::Options::all());
        let mut body = String::new();
        pulldown_cmark::html::push_html(&mut body, parser);

        // ── Wrap in a minimal, readable HTML page ─────────────────────────────
        // The script converts pulldown-cmark's  <pre><code class="language-mermaid">
        // output into <div class="mermaid"> elements, then loads Mermaid.js to
        // render them.
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  body {{
    max-width: 720px;
    margin: 0 auto;
    padding: 64px 32px 96px;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    font-size: 16px;
    line-height: 1.75;
    color: #1a1a1a;
    background: #fff;
    -webkit-font-smoothing: antialiased;
  }}
  h1, h2, h3, h4, h5, h6 {{
    font-weight: 600;
    line-height: 1.25;
    margin: 2em 0 0.5em;
    color: #111;
  }}
  h1 {{ font-size: 2em; border-bottom: 2px solid #e8e8e8; padding-bottom: 0.3em; margin-top: 0; }}
  h2 {{ font-size: 1.4em; border-bottom: 1px solid #e8e8e8; padding-bottom: 0.25em; }}
  h3 {{ font-size: 1.15em; }}
  p  {{ margin: 0 0 1.25em; }}
  a  {{ color: #0969da; text-decoration: none; }}
  a:hover {{ text-decoration: underline; }}
  strong {{ font-weight: 600; }}
  em {{ font-style: italic; }}
  hr {{ border: none; border-top: 1px solid #e8e8e8; margin: 2.5em 0; }}
  ul, ol {{ padding-left: 1.5em; margin: 0 0 1.25em; }}
  li {{ margin: 0.3em 0; }}
  li + li {{ margin-top: 0.25em; }}
  blockquote {{
    border-left: 3px solid #d0d0d0;
    margin: 1.5em 0;
    padding: 0.25em 0 0.25em 1.25em;
    color: #555;
  }}
  blockquote p {{ margin-bottom: 0; }}
  code {{
    font-family: "SFMono-Regular", "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.875em;
    background: #f3f3f3;
    padding: 0.15em 0.35em;
    border-radius: 3px;
    color: #d63384;
  }}
  pre {{
    background: #f6f6f6;
    border: 1px solid #e8e8e8;
    border-radius: 6px;
    padding: 1em 1.25em;
    overflow-x: auto;
    margin: 0 0 1.5em;
    line-height: 1.5;
  }}
  pre code {{
    background: none;
    padding: 0;
    border-radius: 0;
    font-size: 0.85em;
    color: inherit;
  }}
  img {{ max-width: 100%; height: auto; border-radius: 4px; }}
  table {{ border-collapse: collapse; width: 100%; margin: 0 0 1.5em; }}
  th, td {{ border: 1px solid #e0e0e0; padding: 0.5em 0.75em; text-align: left; }}
  th {{ background: #f6f6f6; font-weight: 600; }}
  tr:nth-child(even) {{ background: #fafafa; }}
</style>
</head>
<body>
{body}
<script src="https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js"></script>
<script>
  document.querySelectorAll('pre > code.language-mermaid').forEach(function(code) {{
    var div = document.createElement('div');
    div.className = 'mermaid';
    div.textContent = code.textContent;
    code.parentNode.replaceWith(div);
  }});
  mermaid.initialize({{ startOnLoad: false }});
  mermaid.run();
</script>
</body>
</html>"#,
            title = file_stem,
            body = body,
        );

        // ── Write temp file ───────────────────────────────────────────────────
        let path = std::env::temp_dir().join(format!("forgiven_{file_stem}.html"));
        if let Err(e) = std::fs::write(&path, &html) {
            self.set_status(format!("Failed to write temp file: {e}"));
            return;
        }

        // ── Spawn platform opener (detached) ──────────────────────────────────
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(target_os = "linux")]
        let opener = "xdg-open";
        #[cfg(target_os = "windows")]
        let opener = "explorer";

        match std::process::Command::new(opener).arg(&path).spawn() {
            Ok(_) => self.set_status(format!("Opened in browser: {}", path.display())),
            Err(e) => self.set_status(format!("Failed to open browser: {e}")),
        }
    }

    /// Extract the current (or next) mermaid diagram from the last agent reply,
    /// auto-fix parentheses in node labels, write a self-contained HTML file to
    /// the system temp directory, and open it in the default browser.
    pub(super) fn open_mermaid_in_browser(&mut self) {
        let reply = match self.agent_panel.last_assistant_reply() {
            Some(r) => r,
            None => {
                self.set_status("No agent reply to render".to_string());
                return;
            },
        };

        let blocks = crate::agent::AgentPanel::extract_mermaid_blocks(&reply);
        if blocks.is_empty() {
            self.set_status("No mermaid blocks in last reply".to_string());
            return;
        }

        let idx = self.agent_panel.mermaid_block_idx % blocks.len();
        let source = fix_mermaid_parens(&blocks[idx]);
        self.agent_panel.mermaid_block_idx =
            (self.agent_panel.mermaid_block_idx + 1) % blocks.len();

        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Mermaid diagram {num}/{total}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  body {{
    margin: 0;
    padding: 2rem;
    background: #1e1e2e;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    min-height: 100vh;
  }}
  .mermaid {{ max-width: 100%; }}
</style>
</head>
<body>
<pre class="mermaid">
{source}
</pre>
<script type="module">
  import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs';
  mermaid.initialize({{ startOnLoad: true, theme: 'dark' }});
</script>
</body>
</html>"#,
            num = idx + 1,
            total = blocks.len(),
            source = source,
        );

        let path = std::env::temp_dir().join(format!("forgiven_mermaid_{}.html", idx + 1));
        if let Err(e) = std::fs::write(&path, &html) {
            self.set_status(format!("Failed to write mermaid file: {e}"));
            return;
        }

        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(target_os = "linux")]
        let opener = "xdg-open";
        #[cfg(target_os = "windows")]
        let opener = "explorer";

        match std::process::Command::new(opener).arg(&path).spawn() {
            Ok(_) => self.set_status(format!(
                "Mermaid diagram {}/{} opened in browser  (Ctrl+M for next)",
                idx + 1,
                blocks.len()
            )),
            Err(e) => self.set_status(format!("Failed to open browser: {e}")),
        }
    }

    // ==========================================================================
    // Insights narrative (Phase 4, ADR 0129)
    // ==========================================================================

    /// Build an analysis prompt from `AggregatedInsights` for the LLM narrative.
    fn build_narrative_prompt(
        insights: &crate::insights::aggregator::AggregatedInsights,
        history_snippets: &str,
    ) -> String {
        let log = &insights.log;
        let ses = &insights.sessions;
        let total_requests = log.llm_request_count + log.one_shot_count;
        let date_range = match (&log.first_date, &log.last_date) {
            (Some(f), Some(l)) if f == l => f.clone(),
            (Some(f), Some(l)) => format!("{f} to {l}"),
            _ => "unknown date range".to_string(),
        };

        let mut prompt = format!(
            "You are analysing a developer's AI-assisted coding session history for the \
             Forgiven editor. Produce a concise qualitative narrative (under 400 words) \
             with exactly three sections:\n\
             ## What's working well\n\
             ## What's hindering progress\n\
             ## Quick wins\n\n\
             Base your analysis on these collaboration statistics:\n\n\
             - Date range: {date_range}\n\
             - Active days: {active_days}\n\
             - Editor sessions: {sessions}\n\
             - Total LLM requests: {total_requests}\n\
             - Agentic rounds: {agentic}\n\
             - Chat-only rounds: {chat_only}\n\
             - One-shot calls: {one_shot}\n\
             - Buffer saves: {saves}\n\
             - Log warnings: {warns}, errors: {errors}\n",
            active_days = log.active_days,
            sessions = log.session_count,
            agentic = log.llm_request_count.saturating_sub(log.chat_only_count),
            chat_only = log.chat_only_count,
            one_shot = log.one_shot_count,
            saves = log.buffer_save_count,
            warns = log.warn_count,
            errors = log.error_count,
        );

        if !ses.sessions.is_empty() {
            prompt.push_str(&format!(
                "- Recorded sessions (JSONL): {count}\n\
                 - Avg rounds/session: {avg_rounds:.1}\n\
                 - Avg files changed/session: {avg_files:.1}\n\
                 - Total prompt tokens: {pt}\n\
                 - Total completion tokens: {ct}\n\
                 - Tool errors recorded: {tool_errs}\n",
                count = ses.sessions.len(),
                avg_rounds = ses.avg_rounds(),
                avg_files = ses.avg_files(),
                pt = ses.total_prompt_tokens,
                ct = ses.total_completion_tokens,
                tool_errs = ses.tool_errors.len(),
            ));
        }

        if !insights.log.models.is_empty() {
            let mut models: Vec<_> = insights.log.models.iter().collect();
            models.sort_by(|a, b| b.1.cmp(a.1));
            let model_list: Vec<String> =
                models.iter().take(5).map(|(m, c)| format!("{m} ({c})")).collect();
            prompt.push_str(&format!("- Top models used: {}\n", model_list.join(", ")));
        }

        if !history_snippets.is_empty() {
            prompt.push_str(&format!(
                "\nRecent session sample (last messages, truncated):\n```\n{history_snippets}\n```\n"
            ));
        }

        prompt.push_str(
            "\nOutput only the three markdown sections. Be specific and actionable, not generic.",
        );
        prompt
    }

    /// Kick off an async LLM call to generate the insights narrative.
    ///
    /// Reads up to `max_history_files` recent history JSONL files to extract a
    /// representative sample of user messages, then sends the combined prompt
    /// through `one_shot_with_provider`. The result is delivered via
    /// `self.insights_narrative_rx` and polled in the event loop.
    pub(super) fn generate_insights_narrative(&mut self, max_history_files: usize) {
        let data_dir = match crate::config::Config::log_path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            Some(d) => d,
            None => {
                self.set_status("Cannot resolve data directory for insights".to_string());
                return;
            },
        };

        let insights = crate::insights::build_insights(&data_dir);

        // Collect a sample of recent user messages from history JSONL files.
        let history_dir = data_dir.join("history");
        let history_snippets = Self::sample_history(&history_dir, max_history_files);

        let user_prompt = Self::build_narrative_prompt(&insights, &history_snippets);
        let system = "You are a concise technical analyst. \
            Output only valid markdown with exactly the three requested sections.";

        let model_id = self
            .agent_panel
            .selected_model_id_with_fallback(self.config.active_default_model())
            .to_string();
        let provider_kind = self.agent_panel.provider.clone();
        let ollama_base_url = self.agent_panel.ollama_base_url.clone();
        let api_key = self.agent_panel.api_key.clone();
        let openai_base_url = self.agent_panel.openai_base_url.clone();
        let openrouter_site_url = self.agent_panel.openrouter_site_url.clone();
        let openrouter_app_name = self.agent_panel.openrouter_app_name.clone();

        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = one_shot_with_provider(
                &provider_kind,
                &ollama_base_url,
                &api_key,
                &openai_base_url,
                &openrouter_site_url,
                &openrouter_app_name,
                &model_id,
                system,
                &user_prompt,
                1024,
            )
            .await;
            let _ = tx.send(result);
        });

        self.insights_narrative_rx = Some(rx);
        self.set_status("Generating insights narrative…".to_string());
    }

    /// Read up to `max_files` recent history JSONL files and extract a short
    /// sample of user message text (first 120 chars per message, max 30 messages).
    fn sample_history(history_dir: &std::path::Path, max_files: usize) -> String {
        let Ok(mut entries) = std::fs::read_dir(history_dir) else { return String::new() };
        let mut files: Vec<std::path::PathBuf> =
            entries.by_ref().filter_map(|e| e.ok().map(|e| e.path())).collect();
        // Sort descending (newest first) — file names are Unix timestamps.
        files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        files.truncate(max_files);

        let mut snippets: Vec<String> = Vec::new();
        'outer: for path in &files {
            let Ok(content) = std::fs::read_to_string(path) else { continue };
            for line in content.lines() {
                if snippets.len() >= 30 {
                    break 'outer;
                }
                // Extract role and content cheaply without full deserialisation.
                if line.contains("\"role\":\"user\"") || line.contains("\"role\": \"user\"") {
                    if let Some(start) =
                        line.find("\"content\":\"").or_else(|| line.find("\"content\": \""))
                    {
                        let rest = &line[start..];
                        if let Some(inner) = rest.find('"').and_then(|i| rest.get(i + 1..)) {
                            let snippet: String = inner.chars().take(120).collect();
                            snippets.push(snippet);
                        }
                    }
                }
            }
        }
        snippets.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::strip_json_commit_msg;

    #[test]
    fn plain_text_passes_through() {
        let msg = "Fix render bug\n\n- rewrote cursor calc";
        assert_eq!(strip_json_commit_msg(msg), msg);
    }

    #[test]
    fn known_commit_message_key() {
        let raw = r#"{"commit_message": "Add feature X"}"#;
        assert_eq!(strip_json_commit_msg(raw), "Add feature X");
    }

    #[test]
    fn response_key_extracted() {
        let raw = r#"{"response": "Fix cursor position\n\n- walk byte offsets correctly"}"#;
        assert_eq!(
            strip_json_commit_msg(raw),
            "Fix cursor position\n\n- walk byte offsets correctly"
        );
    }

    #[test]
    fn pretty_printed_response_key() {
        let raw = "{\n  \"response\": \"Refactor state module\"\n}";
        assert_eq!(strip_json_commit_msg(raw), "Refactor state module");
    }

    #[test]
    fn unknown_key_falls_back_to_longest_value() {
        let raw = r#"{"note": "short", "summary": "A longer commit message here"}"#;
        assert_eq!(strip_json_commit_msg(raw), "A longer commit message here");
    }

    #[test]
    fn markdown_fence_stripped_before_json_parse() {
        let raw = "```json\n{\"message\": \"chore: bump version\"}\n```";
        assert_eq!(strip_json_commit_msg(raw), "chore: bump version");
    }

    #[test]
    fn subject_and_body_combined() {
        let raw = r#"{"subject": "Add fold cache", "body": "- reduces per-frame allocs"}"#;
        assert_eq!(strip_json_commit_msg(raw), "Add fold cache\n\n- reduces per-frame allocs");
    }
}
