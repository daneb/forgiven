use super::*;

impl UI {
    /// Render the which-key popup
    pub(super) fn render_which_key(frame: &mut Frame, options: &[(String, String)], area: Rect) {
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
    pub(super) fn render_buffer_picker(
        frame: &mut Frame,
        buffer_list: Option<&BufferList>,
        area: Rect,
    ) {
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
    pub(super) fn render_file_picker(frame: &mut Frame, file_list: Option<&FileList>, area: Rect) {
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
}
