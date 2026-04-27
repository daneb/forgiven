use anyhow::Result;
use ratatui::text::Span;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{Editor, FoldCache, HighlightCache, MarkdownCache, StickyScrollCache};
use crate::highlight::Highlighter;
use crate::keymap::Mode;
use crate::ui::{RenderContext, UI};

// ── Fold data helpers (ADR 0138) ──────────────────────────────────────────────

/// Compute which rows are hidden and which rows are fold stubs given the
/// tree-sitter fold ranges and the set of currently-closed fold start rows.
///
/// Pure function — no `self`, no side effects, fully unit-testable.
/// Called by `render()` on a cache miss; result stored in `FoldCache`.
pub(crate) fn compute_fold_data(
    fold_ranges: &[(usize, usize)],
    fold_closed_set: &HashSet<usize>,
) -> (HashSet<usize>, HashMap<usize, usize>) {
    let mut hidden_rows = HashSet::new();
    let mut stub_map = HashMap::new();
    for &(start, end) in fold_ranges {
        if fold_closed_set.contains(&start) {
            for row in (start + 1)..=end {
                hidden_rows.insert(row);
            }
            stub_map.insert(start, end);
        }
    }
    (hidden_rows, stub_map)
}

/// Cheap fingerprint for a set of closed fold start rows.
/// XOR of all values — O(k), collision-safe enough for a render cache.
pub(crate) fn fold_fingerprint(closed: &HashSet<usize>) -> u64 {
    closed.iter().fold(0u64, |acc, &v| acc ^ (v as u64))
}

// ── Preview cache helper (ADR 0138) ───────────────────────────────────────────

/// Cache-or-render for preview modes (Markdown, CSV, JSON).
///
/// On a cache hit (`key_matches` returns `true`) the stored lines are cloned
/// and returned without calling `render_fn`.  On a miss `render_fn` is called
/// once, the result is stored via `make_cache`, and the same lines are returned.
///
/// Free function (not a method) so the caller can extract fields from `self`
/// before passing a mutable borrow of the cache field — avoiding split-borrow
/// conflicts.
pub(crate) fn cached_preview<C: PreviewLines>(
    cache: &mut Option<C>,
    key_matches: impl Fn(&C) -> bool,
    render_fn: impl FnOnce() -> Vec<ratatui::text::Line<'static>>,
    make_cache: impl FnOnce(Vec<ratatui::text::Line<'static>>) -> C,
) -> Vec<ratatui::text::Line<'static>> {
    if cache.as_ref().is_some_and(key_matches) {
        cache.as_ref().unwrap().lines().to_vec()
    } else {
        let rendered = render_fn();
        *cache = Some(make_cache(rendered.clone()));
        rendered
    }
}

/// Trait so `cached_preview` can access the stored lines from any cache type
/// without knowing its concrete fields.
pub(crate) trait PreviewLines {
    fn lines(&self) -> &[ratatui::text::Line<'static>];
}

impl PreviewLines for MarkdownCache {
    fn lines(&self) -> &[ratatui::text::Line<'static>] {
        &self.lines
    }
}

impl Editor {
    /// Ensure FoldCache is current and return `(hidden_rows, stub_map)`.
    ///
    /// Caller must have already called `ts_tree_for_current_buffer()` so that
    /// `ts_cache[buf_idx]` is fresh before this is invoked.
    fn render_fold_data(&mut self, buf_idx: usize) -> (HashSet<usize>, HashMap<usize, usize>) {
        let lsp_ver = self.current_buffer().map(|b| b.lsp_version).unwrap_or(0);
        let closed = self.fold_closed.get(&buf_idx).cloned().unwrap_or_default();
        let fingerprint = fold_fingerprint(&closed);
        let cache_hit = self.fold_cache.as_ref().is_some_and(|c| {
            c.buffer_idx == buf_idx && c.lsp_version == lsp_ver && c.fold_fingerprint == fingerprint
        });
        if !cache_hit {
            let ranges: Vec<(usize, usize)> = self
                .ts_cache
                .get(&buf_idx)
                .map(crate::treesitter::query::fold_ranges)
                .unwrap_or_default();
            let (hidden_rows, stub_map) = compute_fold_data(&ranges, &closed);
            self.fold_cache = Some(FoldCache {
                buffer_idx: buf_idx,
                lsp_version: lsp_ver,
                fold_fingerprint: fingerprint,
                hidden_rows,
                stub_map,
            });
        }
        self.fold_cache
            .as_ref()
            .map(|c| (c.hidden_rows.clone(), c.stub_map.clone()))
            .unwrap_or_default()
    }

    /// Ensure StickyScrollCache is current and return the header string.
    ///
    /// Returns `None` when there is no enclosing scope at the current scroll
    /// position, or when tree-sitter has no parse for the buffer.
    fn render_sticky_scroll(&mut self, buf_idx: usize) -> Option<String> {
        let scroll_row = self.current_buffer().map(|b| b.scroll_row).unwrap_or(0);
        let lsp_ver = self.current_buffer().map(|b| b.lsp_version).unwrap_or(0);
        let cache_hit = self.sticky_scroll_cache.as_ref().is_some_and(|c| {
            c.buffer_idx == buf_idx && c.scroll_row == scroll_row && c.lsp_version == lsp_ver
        });
        if !cache_hit {
            let header = self
                .ts_cache
                .get(&buf_idx)
                .and_then(|s| crate::treesitter::query::sticky_scroll_header(s, scroll_row));
            self.sticky_scroll_cache = Some(StickyScrollCache {
                buffer_idx: buf_idx,
                scroll_row,
                lsp_version: lsp_ver,
                header,
            });
        }
        self.sticky_scroll_cache.as_ref().and_then(|c| c.header.clone())
    }

    /// Ensure HighlightCache is current and return an Arc to the span vec.
    ///
    /// `fold_hidden_rows` is needed to extend the highlighted range beyond the
    /// visible viewport so the renderer can look up spans for fold-skipped rows.
    fn render_highlight_spans(
        &mut self,
        buf_idx: usize,
        fold_hidden_rows: &HashSet<usize>,
        term_height: usize,
    ) -> Option<Arc<Vec<Vec<Span<'static>>>>> {
        // Collect cache key via immutable borrow — borrow ends before mutable write.
        let cache_key = self.current_buffer().map(|buf| {
            let ext = buf.file_path.as_deref().map(Highlighter::extension_for).unwrap_or_default();
            let name = buf.file_path.as_deref().map(Highlighter::filename_for).unwrap_or_default();
            (buf.scroll_row, buf.lsp_version, ext, name)
        });
        let (scroll_row, lsp_ver, ext, name) = cache_key?;
        let hl_extra = fold_hidden_rows.len();
        let required_end = scroll_row + term_height + hl_extra;
        let cache_hit = self.highlight_cache.as_ref().is_some_and(|c| {
            c.buffer_idx == buf_idx
                && c.scroll_row == scroll_row
                && c.lsp_version == lsp_ver
                && c.spans.len() >= required_end.saturating_sub(scroll_row)
        });
        if cache_hit {
            return self.highlight_cache.as_ref().map(|c| Arc::clone(&c.spans));
        }
        // Cache miss: run syntect for the visible window + hidden fold rows.
        let spans = if let Some(buf) = self.current_buffer() {
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

    /// Return the scrolled preview lines for MarkdownPreview mode.
    /// Returns `None` for all other modes.
    fn render_preview_lines(
        &mut self,
        mode: Mode,
        buf_idx: usize,
        viewport_width: usize,
    ) -> Option<Vec<ratatui::text::Line<'static>>> {
        if mode == Mode::MarkdownPreview {
            let lsp_ver = self.current_buffer().map(|b| b.lsp_version);
            let ver = lsp_ver.unwrap_or(0);
            let content =
                self.current_buffer().map(|buf| buf.lines().join("\n")).unwrap_or_default();
            let highlighter_ref = &self.highlighter;
            let vw = viewport_width;
            let all_lines = cached_preview(
                &mut self.markdown_cache,
                |c| {
                    c.buffer_idx == buf_idx
                        && Some(c.lsp_version) == lsp_ver
                        && c.viewport_width == vw
                },
                || crate::markdown::render(&content, vw, Some(highlighter_ref)),
                |lines| MarkdownCache {
                    buffer_idx: buf_idx,
                    lsp_version: ver,
                    viewport_width: vw,
                    lines,
                },
            );
            let scroll = self.preview_scroll.min(all_lines.len().saturating_sub(1));
            Some(all_lines.into_iter().skip(scroll).collect())
        } else {
            None
        }
    }

    /// Ensure the split-pane HighlightCache is current and return an Arc to
    /// the span vec.  Returns `None` when no split is active.
    fn render_split_highlight(
        &mut self,
        term_height: usize,
    ) -> Option<Arc<Vec<Vec<Span<'static>>>>> {
        let split_idx = self.split.other_idx?;
        // Collect key fields from an immutable borrow before any mutable write.
        let (split_scroll, split_ver, split_ext, split_name) = {
            let buf = self.buffers.get(split_idx)?;
            (
                buf.scroll_row,
                buf.lsp_version,
                buf.file_path.as_deref().map(Highlighter::extension_for).unwrap_or_default(),
                buf.file_path.as_deref().map(Highlighter::filename_for).unwrap_or_default(),
            )
        };
        let cache_hit = self.split.highlight_cache.as_ref().is_some_and(|c| {
            c.buffer_idx == split_idx && c.scroll_row == split_scroll && c.lsp_version == split_ver
        });
        if cache_hit {
            return self.split.highlight_cache.as_ref().map(|c| Arc::clone(&c.spans));
        }
        let end = {
            let buf = self.buffers.get(split_idx)?;
            (split_scroll + term_height).min(buf.lines().len())
        };
        // Re-borrow for the highlight pass — separate from the cache write below.
        let spans: Vec<Vec<Span<'static>>> = {
            let buf = self.buffers.get(split_idx)?;
            buf.lines()[split_scroll..end]
                .iter()
                .map(|line| self.highlighter.highlight_line(line, &split_ext, &split_name))
                .collect()
        };
        let arc = Arc::new(spans);
        self.split.highlight_cache = Some(HighlightCache {
            buffer_idx: split_idx,
            scroll_row: split_scroll,
            lsp_version: split_ver,
            spans: Arc::clone(&arc),
        });
        Some(arc)
    }

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

        // ── Fold data (ADR 0106, cached per ADR 0138) ────────────────────────
        let buf_idx = self.current_buffer_idx;
        let _ = self.ts_tree_for_current_buffer(); // ensures ts_cache[buf_idx] is fresh
        let (fold_hidden_rows, fold_stub_map) = self.render_fold_data(buf_idx);

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
        let sticky_header_owned: Option<String> = self.render_sticky_scroll(buf_idx);
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
        let term_height = viewport_height;
        let highlighted_lines =
            self.render_highlight_spans(buf_idx, &fold_hidden_rows, term_height);

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

        // Clone ghost text so the borrow on self.ghost_text doesn't conflict
        // with the &mut self borrow taken by render_preview_lines below.
        let ghost_owned =
            self.ghost_text.as_ref().map(|(text, row, col)| (text.clone(), *row, *col));
        let ghost = ghost_owned.as_ref().map(|(text, row, col)| (text.as_str(), *row, *col));

        // ── Preview lines (Markdown / CSV / JSON) — ADR 0138 ─────────────────
        let preview_lines_owned = self.render_preview_lines(mode, buf_idx, viewport_width);

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
        let split_highlighted_lines = self.render_split_highlight(term_height);

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
                sidecar_status: (
                    self.sidecar.is_some(),
                    self.companion_process.is_some(),
                    self.sidecar_client_connected,
                ),
                codified_context_info: if self.agent_panel.codified_context_enabled {
                    let (ctokens, scount, kcount) = self
                        .agent_panel
                        .codified_context
                        .as_ref()
                        .map(|cc| {
                            (
                                cc.constitution.as_ref().map(|c| c.token_estimate).unwrap_or(0),
                                cc.specialists.len(),
                                cc.knowledge_docs.len(),
                            )
                        })
                        .unwrap_or((0, 0, 0));
                    Some((
                        ctokens,
                        self.agent_panel.codified_context_constitution_max_tokens,
                        scount,
                        kcount,
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
                diagnostics: &self.lsp.diagnostics,
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
                commit_msg_cursor: self.commit_msg.cursor,
                release_notes: release_notes_view.as_ref(),
                diag_overlay: diag_overlay.as_ref(),
                binary_file_path: self.binary_file_path.as_deref(),
                startup_elapsed: self.startup_elapsed,
                file_info: file_info_data.as_ref(),
                location_list: if mode == Mode::LocationList {
                    self.lsp.location_list.as_ref()
                } else {
                    None
                },
                in_file_search_query: if mode == Mode::InFileSearch {
                    Some(self.in_file_search_buffer.as_str())
                } else {
                    None
                },
                hover_popup: if mode == Mode::LspHover {
                    self.lsp.hover_popup.as_ref()
                } else {
                    None
                },
                lsp_rename_buffer: if mode == Mode::LspRename {
                    Some(self.lsp.rename_buffer.as_str())
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
                highlighter: &self.highlighter,
            };
            UI::render(frame, &ctx);
        })?;

        Ok(())
    }
}

// ── Tests (ADR 0138) ──────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    // ── compute_fold_data ────────────────────────────────────────────────────

    #[test]
    fn fold_no_closed_ranges_produces_empty_sets() {
        let ranges = vec![(2usize, 5usize), (10, 15)];
        let closed: HashSet<usize> = HashSet::new();
        let (hidden, stubs) = compute_fold_data(&ranges, &closed);
        assert!(hidden.is_empty(), "no closed folds → no hidden rows");
        assert!(stubs.is_empty(), "no closed folds → no stubs");
    }

    #[test]
    fn fold_closed_range_hides_interior_rows() {
        let ranges = vec![(2usize, 5usize)];
        let closed: HashSet<usize> = [2].into();
        let (hidden, stubs) = compute_fold_data(&ranges, &closed);
        // rows 3, 4, 5 are hidden; row 2 (the start) is NOT hidden — it becomes the stub
        assert!(!hidden.contains(&2), "start row must not be hidden");
        assert!(hidden.contains(&3));
        assert!(hidden.contains(&4));
        assert!(hidden.contains(&5));
        assert_eq!(hidden.len(), 3);
        assert_eq!(stubs.get(&2), Some(&5), "stub maps start → end");
    }

    #[test]
    fn fold_open_range_not_in_hidden_or_stubs() {
        let ranges = vec![(0usize, 3usize), (5, 8)];
        // Only close the second range
        let closed: HashSet<usize> = [5].into();
        let (hidden, stubs) = compute_fold_data(&ranges, &closed);
        // Rows 1-3 must NOT be hidden (first fold is open)
        assert!(!hidden.contains(&1));
        assert!(!hidden.contains(&2));
        assert!(!hidden.contains(&3));
        assert!(!stubs.contains_key(&0));
        // Rows 6-8 must be hidden
        assert!(hidden.contains(&6));
        assert!(hidden.contains(&7));
        assert!(hidden.contains(&8));
        assert_eq!(stubs.get(&5), Some(&8));
    }

    #[test]
    fn fold_multiple_closed_ranges() {
        let ranges = vec![(0usize, 2usize), (5, 7)];
        let closed: HashSet<usize> = [0, 5].into();
        let (hidden, stubs) = compute_fold_data(&ranges, &closed);
        // First fold: rows 1-2 hidden, start 0 → stub 2
        // Second fold: rows 6-7 hidden, start 5 → stub 7
        assert_eq!(hidden.len(), 4);
        assert!(hidden.contains(&1) && hidden.contains(&2));
        assert!(hidden.contains(&6) && hidden.contains(&7));
        assert_eq!(stubs.len(), 2);
        assert_eq!(stubs.get(&0), Some(&2));
        assert_eq!(stubs.get(&5), Some(&7));
    }

    #[test]
    fn fold_start_row_never_in_hidden_set() {
        // Regression: the start row of a closed fold must be the stub line shown
        // to the user, not hidden. This would cause an invisible cursor if wrong.
        let ranges = vec![(10usize, 20usize)];
        let closed: HashSet<usize> = [10].into();
        let (hidden, _) = compute_fold_data(&ranges, &closed);
        assert!(!hidden.contains(&10), "start row must never be hidden — it is the stub line");
    }

    // ── fold_fingerprint ─────────────────────────────────────────────────────

    #[test]
    fn fold_fingerprint_empty_set_is_zero() {
        assert_eq!(fold_fingerprint(&HashSet::new()), 0);
    }

    #[test]
    fn fold_fingerprint_changes_when_set_changes() {
        let a: HashSet<usize> = [1, 2, 3].into();
        let b: HashSet<usize> = [1, 2, 4].into();
        assert_ne!(fold_fingerprint(&a), fold_fingerprint(&b));
    }

    #[test]
    fn fold_fingerprint_order_independent() {
        // XOR is commutative — insertion order must not matter.
        let a: HashSet<usize> = [10, 20, 30].into();
        let b: HashSet<usize> = [30, 10, 20].into();
        assert_eq!(fold_fingerprint(&a), fold_fingerprint(&b));
    }

    // ── cached_preview ───────────────────────────────────────────────────────

    // Minimal concrete cache type for testing the generic helper.
    struct TestCache {
        key: u32,
        stored_lines: Vec<Line<'static>>,
    }
    impl PreviewLines for TestCache {
        fn lines(&self) -> &[Line<'static>] {
            &self.stored_lines
        }
    }

    fn make_line(s: &'static str) -> Line<'static> {
        Line::from(s)
    }

    #[test]
    fn preview_cache_miss_calls_render_fn_once() {
        let mut cache: Option<TestCache> = None;
        let mut call_count = 0u32;
        let result = cached_preview(
            &mut cache,
            |_| false,
            || {
                call_count += 1;
                vec![make_line("hello")]
            },
            |lines| TestCache { key: 1, stored_lines: lines },
        );
        assert_eq!(call_count, 1, "render_fn must be called exactly once on miss");
        assert_eq!(result.len(), 1);
        assert!(cache.is_some(), "cache must be populated after miss");
    }

    #[test]
    fn preview_cache_hit_skips_render_fn() {
        let mut cache: Option<TestCache> =
            Some(TestCache { key: 42, stored_lines: vec![make_line("cached")] });
        let mut call_count = 0u32;
        let result = cached_preview(
            &mut cache,
            |c| c.key == 42,
            || {
                call_count += 1;
                vec![make_line("fresh")]
            },
            |lines| TestCache { key: 42, stored_lines: lines },
        );
        assert_eq!(call_count, 0, "render_fn must not be called on cache hit");
        assert_eq!(result.len(), 1);
        // Must have returned the cached value, not the fresh one
        assert_eq!(result[0], make_line("cached"));
    }

    #[test]
    fn preview_cache_invalidates_on_key_change() {
        let mut cache: Option<TestCache> =
            Some(TestCache { key: 1, stored_lines: vec![make_line("old")] });
        let mut call_count = 0u32;
        // key changed from 1 → 2: should miss
        let result = cached_preview(
            &mut cache,
            |c| c.key == 2,
            || {
                call_count += 1;
                vec![make_line("new")]
            },
            |lines| TestCache { key: 2, stored_lines: lines },
        );
        assert_eq!(call_count, 1, "render_fn must be called when key changes");
        assert_eq!(result[0], make_line("new"));
        assert_eq!(cache.as_ref().unwrap().key, 2, "cache must be updated with new key");
    }
}
