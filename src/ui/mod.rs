use lsp_types::Diagnostic;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::path::PathBuf;

use crate::agent::{
    split_thinking, AgentPanel, AgentTask, AskUserState, AtPickerState, ContentSegment, Role,
    SlashMenuState,
};
use crate::buffer::{Cursor, Selection};
use crate::editor::DiffLine;
use crate::explorer::FileExplorer;
use crate::keymap::Mode;
use crate::search::{SearchFocus, SearchState, SearchStatus};

/// Per-frame render cache for the agent panel.
/// Avoids re-parsing markdown and re-running split_thinking() for messages
/// that have not changed since the last frame.
struct PanelRenderCache {
    /// Completed-message lines + their exact ratatui row count.
    /// Valid when (msg_count, content_width) match.
    msg_count: usize,
    content_width: usize,
    msg_lines: Vec<Line<'static>>,
    msg_row_count: usize,
    /// Streaming-reply lines + their exact ratatui row count.
    /// Valid when (streaming_len, streaming_width) match.
    streaming_len: usize,
    streaming_width: usize,
    streaming_lines: Vec<Line<'static>>,
    streaming_row_count: usize,
    /// Cached MCP status bottom-bar line.
    /// Valid when (connected_count, failed_count) match.
    mcp_status_key: (usize, usize),
    mcp_bottom: Option<Line<'static>>,
}

impl Default for PanelRenderCache {
    fn default() -> Self {
        Self {
            msg_count: usize::MAX, // force initial render
            content_width: 0,
            msg_lines: Vec::new(),
            msg_row_count: 0,
            streaming_len: usize::MAX, // force initial render
            streaming_width: 0,
            streaming_lines: Vec::new(),
            streaming_row_count: 0,
            mcp_status_key: (usize::MAX, usize::MAX),
            mcp_bottom: None,
        }
    }
}

/// Returns the exact number of terminal rows that `lines` would occupy when
/// rendered in a [`Paragraph`] with `Wrap { trim: false }` at `inner_width`
/// columns.  Falls back to `lines.len()` (one row each) when `inner_width`
/// is zero to avoid a division-by-zero in ratatui's layout code.
fn wrapped_line_count(lines: &[Line<'static>], inner_width: usize) -> usize {
    if inner_width == 0 || lines.is_empty() {
        return lines.len();
    }
    Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false }).line_count(inner_width as u16)
}

thread_local! {
    static PANEL_CACHE: std::cell::RefCell<PanelRenderCache> =
        std::cell::RefCell::new(PanelRenderCache::default());
}

/// Data for the full-screen apply-diff overlay (Mode::ApplyDiff).
pub struct ApplyDiffView<'a> {
    pub target: &'a str,
    pub lines: &'a [DiffLine],
    pub scroll: usize,
}

/// Data for the release notes popup (Mode::ReleaseNotes).
pub struct ReleaseNotesView<'a> {
    /// User's count input (phase 1: count-entry).
    pub count_input: &'a str,
    /// True while the AI request is in flight (phase 2: generating).
    pub generating: bool,
    /// Completed release notes text (phase 3: displaying).
    pub notes: &'a str,
    /// Vertical scroll offset for the notes display.
    pub scroll: u16,
}

/// Data for the diagnostics overlay (Mode::Diagnostics).
pub struct DiagnosticsData<'a> {
    /// MCP: (server_name, tool_count) for each connected server.
    pub mcp_connected: Vec<(&'a str, usize)>,
    /// MCP: (server_name, error) for each failed server.
    pub mcp_failed: &'a [(String, String)],
    /// LSP server names that are active.
    pub lsp_servers: Vec<&'a str>,
    /// Path to the log file.
    pub log_path: &'a str,
    /// Recent log entries (level, message) newest-last.
    pub recent_logs: &'a [(String, String)],
}

/// Data for the file-info popup shown when `i` is pressed in the explorer.
pub struct FileInfoData {
    /// Full absolute path of the selected entry.
    pub path: std::path::PathBuf,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// File size in bytes (None for directories).
    pub size_bytes: Option<u64>,
    /// Last-modified time.
    pub modified: Option<std::time::SystemTime>,
    /// Creation time (not available on all platforms/filesystems).
    pub created: Option<std::time::SystemTime>,
    /// Unix permission string e.g. "rwxr-xr-x". None on non-Unix platforms.
    pub permissions: Option<String>,
}

// Buffer data tuple: (name, is_modified, cursor, scroll_row, scroll_col, lines, selection)
type BufferData = (String, bool, Cursor, usize, usize, Vec<String>, Option<Selection>);
// Buffer list tuple: (buffer names with modified flags, selected index)
type BufferList = (Vec<(String, bool)>, usize);
// File list tuple: (fuzzy-filtered entries with match indices, selected index, query)
type FileList = (Vec<(PathBuf, Vec<usize>)>, usize, String);

/// UI rendering for the editor
pub struct UI;

impl UI {
    /// Render the entire UI
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        frame: &mut Frame,
        buffer_data: Option<&BufferData>,
        mode: Mode,
        status_message: Option<&str>,
        command_buffer: Option<&str>,
        which_key_options: Option<&[(String, String)]>,
        key_sequence: &str,
        buffer_list: Option<&BufferList>,
        file_list: Option<&FileList>,
        diagnostics: &[Diagnostic],
        // Ghost text suggestion: (text, buffer_row, buffer_col)
        ghost_text: Option<(&str, usize, usize)>,
        // Agent panel (None = hidden)
        agent_panel: Option<&AgentPanel>,
        // Pre-computed syntax-highlighted spans for the visible viewport.
        // Each element is the span list for one visible line (index 0 = scroll_row).
        highlighted_lines: Option<&[Vec<Span<'static>>]>,
        // File explorer panel (None = hidden)
        file_explorer: Option<&FileExplorer>,
        // Pre-rendered markdown lines (Mode::MarkdownPreview only).
        // When Some, these are displayed in place of normal buffer content.
        preview_lines: Option<&[Line<'static>]>,
        // Project-wide search overlay (Mode::Search only).
        search_state: Option<&SearchState>,
        // Rename popup filename buffer (Mode::RenameFile only).
        rename_buffer: Option<&str>,
        // Delete confirmation name (Mode::DeleteFile only).
        delete_name: Option<&str>,
        // New folder name buffer (Mode::NewFolder only).
        new_folder_buffer: Option<&str>,
        // Apply-diff overlay data (Mode::ApplyDiff only).
        apply_diff: Option<&ApplyDiffView<'_>>,
        // Vertical split: inactive pane data (None = no split).
        split_buffer_data: Option<&BufferData>,
        // Pre-computed highlighted spans for the split (inactive) pane.
        split_highlighted_lines: Option<&[Vec<Span<'static>>]>,
        // True when the right pane is the focused pane.
        split_right_focused: bool,
        // Commit message buffer (Mode::CommitMsg only).
        commit_msg: Option<&str>,
        // Release notes popup data (Mode::ReleaseNotes only).
        release_notes: Option<&ReleaseNotesView<'_>>,
        // Diagnostics overlay data (Mode::Diagnostics only).
        diag_overlay: Option<&DiagnosticsData<'_>>,
        // Time from process start to the editor being ready; displayed on the welcome screen.
        startup_elapsed: Option<std::time::Duration>,
        // File-info popup data (explorer `i` key). None = hidden.
        file_info: Option<&FileInfoData>,
    ) {
        let size = frame.area();

        if mode == Mode::ApplyDiff {
            if let Some(view) = apply_diff {
                Self::render_apply_diff_overlay(frame, view, size);
            }
            return;
        }

        // If in PickBuffer mode, show buffer picker
        if mode == Mode::PickBuffer {
            Self::render_buffer_picker(frame, buffer_list, size);
            return;
        }

        // If in PickFile mode, show file picker
        if mode == Mode::PickFile {
            Self::render_file_picker(frame, file_list, size);
            return;
        }

        // If in Search mode, show the ripgrep search overlay
        if mode == Mode::Search {
            if let Some(state) = search_state {
                Self::render_search_panel(frame, state, size);
            }
            return;
        }

        // ── Horizontal splits: [explorer/tasks?] | [editor] | [agent?] ─────────────
        let explorer_visible = file_explorer.map(|e| e.visible).unwrap_or(false);
        let agent_visible = agent_panel.map(|p| p.visible).unwrap_or(false);
        let left_sidebar_visible = explorer_visible;

        let (left_sidebar_area, content_area, agent_area) =
            match (left_sidebar_visible, agent_visible) {
                (true, true) => {
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Length(25),
                            Constraint::Min(1),
                            Constraint::Percentage(35),
                        ])
                        .split(size);
                    (Some(cols[0]), cols[1], Some(cols[2]))
                },
                (true, false) => {
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Length(25), Constraint::Min(1)])
                        .split(size);
                    (Some(cols[0]), cols[1], None)
                },
                (false, true) => {
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                        .split(size);
                    (None, cols[0], Some(cols[1]))
                },
                (false, false) => (None, size, None),
            };
        let editor_area = content_area;

        // ── Vertical layout (buffer + status) inside editor_area ─────────────
        let constraints = if which_key_options.is_some() {
            vec![
                Constraint::Min(1),     // Main buffer area
                Constraint::Length(10), // Which-key popup
                Constraint::Length(1),  // Status line
            ]
        } else {
            vec![
                Constraint::Min(1),    // Main buffer area
                Constraint::Length(1), // Status line
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(editor_area);

        let main_area = chunks[0];
        let status_area = if let Some(wk) = which_key_options {
            let which_key_area = chunks[1];
            Self::render_which_key(frame, wk, which_key_area);
            chunks[2]
        } else {
            chunks[1]
        };

        // Render buffer content — single pane or vertical split
        if let Some(split_data) = split_buffer_data {
            // 3-column layout: [left pane | 1-char separator | right pane]
            let split_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(50),
                    Constraint::Length(1),
                    Constraint::Percentage(50),
                ])
                .split(main_area);

            // Determine which buffer goes in which pane.
            // current buffer (buffer_data) is always the focused pane.
            let (left_data, right_data): (Option<&BufferData>, Option<&BufferData>) =
                if split_right_focused {
                    (Some(split_data), buffer_data)
                } else {
                    (buffer_data, Some(split_data))
                };
            let (left_hl, right_hl) = if split_right_focused {
                (split_highlighted_lines, highlighted_lines)
            } else {
                (highlighted_lines, split_highlighted_lines)
            };
            let (left_ghost, right_ghost) =
                if split_right_focused { (None, ghost_text) } else { (ghost_text, None) };
            let left_preview = if split_right_focused { None } else { preview_lines };

            Self::render_buffer(
                frame,
                left_data,
                mode,
                split_chunks[0],
                diagnostics,
                left_ghost,
                left_hl,
                left_preview,
                !split_right_focused,
                startup_elapsed,
            );

            // Draw vertical separator
            let sep_lines: Vec<Line> = (0..split_chunks[1].height)
                .map(|_| Line::from(Span::styled("│", Style::default().fg(Color::DarkGray))))
                .collect();
            frame.render_widget(Paragraph::new(sep_lines), split_chunks[1]);

            Self::render_buffer(
                frame,
                right_data,
                mode,
                split_chunks[2],
                diagnostics,
                right_ghost,
                right_hl,
                None,
                split_right_focused,
                startup_elapsed,
            );
        } else {
            Self::render_buffer(
                frame,
                buffer_data,
                mode,
                main_area,
                diagnostics,
                ghost_text,
                highlighted_lines,
                preview_lines,
                true,
                startup_elapsed,
            );
        }

        // Render status line
        Self::render_status_line(
            frame,
            buffer_data,
            mode,
            status_message,
            command_buffer,
            key_sequence,
            status_area,
            diagnostics,
        );

        // Render agent panel if visible
        if let (Some(panel), Some(area)) = (agent_panel, agent_area) {
            Self::render_agent_panel(frame, panel, mode, area);
        }

        // Render left sidebar (explorer only now)
        if let Some(area) = left_sidebar_area {
            if let Some(explorer) = file_explorer {
                if explorer.visible {
                    Self::render_file_explorer(frame, explorer, mode, area);
                }
            }
        }

        // Render rename popup if active
        if let Some(name) = rename_buffer {
            Self::render_rename_popup(frame, name, size);
        }

        // Render delete confirmation popup if active
        if let Some(name) = delete_name {
            Self::render_delete_popup(frame, name, size);
        }

        // Render new folder popup if active
        if let Some(name) = new_folder_buffer {
            Self::render_new_folder_popup(frame, name, size);
        }

        // Render commit message popup if active
        if let Some(msg) = commit_msg {
            Self::render_commit_msg_popup(frame, msg, size);
        }

        // Render release notes popup if active
        if let Some(view) = release_notes {
            Self::render_release_notes_popup(frame, view, size);
        }

        // Render diagnostics overlay if active
        if let Some(diag) = diag_overlay {
            Self::render_diagnostics_overlay(frame, diag, size);
        }

        // Render file-info popup if active (explorer `i` key)
        if let Some(info) = file_info {
            let explorer_right_edge = if explorer_visible { 25u16 } else { 0 };
            Self::render_file_info_popup(frame, info, size, explorer_right_edge);
        }
    }

    /// Render the Copilot Chat / agent panel on the right side.
    fn render_agent_panel(frame: &mut Frame, panel: &AgentPanel, mode: Mode, area: Rect) {
        let focused = mode == Mode::Agent;
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Compute input box height: expand as the user types, up to 10 text lines.
        // content_width = panel width minus 2 border columns.
        // We calculate how many display rows the current input occupies, accounting for
        // both explicit newlines (\n) and word-wrap within each logical line.
        let content_width = area.width.saturating_sub(2) as usize;
        let explicit_lines: Vec<&str> = panel.input.split('\n').collect();
        let total_wrapped: usize = explicit_lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                // Add 1 to the last line for the trailing cursor character.
                let len = line.chars().count() + if i == explicit_lines.len() - 1 { 1 } else { 0 };
                if content_width > 0 {
                    len.div_ceil(content_width).max(1)
                } else {
                    1
                }
            })
            .sum();
        // At least 1 text line; at most 10 text lines to keep history visible.
        let input_text_lines = total_wrapped.clamp(1, 10) as u16;
        // Each pasted block adds one summary line; each file block adds one badge line.
        let paste_summary_lines = panel.pasted_blocks.len() as u16;
        let file_summary_lines = panel.file_blocks.len() as u16;
        let input_height = input_text_lines + paste_summary_lines + file_summary_lines + 2; // +2 borders

        // Task strip height: 0 when empty, otherwise tasks + 2 border rows (capped at 8).
        let task_strip_height =
            if panel.tasks.is_empty() { 0 } else { (panel.tasks.len() as u16 + 2).min(8) };

        // Split area vertically: history (top) + [task strip] + input (dynamic bottom).
        let vchunks = if task_strip_height > 0 {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(task_strip_height),
                    Constraint::Length(input_height),
                ])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(input_height)])
                .split(area)
        };

        let history_area = vchunks[0];
        let (task_area, input_area) =
            if task_strip_height > 0 { (Some(vchunks[1]), vchunks[2]) } else { (None, vchunks[1]) };

        // ── Chat history (cache-aware) ────────────────────────────────────────
        // render_message_content() runs the markdown parser + split_thinking()
        // which is expensive.  We cache the rendered Line<'static> vectors and
        // only recompute when content or width actually changes.
        let content_width = history_area.width.saturating_sub(4) as usize;
        let inner_width = history_area.width.saturating_sub(2) as usize;
        let visible_height = history_area.height.saturating_sub(2) as usize;

        let cur_msg_count = panel.messages.len();
        let cur_streaming_len = panel.streaming_reply.as_ref().map(|s| s.len()).unwrap_or(0);

        let (lines, total_display_rows) = PANEL_CACHE.with(|cell| {
            let mut cache = cell.borrow_mut();

            // — Completed messages —
            if cache.msg_count != cur_msg_count || cache.content_width != content_width {
                let mut ml: Vec<Line<'static>> = Vec::new();
                for msg in &panel.messages {
                    if matches!(msg.role, Role::System) {
                        ml.push(Line::from(vec![Span::styled(
                            format!("  {}  ", msg.content),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                        )]));
                        ml.push(Line::from(""));
                        continue;
                    }
                    let (label, color) = match msg.role {
                        Role::User => ("You", Color::Green),
                        Role::Assistant => ("Copilot", Color::Cyan),
                        Role::System => unreachable!(),
                    };
                    ml.push(Line::from(vec![Span::styled(
                        format!("╔ {label} "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )]));
                    ml.extend(render_message_content(&msg.content, content_width));
                    ml.push(Line::from(""));
                }
                cache.msg_lines = ml;
                cache.msg_count = cur_msg_count;
                cache.content_width = content_width;
                cache.msg_row_count = wrapped_line_count(&cache.msg_lines, inner_width);
            }

            // — Streaming reply —
            if cache.streaming_len != cur_streaming_len || cache.streaming_width != content_width {
                if let Some(ref partial) = panel.streaming_reply {
                    let mut sl: Vec<Line<'static>> = vec![Line::from(vec![
                        Span::styled(
                            "╔ Copilot ",
                            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "▋",
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::SLOW_BLINK),
                        ),
                    ])];
                    sl.extend(render_message_content(partial, content_width));
                    cache.streaming_lines = sl;
                } else {
                    cache.streaming_lines.clear();
                }
                cache.streaming_len = cur_streaming_len;
                cache.streaming_width = content_width;
                cache.streaming_row_count = wrapped_line_count(&cache.streaming_lines, inner_width);
            }

            // Build the combined line Vec for ratatui.
            // Cloning pre-built Line<'static> objects is far cheaper than
            // re-running the markdown parser on every message every frame.
            let mut lines = cache.msg_lines.clone();
            lines.extend(cache.streaming_lines.iter().cloned());
            lines.push(Line::from(""));
            lines.push(Line::from(""));

            // Total display rows = independently cached per-sub-Vec counts + 2
            // buffer lines.  Because each Line's row count depends only on its
            // own content and inner_width (not on surrounding lines), the counts
            // are additive and do not need the combined Vec — this avoids an
            // extra full clone on every streaming frame.
            let total_display_rows = (cache.msg_row_count + cache.streaming_row_count + 2).max(1);

            (lines, total_display_rows)
        });

        let max_scroll = total_display_rows.saturating_sub(visible_height);
        let scroll = panel.scroll.min(max_scroll);
        // row_offset for Paragraph::scroll: 0 = top of content; max_scroll = show bottom.
        let row_offset = max_scroll.saturating_sub(scroll) as u16;

        // Build a title that shows the active model, live status, and scroll position.
        let model_label = panel.selected_model_display();
        let status_suffix =
            panel.status.label(panel.max_rounds).map(|s| format!("  ● {s}")).unwrap_or_default();
        let scroll_suffix: std::borrow::Cow<'static, str> = if scroll > 0 {
            let pct = if max_scroll > 0 { 100 - (scroll * 100 / max_scroll).min(100) } else { 100 };
            format!("  ↑ scrolled ({pct}%)  ↑/↓ to navigate ").into()
        } else if total_display_rows > visible_height {
            "  (↑ to scroll up) ".into()
        } else {
            " ".into()
        };

        let token_span = if panel.last_prompt_tokens > 0 {
            let window = panel.context_window_size();
            let pct = panel.last_prompt_tokens * 100 / window;
            let color = if pct >= 80 {
                Color::Red
            } else if pct >= 50 {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            let k_used = panel.last_prompt_tokens as f32 / 1000.0;
            let k_total = window as f32 / 1000.0;
            Span::styled(format!("  {k_used:.1}k/{k_total:.0}k"), Style::default().fg(color))
        } else {
            Span::raw("")
        };

        let title_line = Line::from(vec![
            Span::raw(format!(" Copilot Chat [{model_label}]")),
            token_span,
            Span::raw(format!("{status_suffix}{scroll_suffix}")),
        ]);

        // MCP status bottom-bar — rebuilt only when manager presence or failed-
        // server count changes (both are stable after startup), then cloned from
        // the cache every frame instead of rebuilding with format!/join/collect.
        let mcp_bottom = PANEL_CACHE.with(|cell| {
            let mut cache = cell.borrow_mut();
            let mcp_key = (
                panel.mcp_manager.is_some() as usize,
                panel.mcp_manager.as_ref().map_or(0, |m| m.failed_servers.len()),
            );
            if cache.mcp_status_key != mcp_key {
                let line = match &panel.mcp_manager {
                    None => Line::from(Span::styled(
                        " MCP: none ",
                        Style::default().fg(Color::DarkGray),
                    )),
                    Some(mcp) => {
                        let mut spans = vec![Span::raw(" MCP: ")];
                        let connected: Vec<String> = mcp
                            .connected_servers()
                            .into_iter()
                            .map(|(name, count)| format!("{} ({})", name, count))
                            .collect();
                        if !connected.is_empty() {
                            spans.push(Span::styled(
                                connected.join(", "),
                                Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
                            ));
                        } else {
                            spans.push(Span::styled(
                                "no tools",
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        for (name, reason) in &mcp.failed_servers {
                            spans.push(Span::styled(
                                format!("  ⚠ {}: {}", name, reason),
                                Style::default().fg(Color::Red),
                            ));
                        }
                        spans.push(Span::raw(" "));
                        Line::from(spans)
                    },
                };
                cache.mcp_bottom = Some(line);
                cache.mcp_status_key = mcp_key;
            }
            cache.mcp_bottom.clone().unwrap_or_default()
        });

        let history_block = Block::default()
            .title(title_line)
            .title_bottom(mcp_bottom)
            .borders(Borders::ALL)
            .border_style(border_style);
        let history_para = Paragraph::new(lines)
            .block(history_block)
            .wrap(Wrap { trim: false })
            .scroll((row_offset, 0));
        frame.render_widget(history_para, history_area);

        // ── Task strip ────────────────────────────────────────────────────────
        if let Some(area) = task_area {
            Self::render_task_strip(frame, &panel.tasks, border_style, area);
        }

        // ── Input box ─────────────────────────────────────────────────────────
        // Show [a] apply hint when the latest reply contains a code block.
        let hint = if panel.messages.is_empty() {
            " Ask Copilot… (Enter=send, Alt+Enter=newline, Ctrl+P=attach file, Ctrl+T=model)"
                .to_string()
        } else if panel.has_code_to_apply()
            && panel.input.is_empty()
            && panel.pasted_blocks.is_empty()
            && panel.file_blocks.is_empty()
        {
            " Message Copilot… | [a] diff+apply  Ctrl+P=attach  Ctrl+T=model ".to_string()
        } else {
            " Message Copilot… (Ctrl+T=model) ".to_string()
        };
        let hint = hint.as_str();
        let input_block =
            Block::default().title(hint).borders(Borders::ALL).border_style(border_style);

        // Build input content: file block badges first (green), then pasted block
        // badges (cyan), then the typed text.
        let file_style = Style::default().fg(Color::LightGreen).add_modifier(Modifier::DIM);
        let mut input_lines: Vec<Line> = panel
            .file_blocks
            .iter()
            .map(|(name, _, line_count)| {
                let label = format!(
                    "  {} ({} line{})",
                    name,
                    line_count,
                    if *line_count == 1 { "" } else { "s" }
                );
                Line::from(Span::styled(label, file_style))
            })
            .collect();
        let paste_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM);
        input_lines.extend(panel.pasted_blocks.iter().map(|(_, n)| {
            let label = format!("⎘  Pasted {} line{}", n, if *n == 1 { "" } else { "s" });
            Line::from(Span::styled(label, paste_style))
        }));
        let typed = if focused { format!("{}_", panel.input) } else { panel.input.clone() };
        for line in typed.split('\n') {
            input_lines.push(Line::from(line.to_string()));
        }
        let input_para = Paragraph::new(input_lines)
            .block(input_block)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false });
        frame.render_widget(input_para, input_area);

        // Ctrl+P file-context picker — rendered just above the input box.
        if let Some(ref picker) = panel.at_picker {
            Self::render_at_picker(frame, picker, &panel.file_blocks, input_area);
        }

        // Slash-command autocomplete dropdown — rendered just above the input box.
        if let Some(ref menu) = panel.slash_menu {
            Self::render_slash_menu(frame, menu, input_area);
        }

        // Awaiting-continuation dialog — shown whenever the agent hits max rounds.
        // Rendered as a prominent overlay so it can't be missed after a long plan.
        if panel.awaiting_continuation {
            Self::render_continuation_dialog(frame, panel.current_round, panel.max_rounds, area);
        }

        // If the agent is waiting for a question answer, render the dialog on top.
        // Constrain to the agent panel area so it never overlaps the explorer.
        if let Some(ref state) = panel.asking_user {
            Self::render_ask_user_dialog(frame, state, area);
        }
    }

    /// Render the slash-command autocomplete dropdown just above the input box.
    fn render_slash_menu(frame: &mut Frame, menu: &SlashMenuState, input_area: Rect) {
        if menu.items.is_empty() {
            return;
        }

        let n = menu.items.len() as u16;
        // Width matches the input box; height = items + 2 borders, capped at 10 items.
        let popup_width = input_area.width;
        let popup_height = (n + 2).min(12);

        // Position directly above the input box.
        let x = input_area.x;
        let y = input_area.y.saturating_sub(popup_height);
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(" commands ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let lines: Vec<Line> = menu
            .items
            .iter()
            .enumerate()
            .map(|(i, cmd)| {
                if i == menu.selected {
                    Line::from(Span::styled(
                        format!(" /{cmd}"),
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ))
                } else {
                    Line::from(Span::styled(format!(" /{cmd}"), Style::default().fg(Color::White)))
                }
            })
            .collect();

        // Scroll to keep selected item visible.
        let visible_rows = inner.height as usize;
        let scroll = if menu.selected >= visible_rows {
            (menu.selected + 1).saturating_sub(visible_rows) as u16
        } else {
            0
        };

        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
    }

    /// Render the Ctrl+P file-context picker overlay above the agent input box.
    ///
    /// `file_blocks` is the list of currently attached files so the picker can
    /// show a ✓ indicator and let the user toggle files off as well as on.
    fn render_at_picker(
        frame: &mut Frame,
        picker: &AtPickerState,
        file_blocks: &[(String, String, usize)],
        input_area: Rect,
    ) {
        if input_area.y == 0 {
            return; // No vertical space above the input box.
        }

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let n_results = picker.results.len().min(15) as u16;

        // Height: 1 query line + results + 1 hint line + 2 borders.
        // Cannot exceed the space available above the input box.
        let popup_height = (1_u16 + n_results + 1 + 2).min(input_area.y);
        if popup_height < 3 {
            return;
        }

        let popup_width = input_area.width;
        let x = input_area.x;
        let y = input_area.y.saturating_sub(popup_height);
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let n_attached = file_blocks.len();
        let attached_label =
            if n_attached > 0 { format!(" {n_attached} attached ·") } else { String::new() };
        let block = Block::default()
            .title(format!(
                " Attach file ·{attached_label} ↑/↓ navigate · Enter=toggle · Esc=done "
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightGreen));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        if inner.height == 0 {
            return;
        }

        // Split: query line (1 row) + rest for results.
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(inner);
        let query_area = layout[0];
        let results_area = layout[1];

        // Query line with cursor underscore.
        let query_display = format!("> {}_", picker.query);
        frame.render_widget(
            Paragraph::new(Span::styled(query_display, Style::default().fg(Color::White))),
            query_area,
        );

        if results_area.height == 0 {
            return;
        }

        // Results — fuzzy highlighted with attachment indicator prefix.
        // Prefix layout (3 chars): [✓/ ][►/ ][ ]
        //   col 0: ✓ (LightGreen) if attached, space otherwise
        //   col 1: ► (White) if row is selected, space otherwise
        //   col 2: space separator
        let mut lines: Vec<Line> = picker
            .results
            .iter()
            .enumerate()
            .take(15)
            .map(|(i, (path, match_indices))| {
                let display = path.strip_prefix(&cwd).unwrap_or(path).to_string_lossy().to_string();
                let is_selected = i == picker.selected;
                let is_attached = file_blocks.iter().any(|(name, _, _)| name == &display);
                let bg = if is_selected { Color::Rgb(40, 60, 90) } else { Color::Reset };

                let attach_style = Style::default().bg(bg).fg(if is_attached {
                    Color::LightGreen
                } else {
                    Color::Reset
                });
                let cursor_style = Style::default().bg(bg).fg(Color::White);

                let mut spans = vec![
                    Span::styled(if is_attached { "✓" } else { " " }, attach_style),
                    Span::styled(if is_selected { "► " } else { "  " }, cursor_style),
                ];

                // Build multi-span filename: group consecutive chars that share the same
                // match/non-match style.  binary_search() is O(log N) vs O(N) contains();
                // match_indices is sorted because fuzzy_score() scans left-to-right.
                let chars: Vec<char> = display.chars().collect();
                let mut seg = String::new();
                let mut seg_is_match: Option<bool> = None;
                for (ci, &ch) in chars.iter().enumerate() {
                    let is_match = match_indices.binary_search(&ci).is_ok();
                    if seg_is_match == Some(!is_match) && !seg.is_empty() {
                        let style = if seg_is_match == Some(true) {
                            Style::default().bg(bg).fg(Color::Yellow).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().bg(bg).fg(Color::White)
                        };
                        spans.push(Span::styled(std::mem::take(&mut seg), style));
                    }
                    seg.push(ch);
                    seg_is_match = Some(is_match);
                }
                if !seg.is_empty() {
                    let style = if seg_is_match == Some(true) {
                        Style::default().bg(bg).fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().bg(bg).fg(Color::White)
                    };
                    spans.push(Span::styled(seg, style));
                }
                Line::from(spans)
            })
            .collect();

        // Footer hint.
        lines.push(Line::from(Span::styled(
            "  type to filter  ·  ✓ = already attached",
            Style::default().fg(Color::DarkGray),
        )));

        let scroll = (picker.selected as u16).saturating_sub(results_area.height.saturating_sub(2));
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), results_area);
    }

    /// Render the awaiting-continuation dialog at the bottom of the agent panel.
    fn render_continuation_dialog(
        frame: &mut Frame,
        current_round: usize,
        max_rounds: usize,
        area: Rect,
    ) {
        let dialog_width = ((area.width * 92) / 100).max(20);
        // Height: 2 borders + 1 message + 1 blank + 1 hint.
        let dialog_height = 5u16.min(area.height.saturating_sub(2));

        let x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        let y = area.y + area.height.saturating_sub(dialog_height);
        let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .title(format!(" ⏸  Paused — round {current_round}/{max_rounds} "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        let lines = vec![
            Line::from(Span::styled(
                "Maximum rounds reached. Continue the plan?",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "y = continue   n = stop",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Render the ask_user dialog anchored to the bottom of the agent panel.
    /// Constrained to the panel area so it never overlaps the file explorer.
    /// Width is kept moderate so the dialog is taller rather than wider.
    fn render_ask_user_dialog(frame: &mut Frame, state: &AskUserState, area: Rect) {
        let n_opts = state.options.len() as u16;
        // Use 92% of the panel width — moderate enough to be taller than wide.
        let dialog_width = ((area.width * 92) / 100).max(20);
        // Inner width (subtract 2 for borders) used for word-wrap line count.
        let inner_w = dialog_width.saturating_sub(2) as usize;
        // Estimate wrapped line count, adding 50% headroom for word-break overheads.
        let q_chars = state.question.chars().count();
        let q_lines_est = if inner_w == 0 { 1 } else { q_chars.div_ceil(inner_w).max(1) };
        let q_lines = ((q_lines_est * 3) / 2).max(1) as u16;
        // 2 borders + q_lines + 1 blank + options + 1 blank + 1 hint.
        let dialog_height = (2 + q_lines + 1 + n_opts + 1 + 1).min(area.height.saturating_sub(2));

        let x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
        // Pin to the bottom of the panel so output above stays visible.
        let y = area.y + area.height.saturating_sub(dialog_height);
        let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

        frame.render_widget(Clear, dialog_area);

        let block = Block::default()
            .title(" ❓ Question ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

        let inner = block.inner(dialog_area);
        frame.render_widget(block, dialog_area);

        // Render the question text word-wrapped in its own paragraph.
        let q_para = Paragraph::new(Span::styled(
            state.question.clone(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ))
        .wrap(Wrap { trim: false });

        let q_area = Rect::new(inner.x, inner.y, inner.width, q_lines.min(inner.height));
        frame.render_widget(q_para, q_area);

        // Render options + hint below the question.
        let opts_y = inner.y + q_lines + 1; // +1 for blank line
        if opts_y < inner.y + inner.height {
            let mut opt_lines: Vec<Line> = Vec::new();
            for (i, option) in state.options.iter().enumerate() {
                let (prefix, style) = if i == state.selected {
                    ("▶ ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                } else {
                    ("  ", Style::default().fg(Color::White))
                };
                opt_lines.push(Line::from(Span::styled(format!("{prefix}{option}"), style)));
            }
            opt_lines.push(Line::from(""));
            opt_lines.push(Line::from(Span::styled(
                "↑/↓ or j/k = move   Enter = confirm   Esc = cancel",
                Style::default().fg(Color::DarkGray),
            )));

            let remaining = inner.height.saturating_sub(q_lines + 1);
            let opts_area = Rect::new(inner.x, opts_y, inner.width, remaining);
            frame.render_widget(Paragraph::new(opt_lines), opts_area);
        }
    }

    /// Render the file explorer tree on the left side.
    fn render_file_explorer(frame: &mut Frame, explorer: &FileExplorer, mode: Mode, area: Rect) {
        let focused = mode == Mode::Explorer;
        let border_style = if focused {
            Style::default().fg(Color::LightGreen)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let flat = explorer.flat_visible();
        let visible_height = area.height.saturating_sub(2) as usize; // account for border

        // Scroll so the cursor is always visible.
        let cursor = explorer.cursor_idx;
        let scroll = if cursor >= visible_height { cursor - visible_height + 1 } else { 0 };

        let mut lines: Vec<Line> = Vec::new();
        for (i, node) in flat.iter().enumerate().skip(scroll).take(visible_height) {
            let is_selected = i == cursor;

            let indent = "  ".repeat(node.depth);
            let icon = if node.is_dir {
                if node.is_expanded {
                    "▼ "
                } else {
                    "▶ "
                }
            } else {
                "  "
            };
            let label = format!("{}{}{}", indent, icon, node.name);

            let style = if is_selected {
                Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
            } else if node.is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };

            lines.push(Line::from(Span::styled(label, style)));
        }

        // Fill remaining rows with blanks so the block looks solid
        while lines.len() < visible_height {
            lines.push(Line::from(""));
        }

        let root_name = explorer
            .root_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let block = Block::default()
            .title(format!(" {} ", root_name))
            .borders(Borders::ALL)
            .border_style(border_style);

        let para = Paragraph::new(lines).block(block);
        frame.render_widget(para, area);
    }

    /// Render the inline task progress strip inside the agent panel.
    fn render_task_strip(frame: &mut Frame, tasks: &[AgentTask], border_style: Style, area: Rect) {
        let done = tasks.iter().filter(|t| t.done).count();
        let total = tasks.len();
        // Find the first incomplete task — shown in yellow as "current".
        let current_idx = tasks.iter().position(|t| !t.done);

        let lines: Vec<Line> = tasks
            .iter()
            .enumerate()
            .map(|(i, task)| {
                let (icon, style) = if task.done {
                    ("✓", Style::default().fg(Color::DarkGray))
                } else if Some(i) == current_idx {
                    ("⊙", Style::default().fg(Color::Yellow))
                } else {
                    ("○", Style::default().fg(Color::White))
                };
                Line::from(vec![
                    Span::styled(format!("  {} ", icon), style),
                    Span::styled(task.title.clone(), style),
                ])
            })
            .collect();

        let title = format!(" Plan ({}/{}) ", done, total);
        let block = Block::default().title(title).borders(Borders::ALL).border_style(border_style);

        let para = Paragraph::new(lines).block(block);
        frame.render_widget(para, area);
    }

    /// Render the buffer content
    #[allow(clippy::too_many_arguments)]
    fn render_buffer(
        frame: &mut Frame,
        buffer_data: Option<&BufferData>,
        mode: Mode,
        area: Rect,
        diagnostics: &[Diagnostic],
        ghost_text: Option<(&str, usize, usize)>,
        highlighted_lines: Option<&[Vec<Span<'static>>]>,
        preview_lines: Option<&[Line<'static>]>,
        show_cursor: bool,
        startup_elapsed: Option<std::time::Duration>,
    ) {
        // ── Markdown preview mode — render pre-computed lines directly ─────────
        if let Some(md_lines) = preview_lines {
            let viewport_height = area.height as usize;
            // Slice to viewport height; pad with blank lines below.
            let mut visible: Vec<Line> = md_lines.iter().take(viewport_height).cloned().collect();
            while visible.len() < viewport_height {
                visible.push(Line::from(Span::styled("~", Style::default().fg(Color::DarkGray))));
            }
            let paragraph = Paragraph::new(visible);
            frame.render_widget(paragraph, area);
            // No cursor in preview mode.
            return;
        }

        if let Some((_, _, cursor, scroll_row, scroll_col, lines, selection)) = buffer_data {
            let viewport_height = area.height as usize;
            let viewport_width = area.width as usize;

            // `lines` is a viewport-clipped slice: element 0 corresponds to `scroll_row`.
            // Only as many entries as are visible were cloned — see editor/mod.rs buffer_data
            // builder.  Use relative indexing (row - start_line) to address into the slice;
            // `row` itself stays absolute so diagnostic/selection comparisons stay correct.
            let start_line = *scroll_row;
            let end_line = start_line + lines.len().min(viewport_height);

            // Build visible lines
            let mut visible_lines = Vec::new();
            for row in start_line..end_line {
                if let Some(line_text) = lines.get(row - start_line) {
                    // Check if this line has any diagnostics
                    let has_diagnostic =
                        diagnostics.iter().any(|d| d.range.start.line as usize == row);
                    // Only inject ghost text on the row/col it was requested for.
                    let row_ghost = ghost_text.and_then(|(text, ghost_row, ghost_col)| {
                        if row == ghost_row && cursor.col == ghost_col {
                            Some(text.lines().next().unwrap_or(text))
                        } else {
                            None
                        }
                    });
                    // Use pre-highlighted spans when available, fall back to plain text.
                    let line_idx = row - start_line;
                    let line = if let Some(spans) = highlighted_lines.and_then(|h| h.get(line_idx))
                    {
                        Self::render_highlighted_line(
                            spans,
                            *scroll_col,
                            viewport_width,
                            has_diagnostic,
                            row_ghost,
                            selection,
                            row,
                        )
                    } else {
                        Self::render_line(
                            line_text,
                            *scroll_col,
                            viewport_width,
                            row,
                            selection,
                            *scroll_row,
                            has_diagnostic,
                            row_ghost,
                        )
                    };
                    visible_lines.push(line);
                } else {
                    visible_lines.push(Line::from("~"));
                }
            }

            // Fill remaining lines with ~
            for _ in visible_lines.len()..viewport_height {
                visible_lines
                    .push(Line::from(Span::styled("~", Style::default().fg(Color::DarkGray))));
            }

            let paragraph = Paragraph::new(visible_lines);
            frame.render_widget(paragraph, area);

            // Render cursor (only in Normal, Insert modes, and only for the focused pane).
            // GUTTER_WIDTH accounts for the 2-char diagnostic marker ("  " / "● ")
            // prepended to every rendered line — the cursor must be offset by the same amount.
            const GUTTER_WIDTH: u16 = 2;
            if mode != Mode::PickBuffer && show_cursor {
                let cursor_row = cursor.row.saturating_sub(*scroll_row);
                let cursor_col = cursor.col.saturating_sub(*scroll_col);

                if cursor_row < viewport_height && cursor_col < viewport_width {
                    frame.set_cursor_position((
                        area.x + GUTTER_WIDTH + cursor_col as u16,
                        area.y + cursor_row as u16,
                    ));
                }
            }
        } else {
            // No buffer open — show the welcome screen.
            Self::render_welcome(frame, area, startup_elapsed);
        }
    }

    /// Render the welcome / splash screen shown when no buffer is open.
    fn render_welcome(frame: &mut Frame, area: Rect, startup_elapsed: Option<std::time::Duration>) {
        #[rustfmt::skip]
        const CROSS: &[&str] = &[
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "           ━━━━━━━━━━━━━━━━━━━━╋╋╋━━━━━━━━━━━━━━━━━━━━",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
        ];
        #[rustfmt::skip]
        const WORDMARK: &[&str] = &[
            "███████╗ ██████╗ ██████╗  ██████╗ ██╗██╗   ██╗███████╗███╗   ██╗",
            "██╔════╝██╔═══██╗██╔══██╗██╔════╝ ██║██║   ██║██╔════╝████╗  ██║",
            "█████╗  ██║   ██║██████╔╝██║  ███╗██║██║   ██║█████╗  ██╔██╗ ██║",
            "██╔══╝  ██║   ██║██╔══██╗██║   ██║██║╚██╗ ██╔╝██╔══╝  ██║╚██╗██║",
            "██║     ╚██████╔╝██║  ██║╚██████╔╝██║ ╚████╔╝ ███████╗██║ ╚████║",
            "╚═╝      ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝  ╚══════╝╚═╝  ╚═══╝",
        ];
        const TAGLINE: &str = "an AI-first terminal code editor  ·  MIT License";
        const HINTS: &str = "SPC f f  open file    SPC e e  explorer    SPC a a  agent";
        // Width of the widest logo line (WORDMARK row 1 = 64 display columns).
        const LOGO_W: usize = 64;

        let area_h = area.height as usize;
        let area_w = area.width as usize;

        // Total logo height: cross + blank + wordmark + blank + tagline + blank + hints [+ blank + ready].
        let logo_h = CROSS.len()
            + 1
            + WORDMARK.len()
            + 1
            + 1
            + 1
            + 1
            + if startup_elapsed.is_some() { 2 } else { 0 };
        let top_pad = area_h.saturating_sub(logo_h) / 2;
        let left_pad = area_w.saturating_sub(LOGO_W) / 2;

        let cross_style = Style::default().fg(Color::Yellow);
        let word_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
        let dim_style = Style::default().fg(Color::DarkGray);

        let mut lines: Vec<Line> = (0..top_pad).map(|_| Line::from("")).collect();

        for s in CROSS {
            lines.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(left_pad), *s),
                cross_style,
            )));
        }
        lines.push(Line::from(""));
        for s in WORDMARK {
            lines.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(left_pad), *s),
                word_style,
            )));
        }
        lines.push(Line::from(""));
        let tag_pad = area_w.saturating_sub(TAGLINE.len()) / 2;
        lines.push(Line::from(Span::styled(
            format!("{}{}", " ".repeat(tag_pad), TAGLINE),
            dim_style,
        )));
        lines.push(Line::from(""));
        let hint_pad = area_w.saturating_sub(HINTS.len()) / 2;
        lines.push(Line::from(Span::styled(
            format!("{}{}", " ".repeat(hint_pad), HINTS),
            dim_style,
        )));

        if let Some(elapsed) = startup_elapsed {
            let ms = elapsed.as_millis();
            let ready_text = format!("ready in {ms} ms");
            let ready_pad = area_w.saturating_sub(ready_text.len()) / 2;
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(ready_pad), ready_text),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
            )));
        }

        frame.render_widget(Paragraph::new(lines), area);
    }

    /// Render a pre-highlighted line (from syntect) with gutter marker, optional selection
    /// highlight, and ghost text.  Selection is overlaid on top of syntax colours so both
    /// are visible simultaneously.
    fn render_highlighted_line(
        spans: &[Span<'static>],
        scroll_col: usize,
        viewport_width: usize,
        has_diagnostic: bool,
        ghost: Option<&str>,
        selection: &Option<Selection>,
        row: usize,
    ) -> Line<'static> {
        let diag_marker = if has_diagnostic {
            Span::styled("● ", Style::default().fg(Color::Red))
        } else {
            Span::raw("  ")
        };

        // Available columns for actual text (gutter uses 2).
        let text_width = viewport_width.saturating_sub(2);

        // Pre-compute normalised selection bounds once.
        let sel_range = selection.as_ref().map(|sel| sel.normalized());

        // Determine whether this specific row overlaps the selection at all.
        // If it doesn't, we can use the original efficient span-clipping path
        // (no per-character String allocations).
        let row_in_selection = match &sel_range {
            None => false,
            Some((start, end)) => row >= start.row && row <= end.row,
        };

        let mut out_spans: Vec<Span<'static>> = vec![diag_marker];

        if !row_in_selection {
            // ── Fast path: no selection on this row — clip spans to viewport ──
            // Reuses syntect Span content directly; zero extra String allocations.
            let mut col_budget = text_width;
            let mut skipped = 0usize;

            for span in spans {
                if col_budget == 0 {
                    break;
                }
                let span_chars: Vec<char> = span.content.chars().collect();
                let span_len = span_chars.len();

                if skipped < scroll_col {
                    let skip_here = (scroll_col - skipped).min(span_len);
                    skipped += skip_here;
                    let rest: String = span_chars[skip_here..].iter().collect();
                    if !rest.is_empty() {
                        let take: String = rest.chars().take(col_budget).collect();
                        col_budget = col_budget.saturating_sub(take.chars().count());
                        out_spans.push(Span::styled(take, span.style));
                    }
                } else {
                    let take: String = span_chars.iter().take(col_budget).collect();
                    col_budget = col_budget.saturating_sub(take.chars().count());
                    out_spans.push(Span::styled(take, span.style));
                }
            }
        } else {
            // ── Slow path: row overlaps selection — walk character by character ──
            // Needed so we can override the background colour per character.
            let mut abs_col = 0usize;

            for span in spans {
                if abs_col >= scroll_col + text_width {
                    break;
                }
                for ch in span.content.chars() {
                    if abs_col >= scroll_col + text_width {
                        break;
                    }
                    if abs_col < scroll_col {
                        abs_col += 1;
                        continue;
                    }

                    let col_idx = abs_col;

                    // Is this character inside the visual selection?
                    // Charwise visual is inclusive on both ends (like vim).
                    // Linewise mode sets end.col = usize::MAX so `<= usize::MAX` is always true.
                    let is_selected = match &sel_range {
                        Some((start, end)) => {
                            if start.row == end.row && row == start.row {
                                col_idx >= start.col && col_idx <= end.col
                            } else if row == start.row {
                                col_idx >= start.col
                            } else if row == end.row {
                                col_idx <= end.col
                            } else {
                                true // row > start.row && row < end.row (already checked)
                            }
                        },
                        None => false,
                    };

                    let style = if is_selected {
                        Style::default().bg(Color::DarkGray).fg(Color::White)
                    } else {
                        span.style
                    };

                    out_spans.push(Span::styled(ch.to_string(), style));
                    abs_col += 1;
                }
            }
        }

        if let Some(g) = ghost {
            out_spans.push(Span::styled(g.to_string(), Style::default().fg(Color::DarkGray)));
        }

        Line::from(out_spans)
    }

    /// Render a single line with optional selection highlighting and ghost text.
    #[allow(clippy::too_many_arguments)]
    fn render_line(
        line_text: &str,
        scroll_col: usize,
        viewport_width: usize,
        row: usize,
        selection: &Option<Selection>,
        _scroll_row: usize,
        has_diagnostic: bool,
        // First line of inline completion ghost text, shown dimmed after cursor.
        ghost: Option<&str>,
    ) -> Line<'static> {
        let chars: Vec<char> = line_text.chars().collect();

        // Prepare diagnostic marker if present
        let diag_marker = if has_diagnostic {
            vec![Span::styled("● ", Style::default().fg(Color::Red))]
        } else {
            vec![Span::raw("  ")]
        };

        // If there's a selection, highlight the selected portion
        if let Some(sel) = selection {
            let (start, end) = sel.normalized();

            // Available text columns: viewport_width (= area.width) minus the 2-char gutter.
            let text_width = viewport_width.saturating_sub(2);

            let mut spans = Vec::new();
            for (col_idx, ch) in chars.iter().enumerate() {
                if col_idx < scroll_col {
                    continue;
                }
                if col_idx >= scroll_col + text_width {
                    break;
                }

                // Check if this character is in the selection.
                // Charwise visual is inclusive on both ends (like vim).
                // Linewise mode sets end.col = usize::MAX so `<= usize::MAX` is always true.
                let is_selected = if start.row == end.row && row == start.row {
                    col_idx >= start.col && col_idx <= end.col
                } else if row == start.row {
                    col_idx >= start.col
                } else if row == end.row {
                    col_idx <= end.col
                } else {
                    row > start.row && row < end.row
                };

                let style = if is_selected {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                } else {
                    Style::default()
                };

                spans.push(Span::styled(ch.to_string(), style));
            }

            let mut line_spans = diag_marker;
            line_spans.extend(spans);
            if let Some(g) = ghost {
                line_spans.push(Span::styled(g.to_string(), Style::default().fg(Color::DarkGray)));
            }
            Line::from(line_spans)
        } else {
            // No selection, just render normally
            let visible_text: String = chars
                .iter()
                .skip(scroll_col)
                .take(viewport_width.saturating_sub(2)) // Reserve space for diagnostic marker
                .collect();

            let mut line_spans = diag_marker;
            line_spans.push(Span::raw(visible_text));
            if let Some(g) = ghost {
                line_spans.push(Span::styled(g.to_string(), Style::default().fg(Color::DarkGray)));
            }
            Line::from(line_spans)
        }
    }

    /// Render the which-key popup
    fn render_which_key(frame: &mut Frame, options: &[(String, String)], area: Rect) {
        let mut lines = vec![Line::from(Span::styled(
            "Available keys:",
            Style::default().add_modifier(Modifier::BOLD),
        ))];

        for (key, desc) in options {
            lines.push(Line::from(vec![
                Span::styled(format!("  {}", key), Style::default().fg(Color::Cyan)),
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(Color::Gray)),
            ]));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title("Which Key");

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    }

    /// Render the buffer picker
    fn render_buffer_picker(frame: &mut Frame, buffer_list: Option<&BufferList>, area: Rect) {
        if let Some((buffers, selected_idx)) = buffer_list {
            let mut lines = vec![Line::from(Span::styled(
                "Select Buffer:",
                Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow),
            ))];
            lines.push(Line::from(""));

            for (idx, (name, is_modified)) in buffers.iter().enumerate() {
                let modified_marker = if *is_modified { " [+]" } else { "" };
                let style = if idx == *selected_idx {
                    Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                lines.push(Line::from(Span::styled(
                    format!("  {}{}", name, modified_marker),
                    style,
                )));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "↑/↓ or j/k to navigate, Enter to select, Esc to cancel",
                Style::default().fg(Color::Gray),
            )));

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Buffer List ");

            // Center the picker
            let picker_width = 60.min(area.width);
            let picker_height = (buffers.len() + 6).min(area.height as usize);

            let horizontal = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length((area.width.saturating_sub(picker_width)) / 2),
                    Constraint::Length(picker_width),
                    Constraint::Min(0),
                ])
                .split(area);

            let vertical = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length((area.height.saturating_sub(picker_height as u16)) / 2),
                    Constraint::Length(picker_height as u16),
                    Constraint::Min(0),
                ])
                .split(horizontal[1]);

            let picker_area = vertical[1];

            let paragraph = Paragraph::new(lines).block(block);
            frame.render_widget(paragraph, picker_area);
        }
    }

    /// Render the file picker
    fn render_file_picker(frame: &mut Frame, file_list: Option<&FileList>, area: Rect) {
        let Some((files, selected_idx, query)) = file_list else { return };

        let current_dir = std::env::current_dir().unwrap_or_default();

        // ── Size the popup ──────────────────────────────────────────────────────
        let picker_width = 80.min(area.width);
        // 1 border + 1 query line + 1 divider + up-to-20 results + 1 hint + 1 border
        let result_rows = files.len().min(20) as u16;
        let picker_height = (result_rows + 6).min(area.height);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(area.width.saturating_sub(picker_width) / 2),
                Constraint::Length(picker_width),
                Constraint::Min(0),
            ])
            .split(area);

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(area.height.saturating_sub(picker_height) / 2),
                Constraint::Length(picker_height),
                Constraint::Min(0),
            ])
            .split(horizontal[1]);

        let picker_area = vertical[1];

        // Split the popup vertically: query box (3 rows) | results list
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // query input
                Constraint::Min(1),    // results
            ])
            .split(picker_area);

        // ── Query input box ─────────────────────────────────────────────────────
        let query_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightCyan))
            .title(Span::styled(
                " Find File ",
                Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(
                format!(
                    " {} files ",
                    files.iter().filter(|(p, _)| !p.as_os_str().is_empty()).count()
                ),
                Style::default().fg(Color::DarkGray),
            ));
        let query_display = format!("> {query}_");
        let query_para =
            Paragraph::new(Span::styled(query_display, Style::default().fg(Color::White)))
                .block(query_block);
        frame.render_widget(query_para, inner[0]);

        // ── Results list ────────────────────────────────────────────────────────
        let mut lines: Vec<Line> = Vec::new();

        for (idx, (path, match_indices)) in files.iter().enumerate().take(20) {
            // Sentinels injected by refilter_files() when the query is empty.
            if path.as_os_str().is_empty() {
                // Header: "─── Recent ───"
                lines.push(Line::from(Span::styled(
                    "  ─── Recent ────────────────────────────────────────────────────────",
                    Style::default()
                        .fg(Color::Cyan)
                        .bg(Color::Rgb(20, 35, 50))
                        .add_modifier(Modifier::BOLD),
                )));
                continue;
            }
            if path.to_str() == Some("\x01") {
                // Footer: closing divider after recent files.
                lines.push(Line::from(Span::styled(
                    "  ────────────────────────────────────────────────────────────────────",
                    Style::default().fg(Color::Rgb(30, 80, 110)).bg(Color::Rgb(20, 35, 50)),
                )));
                continue;
            }

            let display: String =
                path.strip_prefix(&current_dir).unwrap_or(path).to_string_lossy().to_string();

            let is_selected = idx == *selected_idx;
            let bg = if is_selected { Color::Rgb(40, 60, 90) } else { Color::Reset };
            let prefix = if is_selected { "► " } else { "  " };

            if match_indices.is_empty() {
                // No highlights (empty query or no match positions)
                let style = if is_selected {
                    Style::default().bg(bg).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(Span::styled(format!("{prefix}{display}"), style)));
            } else {
                // Build multi-span line with matched chars highlighted in yellow
                let mut spans: Vec<Span> = vec![Span::styled(
                    prefix.to_string(),
                    Style::default().bg(bg).fg(if is_selected {
                        Color::White
                    } else {
                        Color::Reset
                    }),
                )];
                // Group consecutive chars with the same match/non-match style.
                // binary_search() replaces the O(N) Vec::contains() calls;
                // match_indices is sorted because fuzzy_score() scans left-to-right.
                let chars: Vec<char> = display.chars().collect();
                let mut seg = String::new();
                let mut seg_is_match: Option<bool> = None;
                for (char_idx, &ch) in chars.iter().enumerate() {
                    let is_match = match_indices.binary_search(&char_idx).is_ok();
                    if seg_is_match == Some(!is_match) && !seg.is_empty() {
                        // Flush the segment with the previous style.
                        let style = if seg_is_match == Some(true) {
                            Style::default().bg(bg).fg(Color::Yellow).add_modifier(Modifier::BOLD)
                        } else if is_selected {
                            Style::default().bg(bg).fg(Color::White).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().bg(bg).fg(Color::White)
                        };
                        spans.push(Span::styled(std::mem::take(&mut seg), style));
                    }
                    seg.push(ch);
                    seg_is_match = Some(is_match);
                }
                // Flush the last segment.
                if !seg.is_empty() {
                    let style = if seg_is_match == Some(true) {
                        Style::default().bg(bg).fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else if is_selected {
                        Style::default().bg(bg).fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().bg(bg).fg(Color::White)
                    };
                    spans.push(Span::styled(seg, style));
                }
                lines.push(Line::from(spans));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  ↑/↓  navigate   Enter  open   Esc  cancel",
            Style::default().fg(Color::DarkGray),
        )));

        let results_block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::LightCyan));
        let results_para = Paragraph::new(lines).block(results_block);
        frame.render_widget(results_para, inner[1]);
    }

    /// Render the project-wide ripgrep search overlay (Mode::Search).
    fn render_search_panel(frame: &mut Frame, state: &SearchState, area: Rect) {
        // ── Centre a popup (≤90 cols wide, 80% screen height) ─────────────────
        let popup_width = 90.min(area.width);
        let popup_height = (area.height * 4 / 5).max(10).min(area.height);
        let h_pad = area.width.saturating_sub(popup_width) / 2;
        let v_pad = area.height.saturating_sub(popup_height) / 2;

        let horiz = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(h_pad),
                Constraint::Length(popup_width),
                Constraint::Min(0),
            ])
            .split(area);

        let vert = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(v_pad),
                Constraint::Length(popup_height),
                Constraint::Min(0),
            ])
            .split(horiz[1]);

        let popup_area = vert[1];

        // ── Three-section vertical layout: query | glob | results ─────────────
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // query input (with ALL borders)
                Constraint::Length(3), // glob filter (LEFT|RIGHT|BOTTOM — shares query bottom)
                Constraint::Min(1),    // results list (LEFT|RIGHT|BOTTOM)
            ])
            .split(popup_area);

        // ── Query input ───────────────────────────────────────────────────────
        let query_focused = state.focus == SearchFocus::Query;
        let query_color = if query_focused { Color::LightRed } else { Color::DarkGray };
        let query_cursor = if query_focused { "_" } else { "" };
        let query_text = format!("> {}{}", state.query, query_cursor);

        let query_block = Block::default()
            .title(Span::styled(
                " Search in Project ",
                Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(query_color));
        let query_para =
            Paragraph::new(Span::styled(query_text, Style::default().fg(Color::White)))
                .block(query_block);
        frame.render_widget(query_para, inner[0]);

        // ── Glob filter input ─────────────────────────────────────────────────
        let glob_focused = state.focus == SearchFocus::Glob;
        let glob_color = if glob_focused { Color::LightYellow } else { Color::DarkGray };
        let glob_cursor = if glob_focused { "_" } else { "" };
        let (glob_text, glob_style) = if state.glob.is_empty() && !glob_focused {
            ("  *.rs, src/**/*.ts  …".to_string(), Style::default().fg(Color::DarkGray))
        } else {
            (format!("  {}{}", state.glob, glob_cursor), Style::default().fg(Color::White))
        };
        let glob_block = Block::default()
            .title(Span::styled(
                " File filter (glob) — Tab to focus ",
                Style::default().fg(Color::LightYellow),
            ))
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(glob_color));
        let glob_para = Paragraph::new(Span::styled(glob_text, glob_style)).block(glob_block);
        frame.render_widget(glob_para, inner[1]);

        // ── Results list ──────────────────────────────────────────────────────
        let visible_height = inner[2].height.saturating_sub(2) as usize;

        let status_title = match &state.status {
            SearchStatus::Idle => " Results ".to_string(),
            SearchStatus::Running => " Results  (searching…) ".to_string(),
            SearchStatus::Done => format!(
                " {} result{} ",
                state.results.len(),
                if state.results.len() == 1 { "" } else { "s" }
            ),
            SearchStatus::Error(e) => format!(" Error: {} ", e),
        };

        let results_block = Block::default()
            .title(Span::styled(status_title, Style::default().fg(Color::LightRed)))
            .title_bottom(Span::styled(
                "  Tab=switch fields   ↑/↓ or j/k  navigate   Enter  open   Esc  close",
                Style::default().fg(Color::DarkGray),
            ))
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::LightRed));

        // Scroll so the selected result stays within the visible window.
        let selected = state.selected;
        let scroll = if selected >= visible_height { selected - visible_height + 1 } else { 0 };

        let mut lines: Vec<Line> = Vec::new();

        if state.results.is_empty() {
            let msg = match &state.status {
                SearchStatus::Idle => "  Type a query to search across project files…",
                SearchStatus::Running => "  Searching…",
                SearchStatus::Done => "  No results.",
                SearchStatus::Error(_) => "  Search failed — check the title bar for the error.",
            };
            lines.push(Line::from(Span::styled(msg, Style::default().fg(Color::DarkGray))));
        } else {
            for (idx, result) in state.results.iter().enumerate().skip(scroll).take(visible_height)
            {
                let is_selected = idx == selected;
                let bg = if is_selected { Color::Rgb(40, 60, 90) } else { Color::Reset };
                let prefix = if is_selected { "► " } else { "  " };

                // Truncate long match text to avoid wrapping.
                let text_preview: String = result.text.trim().chars().take(60).collect();
                let loc = format!("{}:{}:  ", result.rel_path, result.line + 1);

                let line = if is_selected {
                    Line::from(vec![
                        Span::styled(prefix.to_string(), Style::default().bg(bg)),
                        Span::styled(
                            loc,
                            Style::default().bg(bg).fg(Color::Yellow).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(text_preview, Style::default().bg(bg).fg(Color::White)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(prefix),
                        Span::styled(loc, Style::default().fg(Color::Gray)),
                        Span::styled(text_preview, Style::default().fg(Color::White)),
                    ])
                };
                lines.push(line);
            }
        }

        let results_para = Paragraph::new(lines).block(results_block);
        frame.render_widget(results_para, inner[2]);
    }

    /// Render the status line
    #[allow(clippy::too_many_arguments)]
    fn render_status_line(
        frame: &mut Frame,
        buffer_data: Option<&BufferData>,
        mode: Mode,
        status_message: Option<&str>,
        command_buffer: Option<&str>,
        key_sequence: &str,
        area: Rect,
        diagnostics: &[Diagnostic],
    ) {
        let mode_str = match mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
            Mode::PickBuffer => "PICK",
            Mode::PickFile => "FIND",
            Mode::Agent => "AGENT",
            Mode::Explorer => "EXPLORE",
            Mode::MarkdownPreview => "PREVIEW",
            Mode::Search => "SEARCH",
            Mode::InFileSearch => "SEARCH",
            Mode::RenameFile => "RENAME",
            Mode::DeleteFile => "DELETE",
            Mode::NewFolder => "MKDIR",
            Mode::ApplyDiff => "DIFF",
            Mode::CommitMsg => "COMMIT",
            Mode::ReleaseNotes => "RELEASE",
            Mode::Diagnostics => "DIAG",
        };

        let mode_color = match mode {
            Mode::Normal => Color::Blue,
            Mode::Insert => Color::Green,
            Mode::Command => Color::Yellow,
            Mode::Visual => Color::Magenta,
            Mode::VisualLine => Color::Magenta,
            Mode::PickBuffer => Color::Cyan,
            Mode::PickFile => Color::LightCyan,
            Mode::Agent => Color::Cyan,
            Mode::Explorer => Color::LightGreen,
            Mode::MarkdownPreview => Color::Magenta,
            Mode::Search => Color::LightRed,
            Mode::InFileSearch => Color::LightRed,
            Mode::RenameFile => Color::Yellow,
            Mode::DeleteFile => Color::Red,
            Mode::NewFolder => Color::LightGreen,
            Mode::ApplyDiff => Color::Cyan,
            Mode::CommitMsg => Color::LightYellow,
            Mode::ReleaseNotes => Color::LightCyan,
            Mode::Diagnostics => Color::LightCyan,
        };

        let mut spans = vec![
            Span::styled(
                format!(" {} ", mode_str),
                Style::default().fg(Color::Black).bg(mode_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];

        // Show key sequence if building one
        if !key_sequence.is_empty() {
            spans.push(Span::styled(
                key_sequence,
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
        }

        // Buffer name and modified indicator
        if let Some((name, is_modified, cursor, _, _, _, _)) = buffer_data {
            let modified = if *is_modified { " [+]" } else { "" };
            spans.push(Span::raw(format!("{}{}", name, modified)));
            spans.push(Span::raw(" "));

            // Cursor position
            spans.push(Span::styled(
                format!("{}:{}", cursor.row + 1, cursor.col + 1),
                Style::default().fg(Color::Gray),
            ));
        }

        // Diagnostic count
        if !diagnostics.is_empty() {
            let error_count = diagnostics
                .iter()
                .filter(|d| matches!(d.severity, Some(lsp_types::DiagnosticSeverity::ERROR)))
                .count();
            let warning_count = diagnostics
                .iter()
                .filter(|d| matches!(d.severity, Some(lsp_types::DiagnosticSeverity::WARNING)))
                .count();

            spans.push(Span::raw(" "));
            if error_count > 0 {
                spans.push(Span::styled(
                    format!("● {}", error_count),
                    Style::default().fg(Color::Red),
                ));
            }
            if warning_count > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("⚠ {}", warning_count),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }

        // Status message or command buffer
        if let Some(cmd) = command_buffer {
            spans = vec![Span::raw(format!(":{}", cmd))];
        } else if let Some(msg) = status_message {
            // Show status message on the right
            let msg_span = Span::styled(msg, Style::default().fg(Color::Yellow));
            spans.push(Span::raw(" "));
            spans.push(msg_span);
        }

        let status_line = Line::from(spans);
        let paragraph = Paragraph::new(status_line).style(Style::default().bg(Color::Black));

        frame.render_widget(paragraph, area);
    }

    /// Render the centred delete confirmation popup (Mode::DeleteFile).
    fn render_delete_popup(frame: &mut Frame, name: &str, area: Rect) {
        let popup_width = 52.min(area.width);
        let popup_height = 3u16;
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let display = format!(" Delete '{}'?  [y/N] ", name);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" Delete ");
        frame.render_widget(Paragraph::new(display).block(block), popup_area);
    }

    /// Render the centred new-folder popup (Mode::NewFolder).
    fn render_new_folder_popup(frame: &mut Frame, folder_buffer: &str, area: Rect) {
        let popup_width = 50.min(area.width);
        let popup_height = 3u16;
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let display = format!(" {}_", folder_buffer); // trailing _ acts as cursor
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightGreen))
            .title(" New Folder ");
        frame.render_widget(Paragraph::new(display).block(block), popup_area);
    }

    /// Render the centred commit-message popup (Mode::CommitMsg).
    fn render_commit_msg_popup(frame: &mut Frame, msg: &str, area: Rect) {
        let popup_width = 80.min(area.width);
        // Height: 2 borders + hint line + content lines (min 4, max 12)
        let content_lines = msg.lines().count().clamp(4, 12) as u16;
        let popup_height = (content_lines + 3).min(area.height);
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let hint = Line::from(Span::styled(
            " Enter=commit   Esc=discard   (edit freely) ",
            Style::default().fg(Color::DarkGray),
        ));
        let content_lines_rendered: Vec<Line<'static>> =
            msg.lines().map(|l| Line::from(Span::raw(format!(" {l}")))).collect();
        let mut all_lines = content_lines_rendered;
        all_lines.insert(0, hint);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightYellow))
            .title(Span::styled(
                " Commit Message ",
                Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
            ));
        frame.render_widget(Paragraph::new(all_lines).block(block), popup_area);
    }

    /// Render the centred release notes popup (Mode::ReleaseNotes).
    fn render_release_notes_popup(frame: &mut Frame, view: &ReleaseNotesView<'_>, area: Rect) {
        let popup_width = 90.min(area.width);
        let popup_height = (area.height * 3 / 4).max(10).min(area.height);
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let (title_str, hint_line, body_lines): (_, Line<'static>, Vec<Line<'static>>) =
            if view.generating {
                // Phase 2: generating
                (
                    " Release Notes ",
                    Line::from(Span::styled(" Esc=cancel ", Style::default().fg(Color::DarkGray))),
                    vec![Line::from(Span::styled(
                        " Generating release notes…",
                        Style::default().fg(Color::Yellow),
                    ))],
                )
            } else if view.notes.is_empty() {
                // Phase 1: count entry
                let display = format!(" Commits to include: {}_", view.count_input);
                (
                    " Release Notes ",
                    Line::from(Span::styled(
                        " Enter=generate   Esc=cancel ",
                        Style::default().fg(Color::DarkGray),
                    )),
                    vec![Line::from(Span::styled(display, Style::default().fg(Color::White)))],
                )
            } else {
                // Phase 3: displaying
                let lines =
                    view.notes.lines().map(|l| Line::from(Span::raw(format!(" {l}")))).collect();
                (
                    " Release Notes ",
                    Line::from(vec![
                        Span::styled(" y", Style::default().fg(Color::Green)),
                        Span::styled("=copy  ", Style::default().fg(Color::DarkGray)),
                        Span::styled("j/k", Style::default().fg(Color::Green)),
                        Span::styled("=scroll  ", Style::default().fg(Color::DarkGray)),
                        Span::styled("Esc", Style::default().fg(Color::Green)),
                        Span::styled("=close ", Style::default().fg(Color::DarkGray)),
                    ]),
                    lines,
                )
            };

        let mut all_lines = body_lines;
        all_lines.insert(0, hint_line);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightCyan))
            .title(Span::styled(
                title_str,
                Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
            ));

        let para = Paragraph::new(all_lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((view.scroll, 0));
        frame.render_widget(para, popup_area);
    }

    /// Render the centred rename popup (Mode::RenameFile).
    fn render_rename_popup(frame: &mut Frame, rename_buffer: &str, area: Rect) {
        let popup_width = 50.min(area.width);
        let popup_height = 3u16;
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let display = format!(" {}_", rename_buffer); // trailing _ acts as cursor
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Rename ");
        frame.render_widget(Paragraph::new(display).block(block), popup_area);
    }

    /// Render the full-screen apply-diff overlay (Mode::ApplyDiff).
    fn render_apply_diff_overlay(frame: &mut Frame, view: &ApplyDiffView<'_>, area: Rect) {
        frame.render_widget(Clear, area);

        let header_area = Rect { x: area.x, y: area.y, width: area.width, height: 3 };
        let body_area = Rect {
            x: area.x,
            y: area.y + 3,
            width: area.width,
            height: area.height.saturating_sub(3),
        };

        let title = format!(" Apply diff → {} ", view.target);
        let hints = "  [y/Enter] apply   [n/Esc] discard   [j/k] scroll   [Ctrl+D/U] half-page ";
        let header_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                " Apply Diff ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
        let header_para = Paragraph::new(vec![
            Line::from(Span::styled(
                title,
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(hints, Style::default().fg(Color::DarkGray))),
        ])
        .block(header_block);
        frame.render_widget(header_para, header_area);

        let visible_h = body_area.height as usize;
        let total = view.lines.len();
        let scroll = view.scroll.min(total.saturating_sub(1));
        let diff_lines: Vec<Line<'static>> = view
            .lines
            .iter()
            .skip(scroll)
            .take(visible_h)
            .map(|dl| match dl {
                DiffLine::Added(s) => {
                    Line::from(Span::styled(format!("+ {s}"), Style::default().fg(Color::Green)))
                },
                DiffLine::Removed(s) => {
                    Line::from(Span::styled(format!("- {s}"), Style::default().fg(Color::Red)))
                },
                DiffLine::Context(s) => {
                    Line::from(Span::styled(format!("  {s}"), Style::default().fg(Color::DarkGray)))
                },
            })
            .collect();
        frame.render_widget(Paragraph::new(diff_lines), body_area);

        if total > visible_h {
            let indicator = format!(" {}/{} ", scroll + 1, total);
            let w = indicator.len() as u16;
            if w < body_area.width {
                let ind = Rect {
                    x: body_area.x + body_area.width - w,
                    y: body_area.y + body_area.height.saturating_sub(1),
                    width: w,
                    height: 1,
                };
                frame.render_widget(
                    Paragraph::new(Span::styled(indicator, Style::default().fg(Color::DarkGray))),
                    ind,
                );
            }
        }
    }

    /// Render the diagnostics overlay (Mode::Diagnostics).
    /// Shows MCP server status and LSP servers. Any key closes it.
    fn render_diagnostics_overlay(frame: &mut Frame, data: &DiagnosticsData<'_>, area: Rect) {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // ── MCP Servers ───────────────────────────────────────────────────────
        lines.push(Line::from(vec![Span::styled(
            " MCP Servers ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )]));

        if data.mcp_connected.is_empty() && data.mcp_failed.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  none configured",
                Style::default().fg(Color::DarkGray),
            )]));
        }

        for (name, count) in &data.mcp_connected {
            lines.push(Line::from(vec![
                Span::styled("  ✓ ", Style::default().fg(Color::Green)),
                Span::styled(
                    name.to_string(),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {count} tools"), Style::default().fg(Color::DarkGray)),
            ]));
        }

        for (name, reason) in data.mcp_failed {
            lines.push(Line::from(vec![
                Span::styled("  ✗ ", Style::default().fg(Color::Red)),
                Span::styled(
                    name.to_string(),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  failed: ", Style::default().fg(Color::DarkGray)),
            ]));
            // Wrap each line of the reason so long errors are readable.
            for err_line in reason.lines() {
                lines.push(Line::from(vec![Span::styled(
                    format!("      {err_line}"),
                    Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
                )]));
            }
        }

        // ── LSP Servers ───────────────────────────────────────────────────────
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " LSP Servers ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )]));

        if data.lsp_servers.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  none configured",
                Style::default().fg(Color::DarkGray),
            )]));
        }

        for name in &data.lsp_servers {
            lines.push(Line::from(vec![
                Span::styled("  ● ", Style::default().fg(Color::Green)),
                Span::styled(name.to_string(), Style::default().fg(Color::White)),
            ]));
        }

        // ── Recent logs ───────────────────────────────────────────────────────
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                " Recent Logs ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  ({})", data.log_path), Style::default().fg(Color::DarkGray)),
        ]));

        if data.recent_logs.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  no warnings or errors",
                Style::default().fg(Color::DarkGray),
            )]));
        }

        for (level, msg) in data.recent_logs {
            let (prefix, color) = match level.as_str() {
                "ERROR" => ("  ERROR ", Color::Red),
                "WARN" => ("  WARN  ", Color::Yellow),
                _ => ("  INFO  ", Color::DarkGray),
            };
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(msg.clone(), Style::default().fg(Color::White)),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " press any key to close ",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        )]));

        // Centre the popup.
        let popup_width = 60.min(area.width);
        let popup_height = (lines.len() as u16 + 2).min(area.height);
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .title(Span::styled(
                " Diagnostics ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(
            Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
            popup_area,
        );
    }

    // ── File-info popup helpers ───────────────────────────────────────────────

    /// Format a byte count as a human-readable string with the raw count in parens.
    fn format_file_size(bytes: u64) -> String {
        const KB: u64 = 1_024;
        const MB: u64 = 1_024 * 1_024;
        const GB: u64 = 1_024 * 1_024 * 1_024;
        if bytes >= GB {
            format!("{:.1} GB  ({bytes} bytes)", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.1} MB  ({bytes} bytes)", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.1} KB  ({bytes} bytes)", bytes as f64 / KB as f64)
        } else {
            format!("{bytes} bytes")
        }
    }

    /// Format a `SystemTime` as "YYYY-MM-DD  HH:MM" without external dependencies.
    /// Uses Howard Hinnant's epoch-to-civil-date algorithm.
    fn format_system_time(t: std::time::SystemTime) -> String {
        let secs = match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(_) => return "(unknown)".to_string(),
        };
        let min = (secs / 60) % 60;
        let hour = (secs / 3_600) % 24;
        let days = secs / 86_400;
        // Gregorian calendar decomposition (Hinnant 2013).
        let z = days as i64 + 719_468;
        let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mon = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if mon <= 2 { y + 1 } else { y };
        format!("{year:04}-{mon:02}-{d:02}  {hour:02}:{min:02}")
    }

    /// Render the file-info popup triggered by `i` in Mode::Explorer.
    ///
    /// Floats to the right of the explorer panel so the tree stays readable.
    /// `explorer_right` is the x-coordinate of the first column past the
    /// explorer's right border (25 when the explorer is visible, 0 otherwise).
    fn render_file_info_popup(
        frame: &mut Frame,
        info: &FileInfoData,
        area: Rect,
        explorer_right: u16,
    ) {
        let available_w = area.width.saturating_sub(explorer_right);
        let popup_width = available_w.clamp(30, 58);
        let inner_w = popup_width.saturating_sub(4) as usize; // 2 borders + 2 padding

        // ── Content rows ──────────────────────────────────────────────────────
        let name = info
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| info.path.to_string_lossy().to_string());

        let full_path = info.path.to_string_lossy().to_string();
        // Truncate path from the left so the filename end is always visible.
        let path_display = if full_path.len() > inner_w {
            format!("…{}", &full_path[full_path.len().saturating_sub(inner_w - 1)..])
        } else {
            full_path
        };

        let type_label: &str = if info.is_dir {
            "directory"
        } else {
            info.path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| match e.to_ascii_lowercase().as_str() {
                    "rs" => "Rust source",
                    "py" => "Python source",
                    "js" => "JavaScript",
                    "ts" => "TypeScript",
                    "html" => "HTML file",
                    "css" => "CSS file",
                    "json" => "JSON file",
                    "toml" => "TOML config",
                    "yaml" | "yml" => "YAML file",
                    "md" => "Markdown",
                    "txt" => "text file",
                    "sh" | "bash" | "zsh" => "shell script",
                    "xml" => "XML file",
                    "csv" => "CSV file",
                    _ => "file",
                })
                .unwrap_or("file")
        };

        let dim = Style::default().fg(Color::DarkGray);
        let val = Style::default().fg(Color::White);

        let mut rows: Vec<Line<'static>> = vec![
            // Full path (Cyan, left-truncated)
            Line::from(Span::styled(format!(" {path_display}"), Style::default().fg(Color::Cyan))),
            Line::from(""),
            // Type row
            Line::from(vec![
                Span::styled(" Type       ".to_string(), dim),
                Span::styled(type_label.to_string(), val),
            ]),
        ];

        // Size (files only)
        if let Some(bytes) = info.size_bytes {
            rows.push(Line::from(vec![
                Span::styled(" Size       ".to_string(), dim),
                Span::styled(Self::format_file_size(bytes), val),
            ]));
        }

        // Timestamps
        if let Some(t) = info.modified {
            rows.push(Line::from(vec![
                Span::styled(" Modified   ".to_string(), dim),
                Span::styled(Self::format_system_time(t), val),
            ]));
        }
        if let Some(t) = info.created {
            rows.push(Line::from(vec![
                Span::styled(" Created    ".to_string(), dim),
                Span::styled(Self::format_system_time(t), val),
            ]));
        }

        // Unix permissions (None on Windows)
        if let Some(ref perms) = info.permissions {
            rows.push(Line::from(vec![
                Span::styled(" Perms      ".to_string(), dim),
                Span::styled(perms.clone(), val),
            ]));
        }

        rows.push(Line::from(""));
        rows.push(Line::from(Span::styled(
            "  [i] close  ·  navigate to update",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        )));

        let popup_height = (rows.len() as u16 + 2).min(area.height);

        // ── Positioning ───────────────────────────────────────────────────────
        // Anchor to the right of the explorer; clamp so it never leaves the screen.
        let x = explorer_right.min(area.width.saturating_sub(popup_width));
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let name_title = if name.len() > inner_w {
            format!("{}…", &name[..inner_w.saturating_sub(1)])
        } else {
            name
        };
        let block = Block::default()
            .title(Span::styled(
                format!(" {name_title} "),
                Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        frame.render_widget(Paragraph::new(rows).block(block), popup_area);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Think-aware message renderer
// ─────────────────────────────────────────────────────────────────────────────

/// Render an assistant message with `<think>` blocks styled as plain dim text
/// and everything else rendered as formatted markdown.
///
/// Thinking blocks get a `◌ thinking` header and word-wrapped dim-gray text;
/// the actual reply beneath is passed unchanged through the markdown renderer.
fn render_message_content(content: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    for segment in split_thinking(content) {
        match segment {
            ContentSegment::Thinking(text) => {
                // Header line.
                lines.push(Line::from(vec![Span::styled(
                    "◌ thinking",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )]));
                // Plain word-wrap — no markdown parsing inside thinking blocks.
                for paragraph in text.split('\n') {
                    if paragraph.trim().is_empty() {
                        lines.push(Line::from(""));
                        continue;
                    }
                    let mut col = 0usize;
                    let mut current = String::new();
                    for word in paragraph.split_whitespace() {
                        let wlen = word.chars().count();
                        if col > 0 && width > 0 && col + 1 + wlen > width {
                            lines.push(Line::from(vec![Span::styled(
                                current.clone(),
                                Style::default().fg(Color::DarkGray),
                            )]));
                            current = word.to_owned();
                            col = wlen;
                        } else {
                            if col > 0 {
                                current.push(' ');
                                col += 1;
                            }
                            current.push_str(word);
                            col += wlen;
                        }
                    }
                    if !current.is_empty() {
                        lines.push(Line::from(vec![Span::styled(
                            current,
                            Style::default().fg(Color::DarkGray),
                        )]));
                    }
                }
                // Spacer between thinking block and the answer.
                lines.push(Line::from(""));
            },
            ContentSegment::Normal(text) => {
                lines.extend(crate::markdown::render(&text, width));
            },
        }
    }

    lines
}
