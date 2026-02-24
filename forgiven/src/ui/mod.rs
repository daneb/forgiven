use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use std::path::PathBuf;
use lsp_types::Diagnostic;

use crate::agent::{AgentPanel, Role};
use crate::buffer::{Cursor, Selection};
use crate::explorer::FileExplorer;
use crate::keymap::Mode;

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
    ) {
        let size = frame.area();

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

        // ── Horizontal splits: [explorer?] | [editor] | [agent?] ─────────────
        let explorer_visible = file_explorer.map(|e| e.visible).unwrap_or(false);
        let agent_visible = agent_panel.map(|p| p.visible).unwrap_or(false);

        let (explorer_area, content_area, agent_area) = match (explorer_visible, agent_visible) {
            (true, true) => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(25), Constraint::Min(1), Constraint::Percentage(35)])
                    .split(size);
                (Some(cols[0]), cols[1], Some(cols[2]))
            }
            (true, false) => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(25), Constraint::Min(1)])
                    .split(size);
                (Some(cols[0]), cols[1], None)
            }
            (false, true) => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(size);
                (None, cols[0], Some(cols[1]))
            }
            (false, false) => (None, size, None),
        };
        let editor_area = content_area;

        // ── Vertical layout (buffer + status) inside editor_area ─────────────
        let constraints = if which_key_options.is_some() {
            vec![
                Constraint::Min(1),         // Main buffer area
                Constraint::Length(10),     // Which-key popup
                Constraint::Length(1),      // Status line
            ]
        } else {
            vec![
                Constraint::Min(1),         // Main buffer area
                Constraint::Length(1),      // Status line
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(editor_area);

        let main_area = chunks[0];
        let status_area = if which_key_options.is_some() {
            let which_key_area = chunks[1];
            Self::render_which_key(frame, which_key_options.unwrap(), which_key_area);
            chunks[2]
        } else {
            chunks[1]
        };

        // Render buffer content
        Self::render_buffer(frame, buffer_data, mode, main_area, diagnostics, ghost_text, highlighted_lines);

        // Render status line
        Self::render_status_line(frame, buffer_data, mode, status_message, command_buffer, key_sequence, status_area, diagnostics);

        // Render agent panel if visible
        if let (Some(panel), Some(area)) = (agent_panel, agent_area) {
            Self::render_agent_panel(frame, panel, mode, area);
        }

        // Render file explorer if visible
        if let (Some(explorer), Some(area)) = (file_explorer, explorer_area) {
            Self::render_file_explorer(frame, explorer, mode, area);
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

        // Compute input box height: expand as the user types, up to 5 text lines (7 rows).
        // content_width = panel width minus 2 border columns.
        // We calculate how many wrapped lines the current input occupies, then add 2 for borders.
        let content_width = area.width.saturating_sub(2) as usize;
        let text_len = panel.input.chars().count() + 1; // +1 for the trailing cursor char
        let wrapped_lines = if content_width > 0 {
            (text_len + content_width - 1) / content_width
        } else {
            1
        };
        // At least 1 text line; at most 5 text lines to keep history visible.
        let input_text_lines = (wrapped_lines.max(1).min(5)) as u16;
        let input_height = input_text_lines + 2; // +2 for top/bottom borders

        // Split area vertically: history (top) + input (dynamic bottom).
        let vchunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(input_height)])
            .split(area);

        let history_area = vchunks[0];
        let input_area = vchunks[1];

        // ── Chat history ──────────────────────────────────────────────────────
        let mut lines: Vec<Line> = Vec::new();
        let content_width = history_area.width.saturating_sub(4) as usize;

        // Render message content, visually separating tool-call lines from prose.
        // Lines starting with ⚙ are tool operations — shown dim.
        // The first prose line after a block of tool lines gets a faint separator.
        let render_content = |content: &str, lines: &mut Vec<Line>| {
            let mut prev_was_tool = false;
            for content_line in content.lines() {
                let trimmed = content_line.trim_start();
                let is_tool = trimmed.starts_with('⚙');

                if trimmed.is_empty() {
                    // Only emit blank lines in the prose section
                    if !prev_was_tool {
                        lines.push(Line::from(""));
                    }
                    continue;
                }

                // Thin separator when transitioning from tools → final answer
                if prev_was_tool && !is_tool {
                    lines.push(Line::from(Span::styled(
                        "  ────────────────────",
                        Style::default().fg(Color::DarkGray),
                    )));
                }

                if is_tool {
                    // Tool line: dim, no word-wrap (already compact)
                    lines.push(Line::from(Span::styled(
                        format!("  {trimmed}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    // Prose: word-wrap in white
                    let words = content_line.split_whitespace().collect::<Vec<_>>();
                    let mut cur = String::new();
                    for word in &words {
                        if cur.is_empty() {
                            cur = word.to_string();
                        } else if cur.len() + 1 + word.len() <= content_width {
                            cur.push(' ');
                            cur.push_str(word);
                        } else {
                            lines.push(Line::from(Span::styled(
                                format!("  {cur}"),
                                Style::default().fg(Color::White),
                            )));
                            cur = word.to_string();
                        }
                    }
                    if !cur.is_empty() {
                        lines.push(Line::from(Span::styled(
                            format!("  {cur}"),
                            Style::default().fg(Color::White),
                        )));
                    }
                }
                prev_was_tool = is_tool;
            }
        };

        for msg in &panel.messages {
            let (label, color) = match msg.role {
                Role::User => ("You", Color::Green),
                Role::Assistant => ("Copilot", Color::Cyan),
                Role::System => ("System", Color::DarkGray),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("╔ {label} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ]));
            render_content(&msg.content, &mut lines);
            lines.push(Line::from(""));
        }

        // Show in-progress streaming reply.
        if let Some(ref partial) = panel.streaming_reply {
            lines.push(Line::from(vec![
                Span::styled("╔ Copilot ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled("▋", Style::default().fg(Color::Yellow).add_modifier(Modifier::SLOW_BLINK)),
            ]));
            render_content(partial, &mut lines);
        }

        // Scroll: panel.scroll=0 means pinned to bottom (newest); higher = scrolled up.
        let visible_height = history_area.height.saturating_sub(2) as usize; // account for border
        let total = lines.len();

        // Cap scroll so we never scroll past the top.
        let max_scroll = total.saturating_sub(visible_height);
        let scroll = panel.scroll.min(max_scroll);

        // Compute the slice: count back from the bottom, then offset by scroll.
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(visible_height);
        let visible_lines = lines[start..end].to_vec();

        // Build a title that shows the active model and scroll position.
        let model_label = panel.selected_model_id();
        let history_title = if scroll > 0 {
            let pct = if max_scroll > 0 { 100 - (scroll * 100 / max_scroll).min(100) } else { 100 };
            format!(" Copilot Chat [{model_label}]  ↑ scrolled ({pct}%)  ↑/↓ to navigate ")
        } else if total > visible_height {
            format!(" Copilot Chat [{model_label}]  (↑ to scroll up) ")
        } else {
            format!(" Copilot Chat [{model_label}] ")
        };

        let history_block = Block::default()
            .title(history_title)
            .borders(Borders::ALL)
            .border_style(border_style);
        let history_para = Paragraph::new(visible_lines)
            .block(history_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(history_para, history_area);

        // ── Input box ─────────────────────────────────────────────────────────
        let input_text = if focused {
            format!("{}_", panel.input)  // trailing cursor block
        } else {
            panel.input.clone()
        };
        // Show [a] apply hint when the latest reply contains a code block.
        let hint = if panel.messages.is_empty() {
            " Ask Copilot… (Enter=send, Ctrl+T=model, Tab=back) ".to_string()
        } else if panel.has_code_to_apply() && panel.input.is_empty() {
            " Message Copilot… | [a] apply  Ctrl+T=model ".to_string()
        } else {
            " Message Copilot… (Ctrl+T=model) ".to_string()
        };
        let hint = hint.as_str();
        let input_block = Block::default()
            .title(hint)
            .borders(Borders::ALL)
            .border_style(border_style);
        let input_para = Paragraph::new(input_text)
            .block(input_block)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false });
        frame.render_widget(input_para, input_area);
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
        let scroll = if cursor >= visible_height {
            cursor - visible_height + 1
        } else {
            0
        };

        let mut lines: Vec<Line> = Vec::new();
        for (i, node) in flat.iter().enumerate().skip(scroll).take(visible_height) {
            let is_selected = i == cursor;

            let indent = "  ".repeat(node.depth);
            let icon = if node.is_dir {
                if node.is_expanded { "▼ " } else { "▶ " }
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

        let root_name = explorer.root_path
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

    /// Render the buffer content
    fn render_buffer(
        frame: &mut Frame,
        buffer_data: Option<&BufferData>,
        mode: Mode,
        area: Rect,
        diagnostics: &[Diagnostic],
        ghost_text: Option<(&str, usize, usize)>,
        highlighted_lines: Option<&[Vec<Span<'static>>]>,
    ) {
        if let Some((_, _, cursor, scroll_row, scroll_col, lines, selection)) = buffer_data {
            let viewport_height = area.height as usize;
            let viewport_width = area.width as usize;

            // Calculate visible line range
            let start_line = *scroll_row;
            let end_line = (start_line + viewport_height).min(lines.len());

            // Build visible lines
            let mut visible_lines = Vec::new();
            for row in start_line..end_line {
                if let Some(line_text) = lines.get(row) {
                    // Check if this line has any diagnostics
                    let has_diagnostic = diagnostics.iter().any(|d| d.range.start.line as usize == row);
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
                    let line = if let Some(spans) = highlighted_lines.and_then(|h| h.get(line_idx)) {
                        Self::render_highlighted_line(spans, *scroll_col, viewport_width, has_diagnostic, row_ghost, selection, row)
                    } else {
                        Self::render_line(line_text, *scroll_col, viewport_width, row, selection, *scroll_row, has_diagnostic, row_ghost)
                    };
                    visible_lines.push(line);
                } else {
                    visible_lines.push(Line::from("~"));
                }
            }

            // Fill remaining lines with ~
            for _ in visible_lines.len()..viewport_height {
                visible_lines.push(Line::from(Span::styled("~", Style::default().fg(Color::DarkGray))));
            }

            let paragraph = Paragraph::new(visible_lines);
            frame.render_widget(paragraph, area);

            // Render cursor (only in Normal, Insert modes).
            // GUTTER_WIDTH accounts for the 2-char diagnostic marker ("  " / "● ")
            // prepended to every rendered line — the cursor must be offset by the same amount.
            const GUTTER_WIDTH: u16 = 2;
            if mode != Mode::PickBuffer {
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
            // No buffer open
            let text = vec![Line::from("No buffer open")];
            let paragraph = Paragraph::new(text);
            frame.render_widget(paragraph, area);
        }
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
                        }
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
            vec![
                Span::styled("● ", Style::default().fg(Color::Red)),
            ]
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
                line_spans.push(Span::styled(
                    g.to_string(),
                    Style::default().fg(Color::DarkGray),
                ));
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
                line_spans.push(Span::styled(
                    g.to_string(),
                    Style::default().fg(Color::DarkGray),
                ));
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
                Constraint::Length(3),  // query input
                Constraint::Min(1),     // results
            ])
            .split(picker_area);

        // ── Query input box ─────────────────────────────────────────────────────
        let query_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightCyan))
            .title(Span::styled(" Find File ", Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD)))
            .title_bottom(Span::styled(
                format!(" {} files ", files.len()),
                Style::default().fg(Color::DarkGray),
            ));
        let query_display = format!("> {query}_");
        let query_para = Paragraph::new(Span::styled(
            query_display,
            Style::default().fg(Color::White),
        ))
        .block(query_block);
        frame.render_widget(query_para, inner[0]);

        // ── Results list ────────────────────────────────────────────────────────
        let mut lines: Vec<Line> = Vec::new();

        for (idx, (path, match_indices)) in files.iter().enumerate().take(20) {
            let display: String = path.strip_prefix(&current_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

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
                    Style::default().bg(bg).fg(if is_selected { Color::White } else { Color::Reset }),
                )];
                let chars: Vec<char> = display.chars().collect();
                let mut ci = 0;
                let mut seg = String::new();
                for (char_idx, &ch) in chars.iter().enumerate() {
                    let is_match = match_indices.contains(&char_idx);
                    let was_match = char_idx > 0 && match_indices.contains(&(char_idx - 1));
                    if is_match != was_match && !seg.is_empty() {
                        // Flush the segment with previous style
                        let prev_is_match = !is_match;
                        let style = if prev_is_match {
                            Style::default().bg(bg).fg(Color::Yellow).add_modifier(Modifier::BOLD)
                        } else if is_selected {
                            Style::default().bg(bg).fg(Color::White).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().bg(bg).fg(Color::White)
                        };
                        spans.push(Span::styled(std::mem::take(&mut seg), style));
                    }
                    seg.push(ch);
                    ci = char_idx;
                }
                // Flush the last segment
                if !seg.is_empty() {
                    let last_is_match = match_indices.contains(&ci);
                    let style = if last_is_match {
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

    /// Render the status line
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
        };

        let mut spans = vec![
            Span::styled(
                format!(" {} ", mode_str),
                Style::default()
                    .fg(Color::Black)
                    .bg(mode_color)
                    .add_modifier(Modifier::BOLD),
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
            let error_count = diagnostics.iter().filter(|d| {
                matches!(d.severity, Some(lsp_types::DiagnosticSeverity::ERROR))
            }).count();
            let warning_count = diagnostics.iter().filter(|d| {
                matches!(d.severity, Some(lsp_types::DiagnosticSeverity::WARNING))
            }).count();
            
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
        let paragraph = Paragraph::new(status_line)
            .style(Style::default().bg(Color::Black));

        frame.render_widget(paragraph, area);
    }
}
