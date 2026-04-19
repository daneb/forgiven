use anyhow::Result;
use ratatui::text::Span;
use std::sync::Arc;

use super::{CsvCache, Editor, HighlightCache, JsonCache, MarkdownCache, StickyScrollCache};
use crate::highlight::Highlighter;
use crate::keymap::Mode;
use crate::ui::{RenderContext, UI};

impl Editor {
    /// Render the UI
    pub(super) fn render(&mut self) -> Result<()> {
        let mode = self.mode;
        // Clone into owned Strings so these don't hold borrows on `self`
        // while we need a mutable borrow below to call scroll_to_cursor().
        let status_owned = self.status_message.clone();
        let status = status_owned.as_deref();
        let command_buffer_owned =
            if self.mode == Mode::Command { Some(self.command_buffer.clone()) } else { None };
        let command_buffer = command_buffer_owned.as_deref();

        // Check if we should show which-key
        let show_which_key = self.key_handler.should_show_which_key();
        let which_key_options =
            if show_which_key { Some(self.key_handler.which_key_options()) } else { None };

        // Get key sequence for display
        let key_sequence = self.key_handler.sequence();

        // ── Scroll to keep cursor in view ─────────────────────────────────────
        // Must happen before the buffer snapshot so scroll_row/col are current.
        //
        // viewport_height: subtract status line (1) and which-key popup (dynamic) when shown.
        //
        // viewport_width: we must match the three-panel layout produced by UI::render()
        // so that horizontal scrolling kicks in at the right column.  The layout is:
        //   explorer+agent → [Length(25), Min(1), Percentage(35)]
        //   explorer only  → [Length(25), Min(1)]
        //   agent only     → [Percentage(60), Percentage(40)]
        //   neither        → [Min(1)]
        // Then subtract 2 for the diagnostic gutter that is always prepended.
        let size = self.terminal.size().unwrap_or_default();
        // which-key popup: 2 (borders) + 1 (header) + number of options
        let wk_height = which_key_options.as_ref().map_or(0, |opts| opts.len() + 3);
        let viewport_height =
            (size.height as usize).saturating_sub(if show_which_key { wk_height + 1 } else { 1 });

        const GUTTER: usize = 2;
        let total_w = size.width as usize;
        let editor_area_w = match (self.file_explorer.visible, self.agent_panel.visible) {
            (true, true) => total_w.saturating_sub(25).saturating_sub(total_w * 35 / 100),
            (true, false) => total_w.saturating_sub(25),
            (false, true) => total_w * 60 / 100,
            (false, false) => total_w,
        };
        let viewport_width = editor_area_w.saturating_sub(GUTTER);

        if self.config.soft_wrap {
            // Text width = viewport_width (gutter already subtracted above).
            self.with_buffer(|buf| buf.scroll_to_cursor_wrapped(viewport_height, viewport_width));
        } else {
            self.with_buffer(|buf| buf.scroll_to_cursor(viewport_height, viewport_width));
        }

        // ── Fold data (ADR 0106) ──────────────────────────────────────────────
        // Populate the tree-sitter cache for the current buffer (no-op if already
        // current), then compute which rows are hidden and which are fold stubs.
        let buf_idx = self.current_buffer_idx;
        let _ = self.ts_tree_for_current_buffer(); // ensures ts_cache[buf_idx] is fresh

        let fold_ranges: Vec<(usize, usize)> = self
            .ts_cache
            .get(&buf_idx)
            .map(crate::treesitter::query::fold_ranges)
            .unwrap_or_default();

        let fold_closed_set = self.fold_closed.get(&buf_idx).cloned().unwrap_or_default();

        let mut fold_hidden_rows: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let mut fold_stub_map: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for &(start, end) in &fold_ranges {
            if fold_closed_set.contains(&start) {
                for row in (start + 1)..=end {
                    fold_hidden_rows.insert(row);
                }
                fold_stub_map.insert(start, end);
            }
        }

        // The fold_data passed to the renderer (None when no folds are active).
        let fold_data_owned: Option<crate::ui::FoldData> = if !fold_stub_map.is_empty() {
            Some(crate::ui::FoldData {
                hidden_rows: fold_hidden_rows.clone(),
                fold_starts: fold_stub_map,
            })
        } else {
            None
        };
        let fold_data_ref: Option<&crate::ui::FoldData> = fold_data_owned.as_ref();

        // ── Sticky scroll header (ADR 0107) ───────────────────────────────────
        let scroll_row_for_sticky = self.current_buffer().map(|b| b.scroll_row).unwrap_or(0);
        let lsp_ver_for_sticky = self.current_buffer().map(|b| b.lsp_version).unwrap_or(0);
        let cache_hit = self.sticky_scroll_cache.as_ref().is_some_and(|c| {
            c.buffer_idx == buf_idx
                && c.scroll_row == scroll_row_for_sticky
                && c.lsp_version == lsp_ver_for_sticky
        });
        if !cache_hit {
            let header = self.ts_cache.get(&buf_idx).and_then(|s| {
                crate::treesitter::query::sticky_scroll_header(s, scroll_row_for_sticky)
            });
            self.sticky_scroll_cache = Some(StickyScrollCache {
                buffer_idx: buf_idx,
                scroll_row: scroll_row_for_sticky,
                lsp_version: lsp_ver_for_sticky,
                header,
            });
        }
        let sticky_header_owned: Option<String> =
            self.sticky_scroll_cache.as_ref().and_then(|c| c.header.clone());
        let sticky_header_ref: Option<&str> = sticky_header_owned.as_deref();

        // ── Account for sticky header in viewport height ───────────────────────
        // When a sticky header is present it occupies 1 row; reduce the content
        // height used for scroll calculations and line-range clipping accordingly.
        let sticky_height: usize = if sticky_header_owned.is_some() { 1 } else { 0 };
        let content_viewport_height = viewport_height.saturating_sub(sticky_height);

        // Get buffer data before drawing to avoid borrow issues.
        // Extend the visible range by the number of hidden rows so that after
        // fold-skipping the renderer still has enough lines to fill the viewport.
        let buffer_data = self.current_buffer().map(|buf| {
            let extra = fold_hidden_rows.len();
            let vis_end = (buf.scroll_row + content_viewport_height + extra).min(buf.lines().len());

            // Adjust cursor.row to the visual row: subtract the number of hidden
            // rows that fall between scroll_row and the cursor row so that the
            // renderer positions the terminal cursor at the correct screen row.
            let hidden_before_cursor =
                (buf.scroll_row..buf.cursor.row).filter(|r| fold_hidden_rows.contains(r)).count();
            let mut visual_cursor = buf.cursor.clone();
            visual_cursor.row = buf.cursor.row.saturating_sub(hidden_before_cursor);

            (
                buf.name.clone(),
                buf.is_modified,
                visual_cursor,
                buf.scroll_row,
                buf.scroll_col,
                buf.lines()[buf.scroll_row..vis_end].to_vec(),
                buf.selection.clone(),
            )
        });

        // ── Syntax-highlight cache ─────────────────────────────────────────────
        // Re-use spans from the previous frame when the buffer content (lsp_version),
        // scroll position, and active buffer are all unchanged.  This eliminates the
        // ~3–8 ms syntect cost on every frame where the user is just moving the cursor.
        let term_height = viewport_height;

        // Collect the cache key from an immutable borrow (borrow ends before mut access).
        let cache_key = self.current_buffer().map(|buf| {
            let ext = buf.file_path.as_deref().map(Highlighter::extension_for).unwrap_or_default();
            let name = buf.file_path.as_deref().map(Highlighter::filename_for).unwrap_or_default();
            (buf.scroll_row, buf.lsp_version, ext, name)
        });

        let highlighted_lines: Option<Arc<Vec<Vec<Span<'static>>>>> =
            if let Some((scroll_row, lsp_ver, ext, name)) = cache_key {
                // Cache hit only when: same buffer, same scroll position, same content version,
                // AND the cached range covers at least as many rows as we now need
                // (the range grows when folds are active to cover hidden rows).
                let hl_extra = fold_hidden_rows.len();
                let required_end = (scroll_row + term_height + hl_extra).min(usize::MAX);
                let cache_hit = self.highlight_cache.as_ref().is_some_and(|c| {
                    c.buffer_idx == buf_idx
                        && c.scroll_row == scroll_row
                        && c.lsp_version == lsp_ver
                        && c.spans.len() >= required_end.saturating_sub(scroll_row)
                });

                if cache_hit {
                    // Cache hit: Arc::clone is a single atomic increment — zero allocation.
                    self.highlight_cache.as_ref().map(|c| Arc::clone(&c.spans))
                } else {
                    // Cache miss: run syntect for the visible window and store result.
                    // Extend the range by the number of hidden (fold-skipped) rows so
                    // that the renderer can look up highlights for all visible buffer rows
                    // using `line_idx = buf_row - scroll_row` even when folds are active.
                    let spans = if let Some(buf) = self.current_buffer() {
                        let hl_extra = fold_hidden_rows.len();
                        let end = (scroll_row + term_height + hl_extra).min(buf.lines().len());
                        buf.lines()[scroll_row..end]
                            .iter()
                            .map(|line| self.highlighter.highlight_line(line, &ext, &name))
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    let arc = Arc::new(spans);
                    self.highlight_cache = Some(HighlightCache {
                        buffer_idx: buf_idx,
                        scroll_row,
                        lsp_version: lsp_ver,
                        spans: Arc::clone(&arc),
                    });
                    Some(arc)
                }
            } else {
                None
            };

        // Buffer list for PickBuffer mode
        let buffer_list = if self.mode == Mode::PickBuffer {
            Some((
                self.buffers.iter().map(|b| (b.name.clone(), b.is_modified)).collect::<Vec<_>>(),
                self.buffer_picker_idx,
            ))
        } else {
            None
        };

        // File list for PickFile mode
        let file_list = if self.mode == Mode::PickFile {
            Some((self.file_list.clone(), self.file_picker_idx, self.file_query.clone()))
        } else {
            None
        };

        let ghost = self.ghost_text.as_ref().map(|(text, row, col)| (text.as_str(), *row, *col));

        // ── Markdown preview lines ─────────────────────────────────────────────
        // Computed when in MarkdownPreview mode; cached by (lsp_version, viewport_width)
        // so markdown re-parsing is skipped on frames where nothing changed.
        let preview_lines_owned: Option<Vec<ratatui::text::Line<'static>>> = if mode
            == Mode::MarkdownPreview
        {
            let all_lines = {
                let buf_idx = self.current_buffer_idx;
                let key = self.current_buffer().map(|buf| buf.lsp_version);
                let cache_hit = self.markdown_cache.as_ref().is_some_and(|c| {
                    c.buffer_idx == buf_idx
                        && Some(c.lsp_version) == key
                        && c.viewport_width == viewport_width
                });
                if cache_hit {
                    self.markdown_cache.as_ref().unwrap().lines.clone()
                } else {
                    let ver = key.unwrap_or(0);
                    let content =
                        self.current_buffer().map(|buf| buf.lines().join("\n")).unwrap_or_default();
                    let rendered = crate::markdown::render(&content, viewport_width);
                    self.markdown_cache = Some(MarkdownCache {
                        buffer_idx: buf_idx,
                        lsp_version: ver,
                        viewport_width,
                        lines: rendered.clone(),
                    });
                    rendered
                }
            };
            // Cap scroll so we can't scroll past the end.
            let max_scroll = all_lines.len().saturating_sub(1);
            let scroll = self.preview_scroll.min(max_scroll);
            Some(all_lines.into_iter().skip(scroll).collect())
        } else if mode == Mode::CsvPreview {
            // ── CSV preview lines ──────────────────────────────────────────────
            let all_lines = {
                let buf_idx = self.current_buffer_idx;
                let key = self.current_buffer().map(|buf| buf.lsp_version);
                let cache_hit = self
                    .csv_cache
                    .as_ref()
                    .is_some_and(|c| c.buffer_idx == buf_idx && Some(c.lsp_version) == key);
                if cache_hit {
                    self.csv_cache.as_ref().unwrap().lines.clone()
                } else {
                    let ver = key.unwrap_or(0);
                    let content =
                        self.current_buffer().map(|buf| buf.lines().join("\n")).unwrap_or_default();
                    let rendered = crate::csv_preview::render(&content);
                    self.csv_cache = Some(CsvCache {
                        buffer_idx: buf_idx,
                        lsp_version: ver,
                        lines: rendered.clone(),
                    });
                    rendered
                }
            };
            let max_scroll = all_lines.len().saturating_sub(1);
            let scroll = self.preview_scroll.min(max_scroll);
            Some(all_lines.into_iter().skip(scroll).collect())
        } else if mode == Mode::JsonPreview {
            // ── JSON preview lines ─────────────────────────────────────────────
            let all_lines = {
                let buf_idx = self.current_buffer_idx;
                let key = self.current_buffer().map(|buf| buf.lsp_version);
                let cache_hit = self
                    .json_cache
                    .as_ref()
                    .is_some_and(|c| c.buffer_idx == buf_idx && Some(c.lsp_version) == key);
                if cache_hit {
                    self.json_cache.as_ref().unwrap().lines.clone()
                } else {
                    let ver = key.unwrap_or(0);
                    let content =
                        self.current_buffer().map(|buf| buf.lines().join("\n")).unwrap_or_default();
                    let rendered = crate::json_preview::render(&content);
                    self.json_cache = Some(JsonCache {
                        buffer_idx: buf_idx,
                        lsp_version: ver,
                        lines: rendered.clone(),
                    });
                    rendered
                }
            };
            let max_scroll = all_lines.len().saturating_sub(1);
            let scroll = self.preview_scroll.min(max_scroll);
            Some(all_lines.into_iter().skip(scroll).collect())
        } else {
            None
        };

        // ── Split pane data ────────────────────────────────────────────────────
        // Same viewport-clipped approach as the primary buffer: only the visible
        // rows are cloned.
        let split_buffer_data = self.split.other_idx.and_then(|idx| {
            self.buffers.get(idx).map(|buf| {
                let vis_end = (buf.scroll_row + viewport_height).min(buf.lines().len());
                (
                    buf.name.clone(),
                    buf.is_modified,
                    buf.cursor.clone(),
                    buf.scroll_row,
                    buf.scroll_col,
                    buf.lines()[buf.scroll_row..vis_end].to_vec(),
                    buf.selection.clone(),
                )
            })
        });

        // ── Split highlight cache ──────────────────────────────────────────────
        let split_highlighted_lines: Option<Arc<Vec<Vec<ratatui::text::Span<'static>>>>> =
            if let Some(split_idx) = self.split.other_idx {
                if let Some(split_buf) = self.buffers.get(split_idx) {
                    let split_scroll = split_buf.scroll_row;
                    let split_ver = split_buf.lsp_version;
                    let split_ext = split_buf
                        .file_path
                        .as_deref()
                        .map(Highlighter::extension_for)
                        .unwrap_or_default();
                    let split_name = split_buf
                        .file_path
                        .as_deref()
                        .map(Highlighter::filename_for)
                        .unwrap_or_default();
                    let cache_hit = self.split.highlight_cache.as_ref().is_some_and(|c| {
                        c.buffer_idx == split_idx
                            && c.scroll_row == split_scroll
                            && c.lsp_version == split_ver
                    });
                    if cache_hit {
                        // Cache hit: Arc::clone is a single atomic increment — zero allocation.
                        self.split.highlight_cache.as_ref().map(|c| Arc::clone(&c.spans))
                    } else {
                        let end = (split_scroll + term_height).min(split_buf.lines().len());
                        let spans: Vec<Vec<ratatui::text::Span<'static>>> = split_buf.lines()
                            [split_scroll..end]
                            .iter()
                            .map(|line| {
                                self.highlighter.highlight_line(line, &split_ext, &split_name)
                            })
                            .collect();
                        let arc = Arc::new(spans);
                        self.split.highlight_cache = Some(HighlightCache {
                            buffer_idx: split_idx,
                            scroll_row: split_scroll,
                            lsp_version: split_ver,
                            spans: Arc::clone(&arc),
                        });
                        Some(arc)
                    }
                } else {
                    None
                }
            } else {
                None
            };

        let split_right_focused = self.split.right_focused;

        let agent_ref = if self.agent_panel.visible { Some(&self.agent_panel) } else { None };
        let explorer_ref =
            if self.file_explorer.visible { Some(&self.file_explorer) } else { None };
        let hl_ref: Option<&[Vec<Span<'static>>]> = highlighted_lines.as_deref().map(Vec::as_slice);
        let split_hl_ref: Option<&[Vec<Span<'static>>]> =
            split_highlighted_lines.as_deref().map(Vec::as_slice);
        let preview_ref = preview_lines_owned.as_deref();
        let search_ref = if mode == Mode::Search { Some(&self.search_state) } else { None };
        let rename_buf_owned =
            if mode == Mode::RenameFile { Some(self.rename_buffer.clone()) } else { None };
        let rename_buf = rename_buf_owned.as_deref();

        let new_folder_buf_owned =
            if mode == Mode::NewFolder { Some(self.new_folder_buffer.clone()) } else { None };
        let new_folder_buf = new_folder_buf_owned.as_deref();

        let delete_path_owned = if mode == Mode::DeleteFile {
            self.delete_confirm_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        } else {
            None
        };
        let delete_name = delete_path_owned.as_deref();

        let commit_msg_buf =
            if mode == Mode::CommitMsg { Some(self.commit_msg.buffer.as_str()) } else { None };

        let release_notes_view = if mode == Mode::ReleaseNotes {
            Some(crate::ui::ReleaseNotesView {
                count_input: self.release_notes.count_input.as_str(),
                generating: self.release_notes.rx.is_some(),
                notes: self.release_notes.buffer.as_str(),
                scroll: self.release_notes.scroll,
            })
        } else {
            None
        };

        let log_path_buf = crate::config::Config::log_path()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/forgiven.log"));
        let log_path_str = log_path_buf.to_string_lossy().into_owned();
        let mcp_failed_empty: Vec<(String, String)> = Vec::new();
        let recent_logs_owned: Vec<(String, String)> =
            self.log_buffer.lock().map(|g| g.iter().cloned().collect()).unwrap_or_default();
        let diag_overlay = if mode == Mode::Diagnostics {
            let mcp_connected =
                self.mcp_manager.as_ref().map(|m| m.connected_servers()).unwrap_or_default();
            let mcp_failed: &[(String, String)] = self
                .mcp_manager
                .as_ref()
                .map(|m| m.failed_servers.as_slice())
                .unwrap_or(mcp_failed_empty.as_slice());
            let lsp_servers =
                self.config.lsp.servers.iter().map(|s| s.language.as_str()).collect::<Vec<_>>();
            let agent_session_tokens = if self.agent_panel.session_rounds > 0 {
                Some((
                    self.agent_panel.total_session_prompt_tokens,
                    self.agent_panel.total_session_completion_tokens,
                    self.agent_panel.context_window_size(),
                    self.agent_panel.session_rounds,
                ))
            } else {
                None
            };
            Some(crate::ui::DiagnosticsData {
                version: env!("CARGO_PKG_VERSION"),
                mcp_connected,
                mcp_failed,
                lsp_servers,
                log_path: &log_path_str,
                recent_logs: recent_logs_owned.as_slice(),
                agent_session_tokens,
                agent_ctx_breakdown: self.agent_panel.last_breakdown,
                observation_mask_threshold_chars: self
                    .config
                    .agent
                    .observation_mask_threshold_chars,
                mcp_call_log: self
                    .mcp_manager
                    .as_ref()
                    .map(|m| m.recent_calls())
                    .unwrap_or_default(),
                tool_retrieval_counts: if self.agent_panel.session_rounds > 0 {
                    Some((
                        self.agent_panel.session_read_file_count,
                        self.agent_panel.session_symbol_count,
                        self.agent_panel.session_outline_count,
                    ))
                } else {
                    None
                },
            })
        } else {
            None
        };

        // File-info popup: stat the selected explorer entry once per frame while active.
        // `fs::metadata` on a local filesystem is effectively instantaneous (~1 µs).
        let file_info_data: Option<crate::ui::FileInfoData> = if self.show_file_info {
            self.file_explorer.selected_path().and_then(|path| {
                std::fs::metadata(&path).ok().map(|meta| {
                    #[cfg(unix)]
                    let permissions = {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = meta.permissions().mode();
                        let bits: &[(u32, char)] = &[
                            (0o400, 'r'),
                            (0o200, 'w'),
                            (0o100, 'x'),
                            (0o040, 'r'),
                            (0o020, 'w'),
                            (0o010, 'x'),
                            (0o004, 'r'),
                            (0o002, 'w'),
                            (0o001, 'x'),
                        ];
                        Some(
                            bits.iter()
                                .map(|(mask, ch)| if mode & mask != 0 { *ch } else { '-' })
                                .collect::<String>(),
                        )
                    };
                    #[cfg(not(unix))]
                    let permissions: Option<String> = None;
                    crate::ui::FileInfoData {
                        is_dir: meta.is_dir(),
                        size_bytes: if meta.is_file() { Some(meta.len()) } else { None },
                        modified: meta.modified().ok(),
                        created: meta.created().ok(),
                        permissions,
                        path,
                    }
                })
            })
        } else {
            None
        };

        self.terminal.draw(|frame| {
            let ctx = RenderContext {
                mode,
                buffer_data: buffer_data.as_ref(),
                status_message: status,
                command_buffer,
                which_key_options: which_key_options.as_deref(),
                key_sequence: key_sequence.as_str(),
                buffer_list: buffer_list.as_ref(),
                file_list: file_list.as_ref(),
                diagnostics: &self.current_diagnostics,
                ghost_text: ghost,
                agent_panel: agent_ref,
                highlighted_lines: hl_ref,
                file_explorer: explorer_ref,
                preview_lines: preview_ref,
                search_state: search_ref,
                rename_buffer: rename_buf,
                delete_name,
                new_folder_buffer: new_folder_buf,
                split_buffer_data: split_buffer_data.as_ref(),
                split_highlighted_lines: split_hl_ref,
                split_right_focused,
                commit_msg: commit_msg_buf,
                release_notes: release_notes_view.as_ref(),
                diag_overlay: diag_overlay.as_ref(),
                binary_file_path: self.binary_file_path.as_deref(),
                startup_elapsed: self.startup_elapsed,
                file_info: file_info_data.as_ref(),
                location_list: if mode == Mode::LocationList {
                    self.location_list.as_ref()
                } else {
                    None
                },
                in_file_search_query: if mode == Mode::InFileSearch {
                    Some(self.in_file_search_buffer.as_str())
                } else {
                    None
                },
                hover_popup: if mode == Mode::LspHover { self.hover_popup.as_ref() } else { None },
                lsp_rename_buffer: if mode == Mode::LspRename {
                    Some(self.lsp_rename_buffer.as_str())
                } else {
                    None
                },
                fold_data: fold_data_ref,
                sticky_header: sticky_header_ref,
                inline_assist: self.inline_assist.as_ref().map(|s| crate::ui::InlineAssistView {
                    prompt: &s.prompt,
                    response: &s.response,
                    phase: s.phase,
                }),
                review_changes: if mode == Mode::ReviewChanges {
                    self.review_changes.as_ref()
                } else {
                    None
                },
                insights_dashboard: if mode == Mode::InsightsDashboard {
                    self.insights_dashboard.as_ref()
                } else {
                    None
                },
                soft_wrap: self.config.soft_wrap,
            };
            UI::render(frame, &ctx);
        })?;

        Ok(())
    }
}
