use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use tokio::sync::oneshot;

use super::Editor;
use crate::keymap::Mode;
use crate::lsp::LspManager;
use crate::search::SearchStatus;

impl Editor {
    /// Main event loop
    pub async fn run(&mut self) -> Result<()> {
        // Render on the very first frame regardless of activity.
        let mut needs_render = true;
        // Set to true whenever the terminal cell grid may be stale (resize, SIGCONT, Ctrl+L).
        let mut force_clear = false;

        // ── SIGCONT: laptop-lid-open / process-resume repaint ─────────────────
        #[cfg(unix)]
        let (sigcont_tx, mut sigcont_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        #[cfg(unix)]
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
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
            let lsp_changed = self.lsp.manager.process_messages().unwrap_or(false);
            if lsp_changed {
                needs_render = true;
            }

            // Surface any human-readable LSP messages.
            // These are sticky so they persist until the user presses Esc.
            for msg in self.lsp.manager.drain_messages() {
                self.set_sticky(msg);
                needs_render = true;
            }

            // Update diagnostics for current buffer — only when LSP sent something new.
            if lsp_changed {
                if let Some(buf) = self.current_buffer() {
                    if let Some(path) = &buf.file_path {
                        if let Ok(uri) = LspManager::path_to_uri(path) {
                            self.lsp.diagnostics = self.lsp.manager.get_diagnostics(&uri);
                        }
                    }
                }
            }

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
            if let Some(v) = poll_lsp_rx!(self.lsp.pending_goto_definition) {
                self.handle_goto_definition_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.lsp.pending_references) {
                self.handle_references_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.lsp.pending_symbols) {
                self.handle_symbols_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.lsp.pending_hover) {
                self.handle_hover_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.lsp.pending_rename) {
                self.handle_rename_response(v);
            }
            // ──────────────────────────────────────────────────────────────────

            // Force a render whenever background work is in-flight OR the
            // which-key timer is pending.
            if self.key_handler.which_key_pending() || self.search_rx.is_some() {
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
                        if key.code == KeyCode::Char('l') && key.modifiers == KeyModifiers::CONTROL
                        {
                            force_clear = true;
                        } else {
                            self.handle_key(key)?;
                        }
                        needs_render = true;
                    },
                    Event::Paste(text) => {
                        self.handle_paste(text)?;
                        needs_render = true;
                    },
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
