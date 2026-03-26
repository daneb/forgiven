use super::*;

impl UI {
    /// Render the buffer content
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_buffer(
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
    pub(super) fn render_welcome(
        frame: &mut Frame,
        area: Rect,
        startup_elapsed: Option<std::time::Duration>,
    ) {
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
    pub(super) fn render_highlighted_line(
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
    pub(super) fn render_line(
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
}
