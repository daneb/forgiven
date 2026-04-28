use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::path::PathBuf;

use super::{ClipboardType, Editor};
use crate::keymap::{Action, Mode};

impl Editor {
    pub(super) fn cycle_panel_focus(&mut self) {
        let current: u8 = match self.mode {
            Mode::Explorer => 0,
            Mode::Agent => 2,
            _ => 1,
        };

        // Build ordered list of visible panel indices (explorer=0, editor=1, agent=2).
        let mut visible: Vec<u8> = vec![1]; // editor is always present
        if self.file_explorer.visible {
            visible.insert(0, 0);
        }
        if self.agent_panel.visible {
            visible.push(2);
        }

        if visible.len() < 2 {
            return;
        }

        let pos = visible.iter().position(|&p| p == current).unwrap_or(0);
        let next = visible[(pos + 1) % visible.len()];

        // Blur the panel losing focus.
        match current {
            0 => self.file_explorer.blur(),
            2 => self.agent_panel.blur(),
            _ => {},
        }

        // Discard any in-flight leader sequence before switching modes.
        // Without this a partial SPC q sequence started in Normal could
        // complete with a key typed in the Agent panel and quit unexpectedly.
        self.key_handler.clear_sequence();

        // Focus the panel gaining focus.
        match next {
            0 => {
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            2 => {
                self.agent_panel.focus();
                self.mode = Mode::Agent;
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
    /// Handle keys while the agent panel is focused.
    pub(super) fn handle_agent_mode(&mut self, key: KeyEvent) -> Result<()> {
        // ── Leader key sequences (e.g. SPC a v) from Agent mode ──────────────
        // Forward Space (when input is empty, so it can't corrupt typed text)
        // and any already-in-progress leader sequence to the normal-mode handler.
        // This mirrors the same forwarding in Visual / VisualLine mode.
        let input_empty = self.agent_panel.conversation.input.is_empty();
        if (key.code == KeyCode::Char(' ') && input_empty) || self.key_handler.leader_active() {
            let action = self.key_handler.handle_normal(key);
            if !matches!(action, Action::Noop) {
                return self.execute_action(action);
            }
            if self.key_handler.leader_active() {
                return Ok(()); // sequence still in-flight — don't fall through to input_char
            }
        }

        // If the agent is waiting for free-text input, intercept all keys for the input dialog.
        if self.agent_panel.asking_user_input.is_some() {
            match key.code {
                KeyCode::Char(c) if key.modifiers.is_empty() => {
                    self.agent_panel.type_char_to_input(c);
                },
                KeyCode::Backspace => {
                    self.agent_panel.backspace_input();
                },
                KeyCode::Left => {
                    self.agent_panel.move_input_cursor(-1);
                },
                KeyCode::Right => {
                    self.agent_panel.move_input_cursor(1);
                },
                KeyCode::Enter => {
                    self.agent_panel.confirm_user_input();
                },
                KeyCode::Esc => {
                    self.agent_panel.cancel_user_input();
                },
                _ => {},
            }
            return Ok(());
        }

        // If the agent is waiting for a question answer, intercept all keys for the dialog.
        if self.agent_panel.asking_user.is_some() {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.agent_panel.move_question_selection(-1);
                },
                KeyCode::Down | KeyCode::Char('j') => {
                    self.agent_panel.move_question_selection(1);
                },
                KeyCode::Enter => {
                    self.agent_panel.confirm_user_question();
                },
                KeyCode::Esc => {
                    self.agent_panel.cancel_user_question();
                },
                _ => {},
            }
            return Ok(());
        }

        // Ctrl+P file-context picker: intercept all keys while the overlay is open.
        if self.agent_panel.at_picker.is_some() {
            return self.handle_at_picker_key(key);
        }

        // Slash-command autocomplete: intercept navigation keys when the menu is visible.
        if self.agent_panel.slash_menu.is_some() {
            match key.code {
                KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                    self.agent_panel.move_slash_selection(1);
                    return Ok(());
                },
                KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
                    self.agent_panel.move_slash_selection(-1);
                    return Ok(());
                },
                KeyCode::Enter => {
                    self.agent_panel.complete_slash_selection();
                    return Ok(());
                },
                KeyCode::Esc => {
                    self.agent_panel.slash_menu = None;
                    return Ok(());
                },
                _ => {}, // fall through to normal input handling
            }
        }

        match key.code {
            // Esc — blur panel, return focus to editor.
            KeyCode::Esc => {
                self.agent_panel.blur();
                self.mode = Mode::Normal;
            },
            // Tab — toggle focus back to editor without closing.
            KeyCode::Tab => {
                self.agent_panel.blur();
                self.mode = Mode::Normal;
            },
            // Alt+Enter — insert a newline into the multi-line input.
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                self.agent_panel.input_newline();
                self.agent_panel.update_slash_menu();
            },
            // Enter — submit the input.
            KeyCode::Enter => {
                // Action slash-command interception: /compress and /translate are
                // editor actions, not prompt templates — handle them before submit.
                let trimmed = self.agent_panel.conversation.input.trim().to_string();
                if trimmed == "/compress" {
                    self.agent_panel.conversation.input.clear();
                    self.agent_panel.update_slash_menu();
                    let _ = self.execute_action(Action::AgentJanitorCompress);
                    return Ok(());
                }
                if trimmed == "/translate" {
                    self.agent_panel.conversation.input.clear();
                    self.agent_panel.update_slash_menu();
                    let _ = self.execute_action(Action::AgentIntentTranslatorToggle);
                    return Ok(());
                }
                // Snapshot current buffer content as context, including its path
                // so the model knows which file is open and can reference it directly.
                let context = self.current_buffer().map(|buf| {
                    let path_header =
                        buf.file_path.as_deref().and_then(|p| p.to_str()).unwrap_or(&buf.name);
                    format!("File: {path_header}\n\n{}", buf.lines().join("\n"))
                });
                // Project root for tool sandboxing.
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                // Submit is async; spawn a task and let the stream_rx handle tokens.
                let panel = &mut self.agent_panel;
                let max_rounds = self.config.max_agent_rounds;
                let warning_threshold = self.config.agent_warning_threshold;
                let preferred_model = self.config.active_default_model().to_string();
                let auto_compress = self.config.agent.auto_compress_tool_results;
                let mask_threshold = self.config.agent.observation_mask_threshold_chars;
                let expand_threshold = self.config.agent.expand_threshold_chars;
                // We need a blocking submit here.  Use a one-shot channel via block_in_place
                // or simply call submit synchronously via tokio::task::block_in_place.
                // Since we are inside an async context, we use a local async block.
                let fut = panel.submit(
                    context,
                    project_root,
                    max_rounds,
                    warning_threshold,
                    &preferred_model,
                    auto_compress,
                    mask_threshold,
                    expand_threshold,
                );
                // We can't .await inside handle_key (sync fn), so we use try_join on
                // the runtime directly.  The cleanest way: push to a queue and process
                // in the async run() loop.  For now use tokio::task::block_in_place.
                let submit_err = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        match fut.await {
                            Ok(()) => None,
                            Err(e) => {
                                tracing::warn!("Agent submit error: {}", e);
                                Some(e.to_string())
                            },
                        }
                    })
                });
                if let Some(e) = submit_err {
                    self.set_status(format!("Agent error: {e}"));
                }
                self.agent_panel.update_slash_menu();
            },
            // Ctrl+Backspace — clear all pending input (text, pastes, images, files).
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.agent_panel.clear_input();
                self.set_status("Input cleared".to_string());
            },
            // Backspace — delete last input character.
            KeyCode::Backspace => {
                self.agent_panel.input_backspace();
                self.agent_panel.update_slash_menu();
            },
            // Left/Right — move cursor within the input field.
            KeyCode::Left => self.agent_panel.cursor_left(),
            KeyCode::Right => self.agent_panel.cursor_right(),
            // Alt+Up / Alt+Down — navigate input history.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                self.agent_panel.history_up();
            },
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                self.agent_panel.history_down();
            },
            // Up / Down — scroll the session message list.
            KeyCode::Up => self.agent_panel.scroll_up(),
            KeyCode::Down => self.agent_panel.scroll_down(),
            // Ctrl+T — cycle through available models.
            // Note: Ctrl+M = Enter (0x0D) in all terminals and cannot be used here.
            // Ctrl+T (0x14) is safe in raw mode and not used by this editor.
            // On first press, fetches the live model list from the Copilot API.
            // Ctrl+C — abort the running agentic loop (stream + tool calls).
            KeyCode::Char('c')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.agent_panel.stream_rx.is_some() =>
            {
                self.agent_panel.cancel_stream();
                self.set_status("Agent stopped".to_string());
            },
            // Ctrl+K — copy next code block from the last reply (cycles through all blocks).
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(reply) = self.agent_panel.last_assistant_reply() {
                    let blocks = crate::agent::AgentPanel::extract_code_blocks(&reply);
                    if blocks.is_empty() {
                        self.set_status("No code blocks in last reply".to_string());
                    } else {
                        let idx = self.agent_panel.code_block_idx % blocks.len();
                        self.sync_system_clipboard(&blocks[idx]);
                        self.set_status(format!(
                            "Code block {}/{} copied  (Ctrl+K for next)",
                            idx + 1,
                            blocks.len()
                        ));
                        self.agent_panel.code_block_idx =
                            (self.agent_panel.code_block_idx + 1) % blocks.len();
                    }
                } else {
                    self.set_status("No reply to copy".to_string());
                }
            },
            // Ctrl+M — open the next mermaid diagram from the last reply in the browser.
            // Auto-fixes unquoted parentheses in node labels (common AI generation bug).
            // Cycles through multiple diagrams; resets on new reply.
            KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_mermaid_in_browser();
            },
            // Ctrl+Y — yank the full last reply to the system clipboard.
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = self.agent_panel.last_assistant_reply() {
                    let len = text.lines().count();
                    self.sync_system_clipboard(&text);
                    self.set_status(format!("Copied {} lines to clipboard", len));
                } else {
                    self.set_status("No reply to copy".to_string());
                }
            },
            // Ctrl+P — open the file-context picker (attach a file to agent message).
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_at_picker();
            },
            // Ctrl+V — paste from clipboard (image-first, then text fallback).
            // On macOS Cmd+V triggers bracketed paste (text only via Event::Paste);
            // Ctrl+V is passed to the app and allows us to read images via arboard.
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                use crate::agent::AgentPanel;
                match AgentPanel::try_paste_image() {
                    Ok(Some(img)) => {
                        let w = img.width;
                        let h = img.height;
                        self.agent_panel.image_blocks.push(img);
                        self.set_status(format!("Image pasted ({w}x{h})"));
                    },
                    Ok(None) => {
                        // No image — try text from clipboard.
                        match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
                            Ok(text) if !text.is_empty() => {
                                self.handle_paste(text)?;
                            },
                            _ => {
                                self.set_status("Clipboard empty".to_string());
                            },
                        }
                    },
                    Err(e) => {
                        self.set_status(format!("Image paste failed: {e}"));
                    },
                }
            },
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Eagerly load models if not yet fetched (brief one-time network call).
                let was_empty = self.agent_panel.available_models.is_empty();
                if was_empty {
                    self.set_status("Loading model list…".to_string());
                    let preferred = self.config.default_copilot_model.clone();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            if let Err(e) = self.agent_panel.ensure_models(&preferred).await {
                                tracing::warn!("Could not fetch model list: {e}");
                            }
                        });
                    });
                    // First press: just confirm the config-preferred model; don't advance past it.
                } else {
                    // Subsequent presses: cycle to next model and persist the choice.
                    self.agent_panel.cycle_model();
                    let model_id = self.agent_panel.selected_model_id().to_string();
                    let model_name = self.agent_panel.selected_model_display().to_string();
                    self.config.default_copilot_model = model_id.clone();
                    if let Err(e) = self.config.save() {
                        tracing::warn!("Failed to save config: {e}");
                    }
                    // Clear conversation history so the new model gets a clean context.
                    self.agent_panel.new_conversation(&model_name);
                }
                let model_name = self.agent_panel.selected_model_display().to_string();
                let n = self.agent_panel.available_models.len();
                let idx = self.agent_panel.selected_model + 1;

                self.set_status(format!(
                    "Agent model → {model_name}  [{idx}/{n}]  (Ctrl+T to cycle)"
                ));
            },
            // Ctrl+Shift+T — refresh model list from API (picks up new releases).
            KeyCode::Char('T') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_status("Refreshing model list from API…".to_string());
                let preferred = self.config.default_copilot_model.clone();
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        if let Err(e) = self.agent_panel.refresh_models(&preferred).await {
                            tracing::warn!("Could not refresh model list: {e}");
                            self.set_status(format!("Failed to refresh models: {e}"));
                        } else {
                            let model_name = self.agent_panel.selected_model_display().to_string();
                            let n = self.agent_panel.available_models.len();
                            self.set_status(format!(
                                "Refreshed {n} models, selected: {model_name}"
                            ));
                        }
                    });
                });
            },
            // Regular characters — handle special agent commands before appending to input.
            KeyCode::Char(ch) => {
                // If awaiting continuation, 'y' approves and 'n' denies.
                if self.agent_panel.awaiting_continuation {
                    match ch {
                        'y' | 'Y' => {
                            self.agent_panel.approve_continuation();
                            self.set_status("Continuing agent work...".to_string());
                        },
                        'n' | 'N' => {
                            self.agent_panel.deny_continuation();
                            self.set_status("Agent stopped by user".to_string());
                        },
                        _ => {
                            // Ignore other keys when awaiting continuation
                        },
                    }
                    return Ok(());
                }

                // All other characters type into the input box.
                // (Apply-diff, copy code block, and yank-reply moved to Ctrl+A / Ctrl+K / Ctrl+Y
                // so single letters never intercept the first character of a message.)
                self.agent_panel.input_char(ch);
                self.agent_panel.update_slash_menu();
            },
            _ => {},
        }
        Ok(())
    }

    // ── Paste handling ─────────────────────────────────────────────────────────

    /// Handle a bracketed-paste event.
    ///
    /// In Agent mode newlines are preserved so multi-line pastes work correctly.
    /// The user still presses Enter to send.
    pub(super) fn handle_paste(&mut self, text: String) -> Result<()> {
        if self.mode == Mode::Agent {
            // Store the paste as a block; the UI shows a compact summary line
            // ("⎘ Pasted N lines") and the full content is sent with the message.
            let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
            let line_count = normalised.lines().count();
            self.agent_panel.pasted_blocks.push((normalised, line_count));
        } else if self.mode == Mode::Insert {
            // In insert mode, paste the text as-is into the current buffer.
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
            // :insights summarize — generate LLM narrative (Phase 4, ADR 0129)
            "insights summarize" => {
                self.generate_insights_narrative(20);
            },
            // :insights — show collaboration analytics from forgiven.log
            "insights" => {
                let log_path = crate::config::Config::log_path()
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp/forgiven.log"));
                match crate::insights::parse_log_file(&log_path) {
                    Some(summary) => {
                        let report = summary.format_report();
                        self.agent_panel.visible = true;
                        self.agent_panel.conversation.messages.push(crate::agent::ChatMessage {
                            role: crate::agent::Role::Assistant,
                            content: report,
                            images: vec![],
                        });
                        self.agent_panel.scroll_to_bottom();
                        self.set_status("Insights loaded".to_string());
                    },
                    None => {
                        self.set_status(format!("No log found at {}", log_path.display()));
                    },
                }
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
