use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::path::PathBuf;

use super::{ClipboardType, Editor};
use crate::keymap::{Action, Mode};

impl Editor {
    pub(super) fn cycle_panel_focus(&mut self) {
        let current: u8 = match self.mode {
            Mode::Explorer => 0,
            _ => 1,
        };

        // Build ordered list of visible panel indices (explorer=0, editor=1).
        let mut visible: Vec<u8> = vec![1]; // editor is always present
        if self.file_explorer.visible {
            visible.insert(0, 0);
        }

        if visible.len() < 2 {
            return;
        }

        let pos = visible.iter().position(|&p| p == current).unwrap_or(0);
        let next = visible[(pos + 1) % visible.len()];

        // Blur the panel losing focus.
        if current == 0 {
            self.file_explorer.blur();
        }

        // Discard any in-flight leader sequence before switching modes.
        self.key_handler.clear_sequence();

        // Focus the panel gaining focus.
        match next {
            0 => {
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            _ => {
                self.mode = Mode::Normal;
            },
        }
    }

    /// Handle a key press
    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Esc always clears sticky notifications (user explicitly dismissing).
        if key.code == KeyCode::Esc {
            self.status_sticky = false;
        }

        // Clear transient status message on any new input (except sticky messages and picker modes).
        if self.mode != Mode::PickBuffer
            && self.mode != Mode::PickFile
            && self.mode != Mode::Search
            && !self.status_sticky
        {
            self.status_message = None;
        }

        // Global: Ctrl+W cycles visible panels (Explorer → Editor → Agent → wrap).
        // Skip in modes that capture text input or show modal overlays.
        if key.code == KeyCode::Char('w')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !matches!(
                self.mode,
                Mode::Command
                    | Mode::PickBuffer
                    | Mode::PickFile
                    | Mode::InFileSearch
                    | Mode::RenameFile
                    | Mode::DeleteFile
                    | Mode::NewFolder
                    | Mode::CommitMsg
                    | Mode::Diagnostics
                    | Mode::LspRename
                    | Mode::InlineAssist
            )
        {
            self.cycle_panel_focus();
            return Ok(());
        }

        match self.mode {
            Mode::Normal => self.handle_normal_mode(key)?,
            Mode::Insert => self.handle_insert_mode(key)?,
            Mode::Command => self.handle_command_mode(key)?,
            Mode::Visual => self.handle_visual_mode(key)?,
            Mode::VisualLine => self.handle_visual_line_mode(key)?,
            Mode::PickBuffer => self.handle_pick_buffer_mode(key)?,
            Mode::PickFile => self.handle_pick_file_mode(key)?,
            Mode::Agent => self.handle_agent_mode(key)?,
            Mode::Explorer => self.handle_explorer_mode(key)?,
            Mode::MarkdownPreview => self.handle_preview_mode(key)?,
            Mode::Search => self.handle_search_mode(key)?,
            Mode::InFileSearch => self.handle_in_file_search_mode(key)?,
            Mode::RenameFile => self.handle_rename_mode(key)?,
            Mode::DeleteFile => self.handle_delete_mode(key)?,
            Mode::NewFolder => self.handle_new_folder_mode(key)?,
            Mode::CommitMsg => self.handle_commit_msg_mode(key)?,
            Mode::ReleaseNotes => self.handle_release_notes_mode(key)?,
            Mode::Diagnostics => {
                // Any key closes the overlay.
                self.mode = Mode::Normal;
            },
            Mode::BinaryFile => self.handle_binary_file_mode(key)?,
            Mode::LocationList => self.handle_location_list_mode(key)?,
            Mode::LspHover => self.handle_lsp_hover_mode(key)?,
            Mode::LspRename => self.handle_lsp_rename_mode(key)?,
            Mode::InlineAssist => self.handle_inline_assist_mode(key)?,
            Mode::ReviewChanges => self.handle_review_changes_mode(key)?,
            Mode::InsightsDashboard => self.handle_insights_dashboard_mode(key)?,
        }

        Ok(())
    }

    /// Handle keys in Normal mode
    pub(super) fn handle_normal_mode(&mut self, key: KeyEvent) -> Result<()> {
        // ── Surround change: awaiting the `to` char after `cs{from}` ──────────
        if let Some(from) = self.surround_change_from.take() {
            if let KeyCode::Char(to) = key.code {
                return self.execute_action(crate::keymap::Action::SurroundChange { from, to });
            }
            // Non-char key — cancel silently.
            return Ok(());
        }

        let action = self.key_handler.handle_normal(key);
        self.execute_action(action)?;
        Ok(())
    }

    pub(super) fn handle_visual_mode(&mut self, key: KeyEvent) -> Result<()> {
        // ── Leader key sequences (e.g. SPC a i) from Visual mode ─────────────
        // Forward Space and any in-progress leader sequence to the normal-mode
        // handler so the visual selection is preserved when triggering actions
        // like InlineAssistStart.
        if key.code == KeyCode::Char(' ') || self.key_handler.leader_active() {
            let action = self.key_handler.handle_normal(key);
            if !matches!(action, Action::Noop) {
                return self.execute_action(action);
            }
            return Ok(());
        }

        // ── Text object prefix (`i` / `a` + kind char) ────────────────────────
        // When `i` or `a` was pressed last frame, the next char selects a
        // tree-sitter text object (f = function, c = class, b = block).
        if let Some(prefix) = self.visual_text_obj_prefix.take() {
            if let KeyCode::Char(ch) = key.code {
                if let Some(kind) = crate::keymap::TextObjectKind::from_char(ch) {
                    let inner = prefix == 'i';
                    return self.execute_action(Action::SelectTextObject { inner, kind });
                }
            }
            // Unrecognised key after prefix — fall through to normal handling
        }

        match key.code {
            // ── Exit / cancel ─────────────────────────────────────────────────
            KeyCode::Esc => {
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },

            // ── Yank / delete / change operators ──────────────────────────────
            // y — copy selection to register + system clipboard, back to Normal
            KeyCode::Char('y') => {
                self.execute_action(Action::YankSelection)?;
            },
            // d / x — delete selection into register, back to Normal
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.execute_action(Action::DeleteSelection)?;
            },
            // c — delete selection + enter Insert mode
            KeyCode::Char('c') => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                let deleted = self.current_buffer_mut().and_then(|buf| buf.delete_selection());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },

            // ── Motion keys (extend the selection) ────────────────────────────
            KeyCode::Char('h') | KeyCode::Left => {
                self.with_buffer(|buf| {
                    buf.move_cursor_left();
                    buf.update_selection();
                });
            },
            KeyCode::Char('l') | KeyCode::Right => {
                self.with_buffer(|buf| {
                    buf.move_cursor_right();
                    buf.update_selection();
                });
            },
            KeyCode::Char('k') | KeyCode::Up => {
                self.with_buffer(|buf| {
                    buf.move_cursor_up();
                    buf.update_selection();
                });
            },
            KeyCode::Char('j') | KeyCode::Down => {
                self.with_buffer(|buf| {
                    buf.move_cursor_down();
                    buf.update_selection();
                });
            },
            KeyCode::Char('w') => {
                self.with_buffer(|buf| {
                    buf.move_cursor_word_forward();
                    buf.update_selection();
                });
            },
            KeyCode::Char('b') => {
                self.with_buffer(|buf| {
                    buf.move_cursor_word_backward();
                    buf.update_selection();
                });
            },
            KeyCode::Char('0') | KeyCode::Home => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_start();
                    buf.update_selection();
                });
            },
            KeyCode::Char('^') => {
                self.with_buffer(|buf| {
                    buf.move_cursor_first_nonblank();
                    buf.update_selection();
                });
            },
            KeyCode::Char('$') | KeyCode::End => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_end_normal();
                    buf.update_selection();
                });
            },
            KeyCode::Char('G') => {
                self.with_buffer(|buf| {
                    buf.goto_last_line();
                    buf.update_selection();
                });
            },

            // ── Tree-sitter text object prefix ────────────────────────────��───
            // `i` or `a` stores the prefix; the NEXT keypress resolves the kind.
            KeyCode::Char('i') | KeyCode::Char('a') => {
                if let KeyCode::Char(ch) = key.code {
                    self.visual_text_obj_prefix = Some(ch);
                }
            },

            // ── Indent / dedent selection ─────────────────────────────────────
            KeyCode::Tab => {
                let use_spaces = self.config.use_spaces;
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| {
                    buf.save_undo_snapshot();
                    buf.indent_selected_lines(use_spaces, tab_width);
                });
                self.notify_lsp_change();
            },
            KeyCode::BackTab => {
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| {
                    buf.save_undo_snapshot();
                    buf.dedent_selected_lines(tab_width);
                });
                self.notify_lsp_change();
            },

            _ => {},
        }
        Ok(())
    }

    /// Handle keys in Visual Line mode (`V`)
    ///
    /// The selection always covers whole lines. `j`/`k` move the cursor and
    /// re-anchor the selection; `y`/`d`/`x` operate on the selected line span.
    pub(super) fn handle_visual_line_mode(&mut self, key: KeyEvent) -> Result<()> {
        // ── Leader key sequences (e.g. SPC a i) from Visual Line mode ────────
        if key.code == KeyCode::Char(' ') || self.key_handler.leader_active() {
            let action = self.key_handler.handle_normal(key);
            if !matches!(action, Action::Noop) {
                return self.execute_action(action);
            }
            return Ok(());
        }

        match key.code {
            // ── Exit ──────────────────────────────────────────────────────────
            KeyCode::Esc | KeyCode::Char('V') => {
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },

            // ── Yank selection (linewise) ─────────────────────────────────────
            // `y` — copy selected lines into register + system clipboard, Normal
            KeyCode::Char('y') => {
                let yanked = self.current_buffer().and_then(|buf| buf.yank_selection_lines());
                if let Some(text) = yanked {
                    let n = text.lines().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.set_status(format!("{n} line{} yanked", if n == 1 { "" } else { "s" }));
                }
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },

            // ── Delete / change selection (linewise) ─────────────────────────
            // `d` / `x` — remove selected lines, store in register, Normal
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                let deleted =
                    self.current_buffer_mut().and_then(|buf| buf.delete_selection_lines());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Normal;
            },

            // `c` — remove selected lines + enter Insert
            KeyCode::Char('c') => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                let deleted =
                    self.current_buffer_mut().and_then(|buf| buf.delete_selection_lines());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },

            // ── Motion keys (extend the line selection) ───────────────────────
            KeyCode::Char('j') | KeyCode::Down => {
                self.with_buffer(|buf| {
                    buf.move_cursor_down();
                    buf.update_selection_line();
                });
            },
            KeyCode::Char('k') | KeyCode::Up => {
                self.with_buffer(|buf| {
                    buf.move_cursor_up();
                    buf.update_selection_line();
                });
            },
            KeyCode::Char('G') => {
                self.with_buffer(|buf| {
                    buf.goto_last_line();
                    buf.update_selection_line();
                });
            },
            KeyCode::Char('g') => {
                // gg — go to first line (we can't use pending_key here easily,
                // so a single `g` press jumps to the top — matches common muscle
                // memory for `Vgg` select-to-top).
                self.with_buffer(|buf| {
                    buf.goto_first_line();
                    buf.update_selection_line();
                });
            },

            // ── Indent / dedent selection ─────────────────────────────────────
            KeyCode::Tab => {
                let use_spaces = self.config.use_spaces;
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| {
                    buf.save_undo_snapshot();
                    buf.indent_selected_lines(use_spaces, tab_width);
                });
                self.notify_lsp_change();
            },
            KeyCode::BackTab => {
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| {
                    buf.save_undo_snapshot();
                    buf.dedent_selected_lines(tab_width);
                });
                self.notify_lsp_change();
            },

            _ => {},
        }
        Ok(())
    }

    /// Handle keys in PickBuffer mode
    /// Handle keys while the agent panel is focused (agent panel removed in slim build).
    pub(super) fn handle_agent_mode(&mut self, key: KeyEvent) -> Result<()> {
        // Agent panel removed — Esc returns to Normal, everything else is a noop.
        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
        }
        Ok(())
    }

    // ── Paste handling ─────────────────────────────────────────────────────────

    /// Handle a bracketed-paste event.
    pub(super) fn handle_paste(&mut self, text: String) -> Result<()> {
        if self.mode == Mode::Insert {
            let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
            self.with_buffer(|buf| buf.insert_text_block(&normalised));
        }
        Ok(())
    }

    // ── Explorer mode key handling ─────────────────────────────────────────────

    // ── Fuzzy file search ──────────────────────────────────────────────────────

    /// Score `query` against `candidate` using a subsequence-match algorithm.
    /// Returns `None` if not all query chars appear in order in the candidate.
    /// Returns `Some((score, match_indices))` otherwise; higher score = better match.
    /// Handle keys in Insert mode
    pub(super) fn handle_insert_mode(&mut self, key: KeyEvent) -> Result<()> {
        let should_notify_lsp = match key.code {
            // Tab: accept ghost text suggestion if one is displayed at the cursor.
            KeyCode::Tab => {
                if let Some((text, row, col)) = self.ghost_text.take() {
                    let cursor_matches = self
                        .current_buffer()
                        .map(|b| b.cursor.row == row && b.cursor.col == col)
                        .unwrap_or(false);
                    if cursor_matches {
                        for ch in text.chars() {
                            if ch == '\n' {
                                if let Some(buf) = self.current_buffer_mut() {
                                    buf.insert_newline();
                                }
                            } else if let Some(buf) = self.current_buffer_mut() {
                                buf.insert_char(ch);
                            }
                        }
                        self.pending_completion = None;
                        // Notify LSP of the accepted text.
                        self.notify_lsp_change();
                        // Immediately clear the debounce so we don't re-request right away.
                        self.last_edit_instant = None;
                        return Ok(());
                    }
                }
                // No ghost text — insert indent (spaces or tab based on config).
                let use_spaces = self.config.use_spaces;
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| {
                    if use_spaces {
                        for _ in 0..tab_width {
                            buf.insert_char(' ');
                        }
                    } else {
                        buf.insert_char('\t');
                    }
                });
                true
            },
            KeyCode::BackTab => {
                // Shift+Tab — remove one indent level from the start of the line.
                let use_spaces = self.config.use_spaces;
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| buf.dedent_line(use_spaces, tab_width));
                true
            },
            KeyCode::Esc => {
                // Clear ghost text when leaving Insert mode.
                self.ghost_text = None;
                self.pending_completion = None;
                self.last_edit_instant = None;
                self.mode = Mode::Normal;
                false
            },
            KeyCode::Char(c) => {
                self.with_buffer(|buf| buf.insert_char(c));
                true
            },
            KeyCode::Enter => {
                self.with_buffer(|buf| buf.insert_newline());
                true
            },
            KeyCode::Backspace => {
                self.with_buffer(|buf| buf.delete_char_before());
                true
            },
            KeyCode::Delete => {
                self.with_buffer(|buf| buf.delete_char_at());
                true
            },
            KeyCode::Left => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_left());
                false
            },
            KeyCode::Right => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_right());
                false
            },
            KeyCode::Up => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_up());
                false
            },
            KeyCode::Down => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_down());
                false
            },
            _ => false,
        };

        // Notify LSP about content changes
        if should_notify_lsp {
            self.notify_lsp_change();
        }

        Ok(())
    }

    /// Handle keys in Command mode
    pub(super) fn handle_command_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            },
            KeyCode::Enter => {
                self.execute_command()?;
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            },
            KeyCode::Char(c) => {
                self.command_buffer.push(c);
            },
            KeyCode::Backspace => {
                self.command_buffer.pop();
            },
            _ => {},
        }

        Ok(())
    }

    /// Execute a command entered in command mode
    pub(super) fn execute_command(&mut self) -> Result<()> {
        let cmd = self.command_buffer.trim();

        match cmd {
            "q" | "quit" => {
                self.check_quit()?;
            },
            "q!" | "quit!" => {
                self.should_quit = true;
            },
            "w" | "write" => {
                if let Some(buf) = self.current_buffer_mut() {
                    match buf.save() {
                        Ok(()) => {
                            if let Some(ref p) = buf.file_path.clone() {
                                self.self_saved.insert(p.clone(), std::time::Instant::now());
                            }
                            self.set_status("File saved".to_string());
                        },
                        Err(e) => self.set_status(format!("Error: {e}")),
                    }
                }
            },
            "wq" => {
                if let Some(buf) = self.current_buffer_mut() {
                    match buf.save() {
                        Ok(()) => {
                            if let Some(ref p) = buf.file_path.clone() {
                                self.self_saved.insert(p.clone(), std::time::Instant::now());
                            }
                        },
                        Err(e) => {
                            self.set_status(format!("Error: {e}"));
                            return Ok(());
                        },
                    }
                }
                self.should_quit = true;
            },
            // :bd / :bdelete — close buffer, refuse if unsaved
            "bd" | "bdelete" => {
                if !self.buffers.is_empty() {
                    let is_modified = self.buffers[self.current_buffer_idx].is_modified;
                    if is_modified {
                        self.set_status(
                            "Unsaved changes. Use :bd! to discard and close, or :w to save."
                                .to_string(),
                        );
                    } else {
                        let closing_idx = self.current_buffer_idx;
                        let closed_path = self.buffers[closing_idx].file_path.clone();
                        let closed_uri = closed_path
                            .as_ref()
                            .and_then(|p| crate::lsp::LspManager::path_to_uri(p).ok());
                        let name = self.buffers[closing_idx].name.clone();
                        self.buffers.remove(closing_idx);
                        if !self.buffers.is_empty() {
                            self.current_buffer_idx =
                                self.current_buffer_idx.min(self.buffers.len() - 1);
                        }
                        self.ts_cache.remove(&closing_idx);
                        self.ts_versions.remove(&closing_idx);
                        self.fold_closed.remove(&closing_idx);
                        if self
                            .sticky_scroll_cache
                            .as_ref()
                            .is_some_and(|c| c.buffer_idx == closing_idx)
                        {
                            self.sticky_scroll_cache = None;
                        }
                        if let Some(ref uri) = closed_uri {
                            self.lsp.manager.clear_diagnostics_for_uri(uri);
                        }
                        self.set_status(format!("Closed buffer: {name}"));
                    }
                }
            },
            // :bd! / :bdelete! — force-close buffer, discarding unsaved changes
            "bd!" | "bdelete!" => {
                if !self.buffers.is_empty() {
                    let closing_idx = self.current_buffer_idx;
                    let closed_path = self.buffers[closing_idx].file_path.clone();
                    let closed_uri = closed_path
                        .as_ref()
                        .and_then(|p| crate::lsp::LspManager::path_to_uri(p).ok());
                    let name = self.buffers[closing_idx].name.clone();
                    self.buffers.remove(closing_idx);
                    if !self.buffers.is_empty() {
                        self.current_buffer_idx =
                            self.current_buffer_idx.min(self.buffers.len() - 1);
                    }
                    self.ts_cache.remove(&closing_idx);
                    self.ts_versions.remove(&closing_idx);
                    self.fold_closed.remove(&closing_idx);
                    if self
                        .sticky_scroll_cache
                        .as_ref()
                        .is_some_and(|c| c.buffer_idx == closing_idx)
                    {
                        self.sticky_scroll_cache = None;
                    }
                    if let Some(ref uri) = closed_uri {
                        self.lsp.manager.clear_diagnostics_for_uri(uri);
                    }
                    self.set_status(format!("Closed buffer: {name} (discarded changes)"));
                }
            },
            "copilot status" => {
                let completion_state = if self.ghost_text.is_some() {
                    "suggestion ready (Tab to accept)"
                } else if self.pending_completion.is_some() {
                    "fetching suggestion..."
                } else {
                    "idle (type in Insert mode to trigger)"
                };
                let has_server = self.lsp.manager.get_client("copilot").is_some();
                self.set_status(format!(
                    "Copilot: server={} | {}",
                    if has_server { "running" } else { "not connected" },
                    completion_state
                ));
            },
            "copilot auth" => {
                // Re-run the auth check + sign-in initiate flow manually.
                if let Some(client) = self.lsp.manager.get_client("copilot") {
                    match client.copilot_check_status() {
                        Ok(rx) => {
                            self.copilot_auth_rx = Some(rx);
                            self.set_status("Copilot: checking auth status…".to_string());
                        },
                        Err(e) => {
                            self.set_status(format!("Copilot auth error: {}", e));
                        },
                    }
                } else {
                    self.set_status(
                        "Copilot: server not connected (check config.toml)".to_string(),
                    );
                }
            },
            // :e <path> / :edit <path> — open or create a file
            _ if cmd.starts_with("e ") || cmd.starts_with("edit ") => {
                let path_str = cmd.split_once(' ').map(|(_, rest)| rest).unwrap_or("").trim();
                if path_str.is_empty() {
                    self.set_status("Usage: e <path>  (e.g.  e src/main.rs)".to_string());
                } else {
                    let path = {
                        let p = PathBuf::from(path_str);
                        if p.is_absolute() {
                            p
                        } else {
                            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
                        }
                    };
                    self.open_file(&path)?;
                    // Refresh explorer tree so newly-created buffers show up on save.
                    if self.file_explorer.visible {
                        self.file_explorer.reload();
                    }
                }
            },
            // :s/pattern/replacement or :s/pattern/replacement/g
            _ if cmd.starts_with("s/") => {
                let rest = &cmd[2..];
                let parts: Vec<&str> = rest.splitn(3, '/').collect();
                if parts.len() < 2 {
                    self.set_status("Usage: s/pattern/replacement[/g]".to_string());
                } else {
                    let pattern = parts[0].to_string();
                    let replacement = parts[1].to_string();
                    let global = parts.get(2).map(|s| *s == "g").unwrap_or(false);
                    self.with_buffer(|buf| buf.set_search_pattern(pattern));
                    if global {
                        let count = self
                            .current_buffer_mut()
                            .map(|buf| buf.replace_all(&replacement))
                            .unwrap_or(0);
                        if count == 0 {
                            self.set_status("Pattern not found".to_string());
                        } else {
                            self.notify_lsp_change();
                            self.set_status(format!("{} replacement(s) made", count));
                        }
                    } else {
                        let made = self
                            .current_buffer_mut()
                            .map(|buf| buf.replace_current(&replacement))
                            .unwrap_or(false);
                        if made {
                            self.notify_lsp_change();
                            self.set_status("1 replacement made".to_string());
                        } else {
                            self.set_status("Pattern not found".to_string());
                        }
                    }
                }
            },
            // :insights — removed in slim build
            "insights" | "insights summarize" => {
                self.set_status("Insights removed in slim build".to_string());
            },
            // :12 — jump to line 12 (1-based), same as vim
            _ if cmd.chars().all(|c| c.is_ascii_digit()) => {
                if let Ok(n) = cmd.parse::<usize>() {
                    self.with_buffer(|buf| buf.goto_line(n));
                }
            },
            _ => {
                self.set_status(format!("Unknown command: {}", cmd));
            },
        }

        Ok(())
    }
}
