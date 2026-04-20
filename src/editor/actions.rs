use anyhow::Result;

use super::{ClipboardType, Editor};
use crate::agent::{ChatMessage, Role};
use crate::keymap::{Action, Mode};
use crate::lsp::LspManager;
use crate::search::SearchState;
use notify::Watcher;

impl Editor {
    /// Execute an action
    pub(super) fn execute_action(&mut self, action: Action) -> Result<()> {
        // Don't consume the count for Noop — the user may still be building a
        // count prefix (e.g. typing "3" before "d") and we must not lose it.
        if matches!(action, Action::Noop) {
            return Ok(());
        }

        // Consume the accumulated count (defaults to 1 if none).
        let count = self.key_handler.take_count();

        // ── Undo snapshot ─────────────────────────────────────────────────────
        // Save buffer state BEFORE any action that mutates content.
        // Insert-mode entry actions save once here — all subsequent keystrokes
        // in Insert mode are NOT snapshotted individually, so the whole Insert
        // session forms a single undo step (vim behaviour).
        let needs_snapshot = matches!(
            action,
            // Enter Insert mode (one snapshot per Insert session)
            Action::Insert
            | Action::InsertAppend
            | Action::InsertLineStart
            | Action::InsertLineEnd
            | Action::InsertNewlineBelow
            | Action::InsertNewlineAbove
            // Normal-mode destructive operations
            | Action::DeleteChar
            | Action::ReplaceChar { .. }
            | Action::DeleteLine
            | Action::DeleteToLineEnd
            | Action::DeleteWord
            | Action::DeleteToChar { .. }
            | Action::YankToChar { .. }
            | Action::ChangeToChar { .. }
            | Action::DeleteSelection
            | Action::ChangeLine
            | Action::ChangeWord
            // Paste (alters content)
            | Action::PasteAfter
            | Action::PasteBefore
        );
        if needs_snapshot {
            self.with_buffer(|buf| buf.save_undo_snapshot());
        }

        match action {
            Action::Noop => unreachable!(),
            Action::Insert => self.mode = Mode::Insert,
            Action::InsertAppend => {
                self.with_buffer(|buf| buf.move_cursor_right());
                self.mode = Mode::Insert;
            },
            Action::InsertLineStart => {
                self.with_buffer(|buf| buf.move_cursor_line_start());
                self.mode = Mode::Insert;
            },
            Action::InsertLineEnd => {
                self.with_buffer(|buf| buf.move_cursor_line_end());
                self.mode = Mode::Insert;
            },
            Action::InsertNewlineBelow => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_end();
                    buf.insert_newline();
                });
                self.mode = Mode::Insert;
            },
            Action::InsertNewlineAbove => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_start();
                    buf.insert_newline();
                    buf.move_cursor_up();
                });
                self.mode = Mode::Insert;
            },
            Action::MoveLeft => {
                // h — clamped, no line wrap; repeats `count` times
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_left_clamp();
                    }
                });
            },
            Action::MoveRight => {
                // l — clamped, no line wrap; repeats `count` times
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_right_clamp();
                    }
                });
            },
            Action::MoveUp => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_up();
                    }
                });
            },
            Action::MoveDown => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_down();
                    }
                });
            },
            Action::MoveLineStart => {
                self.with_buffer(|buf| buf.move_cursor_line_start());
            },
            Action::MoveFirstNonBlank => {
                self.with_buffer(|buf| buf.move_cursor_first_nonblank());
            },
            Action::MoveLineEnd => {
                // Used by A / InsertLineEnd (cursor goes past last char)
                self.with_buffer(|buf| buf.move_cursor_line_end());
            },
            Action::MoveLineEndNormal => {
                // Used by $ in Normal mode (cursor lands ON last char)
                self.with_buffer(|buf| buf.move_cursor_line_end_normal());
            },
            Action::GotoFileTop => {
                self.with_buffer(|buf| {
                    // `5gg` → jump to line 5 (1-based); bare `gg` → first line
                    if count > 1 {
                        buf.goto_line(count);
                    } else {
                        buf.goto_first_line();
                    }
                });
            },
            Action::GotoFileBottom => {
                self.with_buffer(|buf| {
                    // `5G` → jump to line 5 (1-based); bare `G` → last line
                    if count > 1 {
                        buf.goto_line(count);
                    } else {
                        buf.goto_last_line();
                    }
                });
            },
            Action::MoveWordForward => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_word_forward();
                    }
                });
            },
            Action::MoveWordBackward => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_word_backward();
                    }
                });
            },
            Action::Command => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
            },
            Action::Visual => {
                self.with_buffer(|buf| buf.start_selection());
                self.mode = Mode::Visual;
            },
            Action::BufferList => {
                if self.buffers.is_empty() {
                    self.set_status("No buffers open".to_string());
                } else {
                    self.buffer_picker_idx = self.current_buffer_idx;
                    self.mode = Mode::PickBuffer;
                }
            },
            Action::BufferNext => {
                if !self.buffers.is_empty() {
                    self.current_buffer_idx = (self.current_buffer_idx + 1) % self.buffers.len();
                    self.set_status(format!(
                        "Switched to buffer: {}",
                        self.buffers[self.current_buffer_idx].name
                    ));
                }
            },
            Action::BufferPrevious => {
                if !self.buffers.is_empty() {
                    self.current_buffer_idx = if self.current_buffer_idx == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.current_buffer_idx - 1
                    };
                    self.set_status(format!(
                        "Switched to buffer: {}",
                        self.buffers[self.current_buffer_idx].name
                    ));
                }
            },
            Action::BufferClose => {
                if !self.buffers.is_empty() {
                    let buf = &self.buffers[self.current_buffer_idx];
                    if buf.is_modified {
                        self.set_status(
                            "Unsaved changes. Use :bd! to discard and close, or :w to save."
                                .to_string(),
                        );
                    } else {
                        let closing_idx = self.current_buffer_idx;
                        // If closing the focused pane while a split is active, bring the
                        // other pane to focus first so we never close the split's buffer
                        // out from under the cursor.
                        if let Some(other) = self.split.other_idx {
                            if closing_idx == other {
                                // Closing the background pane's buffer — just clear the split.
                                self.split.other_idx = None;
                                self.split.right_focused = false;
                                self.split.highlight_cache = None;
                            } else {
                                // Closing the focused buffer while split is open: swap focus
                                // so the other pane becomes active, then close.
                                self.current_buffer_idx = other;
                                self.split.other_idx = None;
                                self.split.right_focused = false;
                                self.split.highlight_cache = None;
                            }
                        }
                        let closing_buf = &self.buffers[closing_idx];
                        let name = closing_buf.name.clone();
                        let closed_path = closing_buf.file_path.clone();
                        let closed_uri = closed_path
                            .as_ref()
                            .and_then(|p| crate::lsp::LspManager::path_to_uri(p).ok());
                        self.buffers.remove(closing_idx);
                        if !self.buffers.is_empty() {
                            self.current_buffer_idx =
                                self.current_buffer_idx.min(self.buffers.len() - 1);
                        }
                        // Stop watching the closed file.
                        if let (Some(ref mut watcher), Some(ref p)) =
                            (&mut self.file_watcher, &closed_path)
                        {
                            let _ = watcher.unwatch(p);
                        }
                        // Evict per-buffer caches keyed by the closing index.
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
                            self.lsp_manager.clear_diagnostics_for_uri(uri);
                        }
                        self.set_status(format!("Closed buffer: {}", name));
                    }
                }
            },
            Action::BufferForceClose => {
                if !self.buffers.is_empty() {
                    let closing_idx = self.current_buffer_idx;
                    if let Some(other) = self.split.other_idx {
                        if closing_idx == other {
                            self.split.other_idx = None;
                            self.split.right_focused = false;
                            self.split.highlight_cache = None;
                        } else {
                            self.current_buffer_idx = other;
                            self.split.other_idx = None;
                            self.split.right_focused = false;
                            self.split.highlight_cache = None;
                        }
                    }
                    let force_closing_buf = &self.buffers[closing_idx];
                    let name = force_closing_buf.name.clone();
                    let closed_path = force_closing_buf.file_path.clone();
                    let closed_uri = closed_path
                        .as_ref()
                        .and_then(|p| crate::lsp::LspManager::path_to_uri(p).ok());
                    self.buffers.remove(closing_idx);
                    if !self.buffers.is_empty() {
                        self.current_buffer_idx =
                            self.current_buffer_idx.min(self.buffers.len() - 1);
                    }
                    if let (Some(ref mut watcher), Some(ref p)) =
                        (&mut self.file_watcher, &closed_path)
                    {
                        let _ = watcher.unwatch(p);
                    }
                    // Evict per-buffer caches keyed by the closing index.
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
                        self.lsp_manager.clear_diagnostics_for_uri(uri);
                    }
                    self.set_status(format!("Closed buffer (discarded): {}", name));
                }
            },
            Action::WindowSplit => {
                if self.buffers.len() < 2 {
                    self.set_status("Need another buffer open — use SPC f f".into());
                } else if self.split.other_idx.is_some() {
                    self.set_status("Split already open — SPC w c to close".into());
                } else {
                    let other = if self.current_buffer_idx == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.current_buffer_idx - 1
                    };
                    self.split.other_idx = Some(other);
                    self.split.right_focused = false;
                }
            },
            Action::WindowFocusNext => {
                if let Some(ref mut other) = self.split.other_idx {
                    std::mem::swap(&mut self.current_buffer_idx, other);
                    self.split.right_focused = !self.split.right_focused;
                }
            },
            Action::WindowClose => {
                self.split.other_idx = None;
                self.split.right_focused = false;
                self.split.highlight_cache = None;
            },
            Action::FileFind => {
                self.scan_files(); // fills file_all
                self.file_query.clear();
                self.refilter_files(); // fills file_list from file_all
                if self.file_list.is_empty() {
                    self.set_status("No files found".to_string());
                } else {
                    self.file_picker_idx = 0;
                    self.mode = Mode::PickFile;
                }
            },
            Action::FileNew => {
                // Enter command mode pre-filled with "e " — user types the path.
                self.command_buffer = "e ".to_string();
                self.mode = Mode::Command;
            },
            Action::FileEditConfig => match crate::config::Config::config_path() {
                Some(path) => self.open_file(&path)?,
                None => self.set_status("Cannot locate config file ($HOME not set)".to_string()),
            },
            Action::FileSave => {
                // Get file path and text before doing LSP operations
                let (file_path, text) = if let Some(buf) = self.current_buffer_mut() {
                    match buf.save() {
                        Ok(()) => (buf.file_path.clone(), buf.lines().join("\n")),
                        Err(e) => {
                            self.set_status(format!("Error: {e}"));
                            return Ok(());
                        },
                    }
                } else {
                    (None, String::new())
                };
                if let Some(ref p) = file_path {
                    self.self_saved.insert(p.clone(), std::time::Instant::now());
                }

                self.set_status("File saved".to_string());

                // Notify LSP about saved document
                if let Some(path) = file_path {
                    let language = LspManager::language_from_path(&path);
                    if let Ok(uri) = LspManager::path_to_uri(&path) {
                        if let Some(client) = self.lsp_manager.get_client(&language) {
                            let _ = client.did_save(uri, Some(text));
                        }
                    }
                    // Fire any matching on_save hooks (ADR 0114).
                    if let Err(e) = self.fire_hooks_for_save(&path) {
                        tracing::warn!("Hook error: {e}");
                    }
                    // Run tests and fire on_test_fail hooks if configured.
                    if let Err(e) = self.run_tests_if_configured(&path) {
                        tracing::warn!("Test hook error: {e}");
                    }
                }
            },
            Action::Quit => {
                self.check_quit()?;
            },
            Action::LspHover => {
                self.request_hover();
            },
            Action::LspGoToDefinition => {
                self.request_goto_definition();
            },
            Action::LspReferences => {
                self.request_references();
            },
            Action::LspRename => {
                self.start_lsp_rename();
            },
            Action::LspDocumentSymbols => {
                self.request_document_symbols();
            },
            Action::LspNextDiagnostic => {
                self.goto_next_diagnostic();
            },
            Action::LspPrevDiagnostic => {
                self.goto_prev_diagnostic();
            },
            Action::AgentToggle => {
                self.agent_panel.toggle_visible();
                if self.agent_panel.visible {
                    self.mode = Mode::Agent;
                    // Eagerly load models on first show
                    if self.agent_panel.available_models.is_empty() {
                        let preferred = self.config.default_copilot_model.clone();
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                if let Err(e) = self.agent_panel.ensure_models(&preferred).await {
                                    tracing::warn!("Could not fetch model list: {e}");
                                }
                            });
                        });
                    }
                } else {
                    self.mode = Mode::Normal;
                }
            },
            Action::AgentFocus => {
                if !self.agent_panel.visible {
                    self.agent_panel.visible = true;
                }
                self.agent_panel.focus();
                self.mode = Mode::Agent;
                // Eagerly load models on first show
                if self.agent_panel.available_models.is_empty() {
                    let preferred = self.config.default_copilot_model.clone();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            if let Err(e) = self.agent_panel.ensure_models(&preferred).await {
                                tracing::warn!("Could not fetch model list: {e}");
                            }
                        });
                    });
                }
            },
            Action::AgentNewConversation => {
                let model_name = self.agent_panel.selected_model_display().to_string();
                self.agent_panel.new_conversation(&model_name);
                self.set_status(format!("New conversation started · {model_name}"));
            },
            Action::ExplorerToggle => {
                self.file_explorer.toggle_visible();
                if self.file_explorer.visible {
                    self.mode = Mode::Explorer;
                } else {
                    self.mode = Mode::Normal;
                }
            },
            Action::ExplorerFocus => {
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            Action::ExplorerToggleHidden => {
                self.file_explorer.toggle_hidden();
                let status = if self.file_explorer.show_hidden {
                    "Explorer: showing hidden files"
                } else {
                    "Explorer: hiding hidden files"
                };
                self.set_status(status.to_string());
            },
            // ── Git ───────────────────────────────────────────────────────────
            Action::GitOpen => {
                self.open_lazygit()?;
            },
            Action::GitCommitStaged => self.start_commit_msg(true),
            Action::GitCommitLast => self.start_commit_msg(false),
            Action::GitReleaseNotes => self.start_release_notes(),
            // ── Markdown preview ──────────────────────────────────────────────
            Action::MarkdownPreviewToggle => {
                if self.mode == Mode::MarkdownPreview {
                    self.mode = Mode::Normal;
                    self.set_status("Preview closed".to_string());
                } else {
                    self.preview_scroll = 0;
                    self.mode = Mode::MarkdownPreview;
                    self.set_status(
                        "Markdown preview  (Esc/q=back, j/k=scroll, Ctrl+D/U=page)".to_string(),
                    );
                }
            },
            Action::MarkdownOpenBrowser => {
                self.open_markdown_in_browser();
            },
            // ── CSV / JSON preview ────────────────────────────────────────────
            Action::CsvPreviewToggle => {
                if self.mode == Mode::CsvPreview {
                    self.mode = Mode::Normal;
                    self.set_status("Preview closed".to_string());
                } else {
                    self.preview_scroll = 0;
                    self.mode = Mode::CsvPreview;
                    self.set_status(
                        "CSV preview  (Esc/q=back, j/k=scroll, Ctrl+D/U=page)".to_string(),
                    );
                }
            },
            Action::JsonPreviewToggle => {
                if self.mode == Mode::JsonPreview {
                    self.mode = Mode::Normal;
                    self.set_status("Preview closed".to_string());
                } else {
                    self.preview_scroll = 0;
                    self.mode = Mode::JsonPreview;
                    self.set_status(
                        "JSON preview  (Esc/q=back, j/k=scroll, Ctrl+D/U=page)".to_string(),
                    );
                }
            },
            // ── Memory save ───────────────────────────────────────────────────
            Action::MemorySave => {
                const MEMORY_PROMPT: &str = "\
Please save the key context from this session to the knowledge graph now.\n\
\n\
Steps:\n\
1. Call `create_entities` for any new concepts, files, or components we discussed.\n\
2. Call `add_observations` with non-obvious facts discovered during this session \
(decisions made, bugs found, patterns identified, architectural constraints).\n\
3. Call `create_relations` to link related entities where useful.\n\
\n\
Focus on what would be expensive to re-discover in a future session. \
Skip anything already obvious from reading the code.";
                self.agent_panel.input = MEMORY_PROMPT.to_string();
                // Ensure the agent panel is open and focused.
                self.agent_panel.visible = true;
                self.mode = Mode::Agent;
                self.set_status("Saving session context to memory…".to_string());
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let max_rounds = self.config.max_agent_rounds;
                let warning_threshold = self.config.agent_warning_threshold;
                let preferred_model = self.config.active_default_model().to_string();
                let auto_compress = self.config.agent.auto_compress_tool_results;
                let mask_threshold = self.config.agent.observation_mask_threshold_chars;
                let expand_threshold = self.config.agent.expand_threshold_chars;
                let fut = self.agent_panel.submit(
                    None,
                    project_root,
                    max_rounds,
                    warning_threshold,
                    &preferred_model,
                    auto_compress,
                    mask_threshold,
                    expand_threshold,
                );
                let submit_err = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        match fut.await {
                            Ok(()) => None,
                            Err(e) => {
                                tracing::warn!("Memory save error: {}", e);
                                Some(e.to_string())
                            },
                        }
                    })
                });
                if let Some(e) = submit_err {
                    self.set_status(format!("Memory save error: {e}"));
                }
            },
            // ── Auto-Janitor ──────────────────────────────────────────────────
            Action::AgentJanitorCompress => {
                self.agent_panel.compress_history();
                if self.agent_panel.input.is_empty() {
                    // compress_history() bailed — nothing to summarise.
                    self.set_status("Janitor: nothing to compress".to_string());
                } else {
                    self.agent_panel.visible = true;
                    self.mode = Mode::Agent;
                    self.set_status("Janitor: compressing history…".to_string());
                    // Show a visible marker in the chat so the user knows a
                    // second AI call is starting (the janitor, not a duplicate).
                    self.agent_panel.messages.push(ChatMessage {
                        role: Role::System,
                        content:
                            "🗜\u{fe0f} Auto-Janitor: token budget reached — compressing history…"
                                .to_string(),
                        images: vec![],
                    });
                    let project_root =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let max_rounds = 1; // Summarisation needs only one round.
                    let warning_threshold = 0;
                    // Use the cheap janitor model when configured, else fall back.
                    let janitor_model = self.config.agent.janitor_model.clone();
                    let preferred_model = if janitor_model.is_empty() {
                        self.config.active_default_model().to_string()
                    } else {
                        janitor_model
                    };
                    let auto_compress = false; // Don't compress the summariser's own output.
                    let mask_threshold = 0; // Don't mask during the summarisation call itself.
                    let fut = self.agent_panel.submit(
                        None,
                        project_root,
                        max_rounds,
                        warning_threshold,
                        &preferred_model,
                        auto_compress,
                        mask_threshold,
                        0, // Don't truncate tool results during janitor compression.
                    );
                    let submit_err = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            match fut.await {
                                Ok(()) => None,
                                Err(e) => {
                                    tracing::warn!("Janitor compress error: {}", e);
                                    Some(e.to_string())
                                },
                            }
                        })
                    });
                    if let Some(e) = submit_err {
                        self.set_status(format!("Janitor error: {e}"));
                    }
                }
            },
            // ── Investigation subagent (Phase 3.3) ──────────────────────────
            Action::AgentInvestigate => {
                if self.agent_panel.input.trim().is_empty() {
                    self.set_status(
                        "Clear input first (Ctrl+Bksp), type a query, then SPC a v".to_string(),
                    );
                } else {
                    self.agent_panel.visible = true;
                    self.mode = Mode::Agent;
                    self.set_status("Investigation running…".to_string());
                    let project_root =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let preferred_model = self.config.active_default_model().to_string();
                    let fut =
                        self.agent_panel.start_investigation_agent(project_root, &preferred_model);
                    let err = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            match fut.await {
                                Ok(()) => None,
                                Err(e) => {
                                    tracing::warn!("Investigation error: {e}");
                                    Some(e.to_string())
                                },
                            }
                        })
                    });
                    if let Some(e) = err {
                        self.set_status(format!("Investigation error: {e}"));
                    }
                }
            },
            // ── Multi-file review / change set view (ADR 0113) ───────────────
            Action::ReviewChangesOpen => {
                if !self.agent_panel.has_checkpoint() {
                    self.set_status("No agent changes to review".to_string());
                } else {
                    let project_root =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let state = crate::editor::ReviewChangesState::build(
                        &self.agent_panel.session_snapshots,
                        &self.agent_panel.session_created_files,
                        &project_root,
                    );
                    self.review_changes = Some(state);
                    self.mode = Mode::ReviewChanges;
                }
            },
            // ── Insights dashboard (ADR 0129 Phase 3) ────────────────────────
            Action::InsightsDashboardOpen => {
                let data_dir = crate::config::Config::log_path()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
                let insights = crate::insights::build_insights(&data_dir);
                self.insights_dashboard =
                    Some(crate::insights::panel::InsightsDashboardState::new(insights));
                self.mode = Mode::InsightsDashboard;
            },
            // ── Intent Translator toggle (SPC a t) ───────────────────────────
            Action::AgentIntentTranslatorToggle => {
                self.agent_panel.intent_translator_enabled =
                    !self.agent_panel.intent_translator_enabled;
                let state = if self.agent_panel.intent_translator_enabled { "on" } else { "off" };
                self.set_status(format!("Intent translator {state} (SPC a t to toggle)"));
            },
            // ── Codified Context file openers (SPC a c/C/k) ──────────────────
            Action::CodifiedContextOpenConstitution => {
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let path = project_root.join(".forgiven/constitution.md");
                if !path.exists() {
                    // Create the directory and an empty stub so the user can start writing.
                    let dir = path.parent().unwrap();
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(
                        &path,
                        "# Project Constitution\n\n\
                         ## Language\n\n\
                         ## Style\n\n\
                         ## Architecture\n\n\
                         ## Hard rules\n",
                    );
                }
                let _ = self.open_file(&path);
            },
            Action::CodifiedContextOpenSpecialist => {
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let agents_dir = project_root.join(".forgiven/agents");
                let _ = std::fs::create_dir_all(&agents_dir);
                // Open the directory in the file explorer so the user can pick a file.
                self.set_status(
                    "Specialists are in .forgiven/agents/ — use the file explorer to open one"
                        .to_string(),
                );
            },
            Action::CodifiedContextOpenKnowledge => {
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let knowledge_dir = project_root.join(".forgiven/knowledge");
                let _ = std::fs::create_dir_all(&knowledge_dir);
                self.set_status(
                    "Knowledge docs are in .forgiven/knowledge/ — use the file explorer to open one"
                        .to_string(),
                );
            },
            // ── Session revert (checkpoint undo) ─────────────────────────────
            Action::AgentSessionRevert => {
                if !self.agent_panel.has_checkpoint() {
                    self.set_status(
                        "No checkpoint: agent has not modified any files this session".to_string(),
                    );
                } else {
                    let project_root =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let (restored, deleted) = self.agent_panel.revert_session(&project_root);
                    // Queue restored files for buffer reload.
                    for rel in &restored {
                        self.agent_panel.pending_reloads.push(rel.clone());
                    }
                    let mut parts = Vec::new();
                    if !restored.is_empty() {
                        let n = restored.len();
                        parts.push(format!("{n} file{} restored", if n == 1 { "" } else { "s" }));
                    }
                    if !deleted.is_empty() {
                        let n = deleted.len();
                        parts
                            .push(format!("{n} new file{} deleted", if n == 1 { "" } else { "s" }));
                    }
                    let msg = if parts.is_empty() {
                        "Session reverted (nothing to restore)".to_string()
                    } else {
                        format!("Session reverted: {}", parts.join(", "))
                    };
                    self.set_status(msg);
                }
            },
            // ── Diagnostics overlay ───────────────────────────────────────────
            Action::DiagnosticsOpen => {
                self.mode = Mode::Diagnostics;
            },
            Action::DiagnosticsOpenLog => {
                let path = crate::config::Config::log_path()
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp/forgiven.log"));
                self.open_file(&path)?;
            },
            // ── Project-wide text search ──────────────────────────────────────
            Action::SearchOpen => {
                self.search_state = SearchState::new();
                self.search_rx = None;
                self.last_search_instant = None;
                self.mode = Mode::Search;
            },
            // ── Edit operations ───────────────────────────────────────────────
            Action::DeleteChar => {
                self.with_buffer(|buf| buf.delete_char_at_cursor());
                self.notify_lsp_change();
            },
            Action::ReplaceChar { ch } => {
                self.with_buffer(|buf| buf.replace_char_at_cursor(ch));
                self.notify_lsp_change();
            },
            // ── Linewise deletes/yanks (paste creates new rows) ───────────────
            Action::DeleteLine => {
                // `count` lines deleted, e.g. `3dd` removes 3 lines
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_lines(count));
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                }
                self.notify_lsp_change();
            },
            Action::YankLine => {
                // `count` lines yanked, e.g. `3yy` copies 3 lines
                let yanked = self.current_buffer().map(|buf| buf.yank_lines(count));
                if let Some(text) = yanked {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.set_status(format!(
                        "{count} line{} yanked",
                        if count == 1 { "" } else { "s" }
                    ));
                }
            },
            Action::ChangeLine => {
                // `count` lines deleted then enter Insert, e.g. `3cc`
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_lines(count));
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },
            // ── Visual Line mode ─────────────────────────────────────────────
            Action::VisualLine => {
                self.with_buffer(|buf| buf.start_selection_line());
                self.mode = Mode::VisualLine;
            },
            // ── Charwise deletes/yanks (paste inserts inline) ─────────────────
            Action::DeleteToLineEnd => {
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_to_line_end());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
                self.notify_lsp_change();
            },
            Action::DeleteWord => {
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_word());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
                self.notify_lsp_change();
            },
            Action::DeleteToChar { ch, inclusive } => {
                let deleted = self.current_buffer_mut().and_then(|buf| {
                    let target = buf.find_char_forward(ch)?;
                    let end_col = if inclusive { target + 1 } else { target };
                    Some(buf.delete_to_col(end_col))
                });
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
                self.notify_lsp_change();
            },
            Action::YankToChar { ch, inclusive } => {
                let yanked = self.current_buffer().and_then(|buf| {
                    let target = buf.find_char_forward(ch)?;
                    let end_col = if inclusive { target + 1 } else { target };
                    Some(buf.yank_to_col(end_col))
                });
                if let Some(text) = yanked {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
            },
            Action::ChangeToChar { ch, inclusive } => {
                let deleted = self.current_buffer_mut().and_then(|buf| {
                    let target = buf.find_char_forward(ch)?;
                    let end_col = if inclusive { target + 1 } else { target };
                    Some(buf.delete_to_col(end_col))
                });
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },
            Action::FindCharForward { ch, inclusive } => {
                self.with_buffer(|buf| {
                    if let Some(target) = buf.find_char_forward(ch) {
                        let col = if inclusive { target } else { target.saturating_sub(1) };
                        buf.move_to_col(col);
                    }
                });
            },
            Action::FindCharBackward { ch, inclusive } => {
                self.with_buffer(|buf| {
                    if let Some(target) = buf.find_char_backward(ch) {
                        let col = if inclusive { target } else { target + 1 };
                        buf.move_to_col(col);
                    }
                });
            },
            Action::YankWord => {
                let yanked = self.current_buffer().map(|buf| buf.yank_word());
                if let Some(text) = yanked {
                    let n = text.chars().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.set_status(format!("{n} chars yanked"));
                }
            },
            Action::YankToLineEnd => {
                let yanked = self.current_buffer().map(|buf| buf.yank_to_line_end());
                if let Some(text) = yanked {
                    let n = text.chars().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.set_status(format!("{n} chars yanked"));
                }
            },
            Action::YankSelection => {
                let yanked = self.current_buffer().and_then(|buf| buf.yank_selection());
                if let Some(text) = yanked {
                    let n = text.chars().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.set_status(format!("{n} chars yanked"));
                }
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },
            Action::DeleteSelection => {
                let deleted = self.current_buffer_mut().and_then(|buf| buf.delete_selection());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Normal;
            },
            Action::ChangeWord => {
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_word());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },
            // ── Paste — dispatch on clipboard type ────────────────────────────
            Action::PasteAfter => {
                if let Some((text, clip_type)) = self.clipboard.clone() {
                    self.with_buffer(|buf| match clip_type {
                        ClipboardType::Linewise => buf.paste_linewise_after(&text),
                        ClipboardType::Charwise => buf.paste_charwise_after(&text),
                    });
                    self.notify_lsp_change();
                }
            },
            Action::PasteBefore => {
                if let Some((text, clip_type)) = self.clipboard.clone() {
                    self.with_buffer(|buf| match clip_type {
                        ClipboardType::Linewise => buf.paste_linewise_before(&text),
                        ClipboardType::Charwise => buf.paste_charwise_before(&text),
                    });
                    self.notify_lsp_change();
                }
            },
            Action::Undo => {
                let did_undo = self.current_buffer_mut().map(|buf| buf.undo()).unwrap_or(false);
                if did_undo {
                    self.notify_lsp_change();
                } else {
                    self.set_status("Already at oldest change".to_string());
                }
            },
            Action::Redo => {
                let did_redo = self.current_buffer_mut().map(|buf| buf.redo()).unwrap_or(false);
                if did_redo {
                    self.notify_lsp_change();
                } else {
                    self.set_status("Already at newest change".to_string());
                }
            },
            Action::InFileSearchStart => {
                self.in_file_search_buffer.clear();
                self.mode = Mode::InFileSearch;
            },
            Action::InFileSearchNext => {
                self.with_buffer(|buf| buf.search_next());
            },
            Action::InFileSearchPrev => {
                self.with_buffer(|buf| buf.search_prev());
            },

            // ── Tree-sitter text objects (ADR 0105) ───────────────────────────
            Action::SelectTextObject { inner, kind } => {
                self.apply_text_object_select(inner, kind);
            },
            Action::DeleteTextObject { inner, kind } => {
                self.apply_text_object_delete(inner, kind);
            },
            Action::YankTextObject { inner, kind } => {
                self.apply_text_object_yank(inner, kind);
            },
            Action::ChangeTextObject { inner, kind } => {
                self.apply_text_object_change(inner, kind);
            },

            // ── Code folding (ADR 0106) ───────────────────────────────────────
            Action::FoldToggle => {
                self.fold_toggle();
            },
            Action::FoldCloseAll => {
                self.fold_close_all();
            },
            Action::FoldOpenAll => {
                self.fold_open_all();
            },

            // ── Surround operations (ADR 0110) ────────────────────────────────
            Action::SurroundDelete { ch } => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                self.apply_surround_delete(ch);
                self.notify_lsp_change();
            },
            Action::SurroundChangePrepare { from } => {
                self.surround_change_from = Some(from);
                self.set_status(format!("cs{from} — enter target char"));
            },
            Action::SurroundChange { from, to } => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                self.apply_surround_change(from, to);
                self.notify_lsp_change();
            },
            Action::SurroundAddWord { ch } => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                self.apply_surround_add_word(ch);
                self.notify_lsp_change();
            },

            // ── Inline assistant (ADR 0111) ───────────────────────────────────
            Action::InlineAssistStart => {
                if self.current_buffer().is_none() {
                    return Ok(());
                }
                // Capture the current selection (if any) and the selected text.
                // When there is no visual selection (Normal mode), synthesise a
                // line-covering selection so accept replaces the line rather than
                // inserting alongside it.
                let has_visual_selection =
                    self.current_buffer().and_then(|buf| buf.selection.as_ref()).is_some();

                let (original_selection, original_text) = if has_visual_selection {
                    let sel = self.current_buffer().and_then(|buf| buf.selection.clone());
                    let text = self
                        .current_buffer()
                        .and_then(|buf| buf.yank_selection())
                        .unwrap_or_default();
                    (sel, text)
                } else {
                    // No visual selection — treat the current line as the target.
                    let (row, line_len, line_text) = self
                        .current_buffer()
                        .map(|buf| {
                            let row = buf.cursor.row;
                            let text = buf.lines().get(row).cloned().unwrap_or_default();
                            let len = text.chars().count();
                            (row, len, text)
                        })
                        .unwrap_or((0, 0, String::new()));

                    let sel = crate::buffer::Selection::new(
                        crate::buffer::Cursor { row, col: 0 },
                        crate::buffer::Cursor { row, col: line_len },
                    );
                    (Some(sel), line_text)
                };
                let target_buffer_idx = self.current_buffer_idx;
                let language = self
                    .current_buffer()
                    .and_then(|buf| buf.file_path.as_deref())
                    .and_then(|p| p.extension())
                    .and_then(|e| e.to_str())
                    .map(|ext| match ext.to_ascii_lowercase().as_str() {
                        "rs" => "Rust".to_string(),
                        "py" => "Python".to_string(),
                        "js" => "JavaScript".to_string(),
                        "ts" => "TypeScript".to_string(),
                        "tsx" => "TypeScript TSX".to_string(),
                        "go" => "Go".to_string(),
                        "c" | "h" => "C".to_string(),
                        "cpp" | "cc" | "cxx" | "hpp" => "C++".to_string(),
                        "java" => "Java".to_string(),
                        "kt" => "Kotlin".to_string(),
                        "swift" => "Swift".to_string(),
                        "rb" => "Ruby".to_string(),
                        "sh" | "bash" | "zsh" => "Shell".to_string(),
                        "toml" => "TOML".to_string(),
                        "json" => "JSON".to_string(),
                        "yaml" | "yml" => "YAML".to_string(),
                        "md" => "Markdown".to_string(),
                        "html" => "HTML".to_string(),
                        "css" => "CSS".to_string(),
                        "sql" => "SQL".to_string(),
                        other => other.to_string(),
                    });

                self.inline_assist = Some(crate::editor::InlineAssistState {
                    prompt: String::new(),
                    original_text,
                    original_selection,
                    target_buffer_idx,
                    language,
                    response: String::new(),
                    phase: crate::editor::InlineAssistPhase::Input,
                    stream_rx: None,
                    abort_tx: None,
                });
                self.mode = Mode::InlineAssist;
            },

            Action::InlineAssistAccept => {
                if let Some(state) = self.inline_assist.take() {
                    let response = state.response.clone();
                    let buf_idx = state.target_buffer_idx;
                    if let Some(buf) = self.buffers.get_mut(buf_idx) {
                        buf.save_undo_snapshot();
                        // Restore and delete the original selection, then insert response.
                        if let Some(sel) = state.original_selection {
                            buf.selection = Some(sel);
                            buf.delete_selection();
                        }
                        if !response.is_empty() {
                            buf.insert_text_block(&response);
                        }
                        // mark_modified() is called internally by delete_selection() and
                        // insert_text_block(), so no explicit call needed.
                    }
                    self.notify_lsp_change();
                }
                self.mode = Mode::Normal;
            },

            Action::InlineAssistCancel => {
                // Dropping inline_assist fires abort_tx (oneshot sender drops = sends).
                self.inline_assist = None;
                self.mode = Mode::Normal;
            },
        }
        Ok(())
    }
}
