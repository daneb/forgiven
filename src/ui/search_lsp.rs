use crate::editor::HoverPopupState;

use super::*;

impl UI {
    /// Render the project-wide ripgrep search overlay (Mode::Search).
    pub(super) fn render_search_panel(frame: &mut Frame, state: &SearchState, area: Rect) {
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

    /// Render the LSP location-list overlay (Mode::LocationList).
    pub(super) fn render_location_list(frame: &mut Frame, state: &LocationListState, area: Rect) {
        let popup_width = 80.min(area.width);
        let popup_height =
            (state.entries.len().min(u16::MAX as usize) as u16 + 4).min(area.height * 4 / 5).max(6);
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

        frame.render_widget(Clear, popup_area);

        let visible_height = popup_area.height.saturating_sub(3) as usize;
        let selected = state.selected;
        let scroll = if selected >= visible_height { selected - visible_height + 1 } else { 0 };

        let mut lines: Vec<Line> = Vec::new();
        for (idx, entry) in state.entries.iter().enumerate().skip(scroll).take(visible_height) {
            let is_sel = idx == selected;
            let bg = if is_sel { Color::Rgb(40, 60, 90) } else { Color::Reset };
            let prefix = if is_sel { "► " } else { "  " };
            let label_width = popup_area.width.saturating_sub(4) as usize;
            let label: String = entry.label.chars().take(label_width).collect();
            lines.push(Line::from(vec![Span::styled(
                format!("{prefix}{label}"),
                Style::default().fg(if is_sel { Color::White } else { Color::Gray }).bg(bg),
            )]));
        }

        let block = Block::default()
            .title(Span::styled(
                format!(" {} ", state.title),
                Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(
                "  j/k  navigate   Enter  jump   Esc  close",
                Style::default().fg(Color::DarkGray),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightCyan));

        let para = Paragraph::new(lines).block(block);
        frame.render_widget(para, popup_area);
    }

    /// Render the hover info popup (Mode::LspHover).
    pub(super) fn render_hover_popup(frame: &mut Frame, state: &HoverPopupState, area: Rect) {
        let popup_width = 80.min(area.width);
        let popup_height = (area.height * 3 / 5).max(6).min(area.height);
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

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(Span::styled(
                " Hover ",
                Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(
                "  j/k  scroll   Esc  close",
                Style::default().fg(Color::DarkGray),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightYellow));

        let para = Paragraph::new(state.content.as_str())
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((state.scroll, 0));
        frame.render_widget(para, popup_area);
    }

    /// Render the LSP rename input popup (Mode::LspRename).
    pub(super) fn render_lsp_rename_popup(frame: &mut Frame, buffer: &str, area: Rect) {
        let popup_width = 50.min(area.width);
        let popup_height = 3u16;
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

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(Span::styled(
                " Rename symbol ",
                Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightGreen));

        let display = format!("{buffer}█");
        let para = Paragraph::new(display.as_str()).block(block);
        frame.render_widget(para, popup_area);
    }
}
