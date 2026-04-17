use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use tokio::sync::oneshot;

use super::ai::strip_markdown_fence;
use super::Editor;
use crate::keymap::Mode;
use crate::lsp::{parse_first_inline_completion, LspManager};
use crate::search::SearchStatus;

impl Editor {
    /// Main event loop
    pub async fn run(&mut self) -> Result<()> {
        const COMPLETION_DEBOUNCE_MS: u128 = 300;

        // Render on the very first frame regardless of activity.
        let mut needs_render = true;
        // Set to true whenever the terminal cell grid may be stale (resize, SIGCONT, Ctrl+L).
        // A full terminal clear is issued before the next render to force a repaint.
        let mut force_clear = false;

        // ── SIGCONT: laptop-lid-open / process-resume repaint ─────────────────
        // When the OS suspends and resumes a process it sends SIGCONT.  The
        // terminal has already forgotten our screen contents, so we must clear
        // and repaint everything.  Tokio's signal module is already available
        // (tokio full feature); no extra dependency is needed.
        #[cfg(unix)]
        let (sigcont_tx, mut sigcont_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        #[cfg(unix)]
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            // SIGCONT = 18 on Linux and macOS.
            if let Ok(mut sig) = signal(SignalKind::from_raw(18)) {
                loop {
                    sig.recv().await;
                    if sigcont_tx.send(()).is_err() {
                        break;
                    }
                }
            }
        });

        loop {
            // ── LSP: process incoming notifications / responses ────────────────
            let lsp_changed = self.lsp_manager.process_messages().unwrap_or(false);
            if lsp_changed {
                needs_render = true;
            }

            // Surface any human-readable LSP messages (e.g. Copilot auth instructions).
            // These are sticky so they persist until the user presses Esc.
            for msg in self.lsp_manager.drain_messages() {
                self.set_sticky(msg);
                needs_render = true;
            }

            // Update diagnostics for current buffer — only when LSP sent something new
            // to avoid cloning the full diagnostic Vec on every frame.
            if lsp_changed {
                if let Some(buf) = self.current_buffer() {
                    if let Some(path) = &buf.file_path {
                        if let Ok(uri) = LspManager::path_to_uri(path) {
                            self.current_diagnostics = self.lsp_manager.get_diagnostics(&uri);
                        }
                    }
                }
            }

            // ── Copilot auth polling ───────────────────────────────────────────
            let auth_done = if let Some(rx) = self.copilot_auth_rx.as_mut() {
                match rx.try_recv() {
                    Ok(val) => Some(val),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(serde_json::Value::Null),
                }
            } else {
                None
            };
            if let Some(val) = auth_done {
                self.copilot_auth_rx = None;
                needs_render = true;
                let status = val.get("status").and_then(|s| s.as_str()).unwrap_or("");
                tracing::info!("Copilot auth response: {:?}", val);
                match status {
                    "OK" | "AlreadySignedIn" => {
                        let user = val.get("user").and_then(|u| u.as_str()).unwrap_or("unknown");
                        self.set_sticky(format!("Copilot: signed in as {}", user));
                    },
                    "NotSignedIn" => {
                        // Auto-escalate: start the device auth flow
                        if let Some(client) = self.lsp_manager.get_client("copilot") {
                            match client.copilot_sign_in_initiate() {
                                Ok(rx) => {
                                    self.copilot_auth_rx = Some(rx);
                                },
                                Err(e) => {
                                    self.set_sticky(format!("Copilot sign-in failed: {}", e));
                                },
                            }
                        }
                    },
                    "PromptUserDeviceFlow" => {
                        let uri =
                            val.get("verificationUri").and_then(|u| u.as_str()).unwrap_or("?");
                        let code = val.get("userCode").and_then(|c| c.as_str()).unwrap_or("?");
                        self.set_sticky(format!(
                            "Copilot auth: go to {}  and enter code: {}  (Esc to dismiss)",
                            uri, code
                        ));
                    },
                    _ => {
                        self.set_sticky(format!("Copilot: {}", val));
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Agent panel stream polling ─────────────────────────────────────
            let agent_active = self.agent_panel.poll_stream();
            if let Some(err) = self.agent_panel.last_error.take() {
                self.set_status(format!("Agent error: {err}"));
            }
            // Clear the hook re-entry guard once the agent goes idle.
            if self.hooks_firing && self.agent_panel.status == crate::agent::AgentStatus::Idle {
                self.hooks_firing = false;
            }
            if agent_active {
                // Rate-limit agent-only renders to ≤10 Hz (100 ms between frames).
                // If another source (keyboard, watcher) already set `needs_render`
                // we render immediately; the cap only kicks in when streaming is
                // the sole reason to repaint.
                const AGENT_RENDER_INTERVAL: std::time::Duration =
                    std::time::Duration::from_millis(100);
                if needs_render {
                    // Another source is already dirty — update stamp and render now.
                    self.last_agent_render = Some(std::time::Instant::now());
                } else {
                    let due = self
                        .last_agent_render
                        .map(|t| t.elapsed() >= AGENT_RENDER_INTERVAL)
                        .unwrap_or(true);
                    if due {
                        self.last_agent_render = Some(std::time::Instant::now());
                        needs_render = true;
                    }
                }
            }

            // ── Inline assist stream polling (ADR 0111) ───────────────────────
            if self.poll_inline_assist() {
                needs_render = true;
            }
            // Reload any buffers the agent modified on disk this tick.
            let reloads: Vec<String> = std::mem::take(&mut self.agent_panel.pending_reloads);
            for rel_path in reloads {
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let abs_path = cwd.join(&rel_path);
                // Canonicalize once — resolves symlinks, cleans ".." etc.
                // Falls back to the plain joined path if the file somehow can't be stat'd.
                let canonical = abs_path.canonicalize().unwrap_or_else(|_| abs_path.clone());

                let mut reloaded = false;
                for buf in &mut self.buffers {
                    let matches = buf
                        .file_path
                        .as_ref()
                        .map(|fp| {
                            // Case 1: buffer stored an absolute path (opened from explorer)
                            // — compare both canonicalized so symlinks don't fool us.
                            let fp_canon = fp.canonicalize().unwrap_or_else(|_| fp.clone());
                            if fp_canon == canonical {
                                return true;
                            }
                            // Case 2: buffer stored a relative path (opened from CLI)
                            // — compare component-wise suffix of the file_path against rel_path.
                            fp.ends_with(std::path::Path::new(&rel_path))
                        })
                        .unwrap_or(false);

                    if matches {
                        if let Err(e) = buf.reload_from_disk() {
                            tracing::warn!("Failed to reload {rel_path}: {e}");
                        } else {
                            reloaded = true;
                        }
                    }
                }
                if reloaded {
                    self.set_status(format!("↺ reloaded {rel_path}"));
                    needs_render = true;
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Filesystem watcher: reload buffers changed externally ──────────
            // Prune self_saved entries older than 500 ms.
            let suppress_window = std::time::Duration::from_millis(500);
            self.self_saved.retain(|_, t| t.elapsed() < suppress_window);

            let fs_changed_paths: Vec<std::path::PathBuf> = if let Some(ref rx) = self.watcher_rx {
                let mut paths = Vec::new();
                while let Ok(Ok(event)) = rx.try_recv() {
                    use notify::EventKind;
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        for p in event.paths {
                            let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
                            // Skip events caused by our own saves.
                            let self_saved = self.self_saved.keys().any(|saved| {
                                saved.canonicalize().unwrap_or_else(|_| saved.clone()) == canonical
                            });
                            if !self_saved {
                                paths.push(p);
                            }
                        }
                    }
                }
                paths
            } else {
                Vec::new()
            };

            for changed_path in fs_changed_paths {
                let canonical =
                    changed_path.canonicalize().unwrap_or_else(|_| changed_path.clone());
                let mut status_msg: Option<String> = None;
                for buf in &mut self.buffers {
                    let matches = buf
                        .file_path
                        .as_ref()
                        .map(|fp| fp.canonicalize().unwrap_or_else(|_| fp.clone()) == canonical)
                        .unwrap_or(false);
                    if !matches {
                        continue;
                    }
                    if buf.is_modified {
                        status_msg = Some(format!(
                            "⚠ external change to '{}' (unsaved — :e! to reload)",
                            buf.name
                        ));
                    } else if buf.reload_from_disk().is_ok() {
                        status_msg = Some(format!("↺ {} reloaded", buf.name));
                    }
                    needs_render = true;
                }
                if let Some(msg) = status_msg {
                    self.set_status(msg);
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Inline completion debounce + poll ──────────────────────────────
            // Fire a new request once the debounce delay has elapsed in Insert mode.
            if self.pending_completion.is_none() && self.ghost_text.is_none() {
                if let Some(instant) = self.last_edit_instant {
                    if instant.elapsed().as_millis() >= COMPLETION_DEBOUNCE_MS
                        && self.mode == Mode::Insert
                    {
                        self.last_edit_instant = None; // consume
                        self.request_inline_completion();
                    }
                }
            }

            // Poll for a response from an in-flight request.
            let completed = if let Some(rx) = self.pending_completion.as_mut() {
                match rx.try_recv() {
                    Ok(value) => Some(value),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => {
                        // channel closed without a response
                        Some(serde_json::Value::Null)
                    },
                }
            } else {
                None
            };
            if let Some(value) = completed {
                self.pending_completion = None;
                needs_render = true;
                if let Some(text) = parse_first_inline_completion(value) {
                    if let Some(buf) = self.current_buffer() {
                        let row = buf.cursor.row;
                        let col = buf.cursor.col;
                        self.ghost_text = Some((text, row, col));
                    }
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Project-wide search: debounce + poll ──────────────────────────
            const SEARCH_DEBOUNCE_MS: u128 = 300;
            if self.search_rx.is_none() {
                if let Some(instant) = self.last_search_instant {
                    if instant.elapsed().as_millis() >= SEARCH_DEBOUNCE_MS
                        && self.mode == Mode::Search
                    {
                        self.last_search_instant = None;
                        self.fire_search();
                    }
                }
            }

            let search_done = if let Some(rx) = self.search_rx.as_mut() {
                match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(Err(anyhow::anyhow!("search channel closed"))),
                }
            } else {
                None
            };
            if let Some(result) = search_done {
                self.search_rx = None;
                needs_render = true;
                match result {
                    Ok(results) => {
                        self.search_state.set_results(results);
                    },
                    Err(e) => {
                        self.search_state.status = SearchStatus::Error(e.to_string());
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── LSP goto-definition / references / symbols polls ──────────────
            macro_rules! poll_lsp_rx {
                ($field:expr) => {{
                    if let Some(rx) = $field.as_mut() {
                        match rx.try_recv() {
                            Ok(v) => {
                                $field = None;
                                needs_render = true;
                                Some(v)
                            },
                            Err(oneshot::error::TryRecvError::Empty) => None,
                            Err(_) => {
                                $field = None;
                                Some(serde_json::Value::Null)
                            },
                        }
                    } else {
                        None
                    }
                }};
            }
            if let Some(v) = poll_lsp_rx!(self.pending_goto_definition) {
                self.handle_goto_definition_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_references) {
                self.handle_references_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_symbols) {
                self.handle_symbols_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_hover) {
                self.handle_hover_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_rename) {
                self.handle_rename_response(v);
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Commit-message AI response poll ───────────────────────────────
            let commit_done = if let Some(rx) = self.commit_msg.rx.as_mut() {
                match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(Err(anyhow::anyhow!("commit msg channel closed"))),
                }
            } else {
                None
            };
            if let Some(result) = commit_done {
                self.commit_msg.rx = None;
                needs_render = true;
                match result {
                    Ok(msg) => {
                        self.commit_msg.buffer = msg;
                        self.set_status(
                            "Commit message ready — edit then Enter to commit, Esc to discard"
                                .to_string(),
                        );
                    },
                    Err(e) => {
                        self.mode = Mode::Normal;
                        self.set_status(format!("Failed to generate commit message: {e}"));
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Release-notes AI response poll ────────────────────────────────
            let release_notes_done = if let Some(rx) = self.release_notes.rx.as_mut() {
                match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(Err(anyhow::anyhow!("release notes channel closed"))),
                }
            } else {
                None
            };
            if let Some(result) = release_notes_done {
                self.release_notes.rx = None;
                needs_render = true;
                match result {
                    Ok(notes) => {
                        self.release_notes.buffer = strip_markdown_fence(&notes);
                        self.set_status(
                            "Release notes ready — y=copy to clipboard, j/k=scroll, Esc=close"
                                .to_string(),
                        );
                    },
                    Err(e) => {
                        self.mode = Mode::Normal;
                        self.set_status(format!("Failed to generate release notes: {e}"));
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Insights narrative AI response poll (Phase 4, ADR 0129) ──────────
            let narrative_done = if let Some(rx) = self.insights_narrative_rx.as_mut() {
                match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(Err(anyhow::anyhow!("insights narrative channel closed"))),
                }
            } else {
                None
            };
            if let Some(result) = narrative_done {
                self.insights_narrative_rx = None;
                needs_render = true;
                match result {
                    Ok(narrative) => {
                        // Persist to disk so future dashboard opens include the narrative.
                        if let Some(data_dir) = crate::config::Config::log_path()
                            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                        {
                            let _ =
                                std::fs::write(data_dir.join("insights_narrative.md"), &narrative);
                        }
                        // If the dashboard is currently open, hot-reload the narrative.
                        if let Some(d) = self.insights_dashboard.as_mut() {
                            d.insights.narrative = Some(narrative);
                        }
                        self.set_status(
                            "Insights narrative ready — open SPC a I to view".to_string(),
                        );
                    },
                    Err(e) => {
                        self.set_status(format!("Failed to generate insights narrative: {e}"));
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── MCP background connection poll ────────────────────────────────
            if let Some(rx) = self.mcp_rx.as_mut() {
                if let Ok(manager) = rx.try_recv() {
                    tracing::info!("MCP ready: {}", manager.summary());
                    let arc = std::sync::Arc::new(manager);
                    self.mcp_manager = Some(std::sync::Arc::clone(&arc));
                    self.agent_panel.mcp_manager = Some(arc);
                    self.mcp_rx = None;
                    needs_render = true;
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // Force a render whenever background work is in-flight OR the
            // which-key timer is pending (so the popup appears after 500 ms
            // even when no key event arrives to trigger a normal render).
            if self.copilot_auth_rx.is_some()
                || self.pending_completion.is_some()
                || self.key_handler.which_key_pending()
                || self.search_rx.is_some()
                || self.commit_msg.rx.is_some()
                || self.release_notes.rx.is_some()
                || self.mcp_rx.is_some()
                || self.insights_narrative_rx.is_some()
            {
                needs_render = true;
            }

            // ── SIGCONT: drain any pending resume notifications ────────────────
            #[cfg(unix)]
            while sigcont_rx.try_recv().is_ok() {
                force_clear = true;
                needs_render = true;
            }

            // ── Render (only when something changed) ───────────────────────────
            if needs_render {
                if force_clear {
                    self.terminal.clear()?;
                    force_clear = false;
                }
                self.render()?;
                needs_render = false;
            }

            // ── Input (blocks up to 50 ms) ─────────────────────────────────────
            if event::poll(std::time::Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) => {
                        // Ctrl+L: force a full redraw (universal terminal convention).
                        // Intercepted before handle_key so it works in every mode.
                        if key.code == KeyCode::Char('l') && key.modifiers == KeyModifiers::CONTROL
                        {
                            force_clear = true;
                        } else {
                            self.handle_key(key)?;
                        }
                        needs_render = true;
                    },
                    // Bracketed paste: the terminal wraps pasted text in escape sequences
                    // so it arrives as a single Event::Paste(String) instead of a stream
                    // of KeyCode::Char / KeyCode::Enter events.
                    Event::Paste(text) => {
                        self.handle_paste(text)?;
                        needs_render = true;
                    },
                    // Terminal resize: the cell grid has been invalidated — clear and
                    // repaint so ratatui lays out to the new dimensions correctly.
                    Event::Resize(_, _) => {
                        force_clear = true;
                        needs_render = true;
                    },
                    _ => {},
                }
            }

            if self.should_quit {
                break;
            }
        }

        // Clean up terminal
        self.cleanup()?;
        Ok(())
    }
}
