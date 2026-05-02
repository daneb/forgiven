use super::*;
use crate::buffer::visual_rows_for_len;

impl UI {
    /// Render the buffer content.
    ///
    /// When `fold_data` is supplied, rows inside closed folds are skipped and
    /// fold-start rows are annotated with a `··· N lines` stub.  When
    /// `sticky_header` is supplied, a 1-line context header is rendered at the
    /// top of `area` and the editor content is shifted down by one row.
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
        fold_data: Option<&FoldData>,
        sticky_header: Option<&str>,
        soft_wrap: bool,
        debt_report: Option<&crate::debt::DebtReport>,
        debt_narrative: Option<&str>,
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
            // ── Sticky scroll header (ADR 0107) ───────────────────────────────
            // Render the enclosing scope name as a 1-line overlay at the top of
            // the editor area when the viewport has scrolled past a scope boundary.
            let header_rows: u16 = if let Some(header_text) = sticky_header {
                let header_area = Rect { height: 1, ..area };
                let header_span = Span::styled(
                    format!("  {}", header_text.trim_end()),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                );
                frame.render_widget(Paragraph::new(Line::from(header_span)), header_area);
                1
            } else {
                0
            };

            // Content area excludes the sticky header row.
            let content_area = Rect {
                y: area.y + header_rows,
                height: area.height.saturating_sub(header_rows),
                ..area
            };
            let viewport_height = content_area.height as usize;
            let viewport_width = content_area.width as usize;

            let start_line = *scroll_row;
            let max_buf_line = start_line + lines.len();

            // ── Build visible lines (ADR 0106 fold rendering) ─────────────────
            // Iterate buffer rows from `start_line`, skipping hidden fold rows,
            // until we have filled `viewport_height` visible rows.
            let mut visible_lines: Vec<Line<'static>> = Vec::with_capacity(viewport_height);
            let mut buf_row = start_line;

            // text_width = columns available for text (gutter is 2 chars).
            let text_width = viewport_width.saturating_sub(2);

            while buf_row < max_buf_line && visible_lines.len() < viewport_height {
                // Skip rows hidden inside a closed fold.
                if let Some(fd) = fold_data {
                    if fd.hidden_rows.contains(&buf_row) {
                        buf_row += 1;
                        continue;
                    }
                }

                let line_idx = buf_row - start_line;
                let raw_line_text = lines.get(line_idx).map(String::as_str).unwrap_or("");

                let has_diagnostic =
                    diagnostics.iter().any(|d| d.range.start.line as usize == buf_row);

                // Ghost text is only shown on the exact row/col it was requested for.
                let row_ghost = ghost_text.and_then(|(text, ghost_row, ghost_col)| {
                    if buf_row == ghost_row && cursor.col == ghost_col {
                        Some(text.lines().next().unwrap_or(text))
                    } else {
                        None
                    }
                });

                if soft_wrap && text_width > 0 {
                    // ── Soft-wrap path: emit one visual row per segment ───────
                    let char_len = raw_line_text.chars().count();
                    let num_segs = visual_rows_for_len(char_len, text_width);

                    for seg in 0..num_segs {
                        if visible_lines.len() >= viewport_height {
                            break;
                        }
                        let seg_start_col = seg * text_width;
                        // Diagnostic marker only on the first segment; continuation
                        // lines get a plain blank gutter so the dot isn't repeated.
                        let seg_diag = has_diagnostic && seg == 0;
                        let seg_ghost = if seg == 0 { row_ghost } else { None };

                        let mut line =
                            if let Some(spans) = highlighted_lines.and_then(|h| h.get(line_idx)) {
                                Self::render_highlighted_line(
                                    spans,
                                    seg_start_col,
                                    viewport_width,
                                    seg_diag,
                                    seg_ghost,
                                    selection,
                                    buf_row,
                                )
                            } else {
                                Self::render_line(
                                    raw_line_text,
                                    seg_start_col,
                                    viewport_width,
                                    buf_row,
                                    selection,
                                    *scroll_row,
                                    seg_diag,
                                    seg_ghost,
                                )
                            };

                        // Fold stub only on the first segment.
                        if seg == 0 {
                            if let Some(fd) = fold_data {
                                if let Some(&end_row) = fd.fold_starts.get(&buf_row) {
                                    let n = end_row.saturating_sub(buf_row);
                                    let stub =
                                        format!(" ··· {} line{}", n, if n == 1 { "" } else { "s" });
                                    line.spans.push(Span::styled(
                                        stub,
                                        Style::default().fg(Color::DarkGray),
                                    ));
                                }
                            }
                        }

                        visible_lines.push(line);
                    }
                } else {
                    // ── Normal (no-wrap) path ─────────────────────────────────
                    // Use pre-highlighted spans when available (line_idx = buf_row - scroll_row
                    // correctly indexes the unfiltered highlighted_lines slice).
                    let mut line =
                        if let Some(spans) = highlighted_lines.and_then(|h| h.get(line_idx)) {
                            Self::render_highlighted_line(
                                spans,
                                *scroll_col,
                                viewport_width,
                                has_diagnostic,
                                row_ghost,
                                selection,
                                buf_row,
                            )
                        } else {
                            Self::render_line(
                                raw_line_text,
                                *scroll_col,
                                viewport_width,
                                buf_row,
                                selection,
                                *scroll_row,
                                has_diagnostic,
                                row_ghost,
                            )
                        };

                    // Append fold stub indicator for closed fold start rows.
                    if let Some(fd) = fold_data {
                        if let Some(&end_row) = fd.fold_starts.get(&buf_row) {
                            let n = end_row.saturating_sub(buf_row);
                            let stub = format!(" ··· {} line{}", n, if n == 1 { "" } else { "s" });
                            line.spans
                                .push(Span::styled(stub, Style::default().fg(Color::DarkGray)));
                        }
                    }

                    visible_lines.push(line);
                }

                buf_row += 1;
            }

            // Fill remaining rows with tildes.
            while visible_lines.len() < viewport_height {
                visible_lines
                    .push(Line::from(Span::styled("~", Style::default().fg(Color::DarkGray))));
            }

            frame.render_widget(Paragraph::new(visible_lines), content_area);

            // Render cursor (only in Normal, Insert modes, and only for the focused pane).
            // GUTTER_WIDTH accounts for the 2-char diagnostic marker ("  " / "● ")
            // prepended to every rendered line — the cursor must be offset by the same amount.
            const GUTTER_WIDTH: u16 = 2;
            if mode != Mode::PickBuffer && show_cursor {
                if soft_wrap && text_width > 0 {
                    // ── Soft-wrap cursor position ─────────────────────────────
                    // Count visual rows from scroll_row up to (but not including)
                    // cursor.row, then add the intra-line wrap offset.
                    let mut cursor_vrow: usize = 0;
                    for r in *scroll_row..cursor.row {
                        let len = lines
                            .get(r - start_line)
                            .map(|l: &String| l.chars().count())
                            .unwrap_or(0);
                        cursor_vrow += visual_rows_for_len(len, text_width);
                    }
                    cursor_vrow += cursor.col / text_width;
                    let cursor_vcol = cursor.col % text_width;

                    if cursor_vrow < viewport_height && cursor_vcol < text_width {
                        frame.set_cursor_position((
                            content_area.x + GUTTER_WIDTH + cursor_vcol as u16,
                            content_area.y + cursor_vrow as u16,
                        ));
                    }
                } else {
                    // ── Normal cursor position ────────────────────────────────
                    // `cursor.row` has been pre-adjusted by the editor to be the visual row
                    // (= buffer row minus hidden rows above it within the viewport).
                    let cursor_row = cursor.row.saturating_sub(*scroll_row);
                    let cursor_col = cursor.col.saturating_sub(*scroll_col);

                    if cursor_row < viewport_height && cursor_col < viewport_width {
                        frame.set_cursor_position((
                            content_area.x + GUTTER_WIDTH + cursor_col as u16,
                            content_area.y + cursor_row as u16,
                        ));
                    }
                }
            }
        } else {
            // No buffer open — show the welcome screen.
            Self::render_welcome(frame, area, startup_elapsed, debt_report, debt_narrative);
        }
    }

    /// Render the welcome / splash screen shown when no buffer is open.
    pub(super) fn render_welcome(
        frame: &mut Frame,
        area: Rect,
        startup_elapsed: Option<std::time::Duration>,
        debt_report: Option<&crate::debt::DebtReport>,
        debt_narrative: Option<&str>,
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
        const LOGO_W: usize = 64;

        let area_w = area.width as usize;

        // Decide vertical layout: logo block + optional debt block below.
        let debt_visible = debt_report.is_some();
        // Debt block: 1 blank + 1 header-separator + 3 data rows (per column) + optional narrative.
        let narrative_rows: u16 = if debt_narrative.is_some() { 3 } else { 0 };
        let debt_block_h: u16 = if debt_visible { 6 + narrative_rows } else { 0 };

        let logo_h = (CROSS.len()
            + 1
            + WORDMARK.len()
            + 1
            + 1
            + 1
            + 1
            + if startup_elapsed.is_some() { 2 } else { 0 }) as u16;

        let (logo_area, debt_area) = if debt_visible && area.height > logo_h + debt_block_h + 2 {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(area.height - debt_block_h),
                    Constraint::Length(debt_block_h),
                ])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // ── Logo block (unchanged) ────────────────────────────────────────────
        let logo_area_h = logo_area.height as usize;
        let left_pad = area_w.saturating_sub(LOGO_W) / 2;

        let cross_style = Style::default().fg(Color::Yellow);
        let word_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
        let dim_style = Style::default().fg(Color::DarkGray);

        let top_pad = logo_area_h.saturating_sub(logo_h as usize) / 2;
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

        frame.render_widget(Paragraph::new(lines), logo_area);

        // ── Debt dashboard ────────────────────────────────────────────────────
        if let (Some(da), Some(report)) = (debt_area, debt_report) {
            Self::render_debt_dashboard(frame, da, report, debt_narrative);
        }
    }

    /// Three-column debt dashboard rendered below the logo.
    fn render_debt_dashboard(
        frame: &mut Frame,
        area: Rect,
        report: &crate::debt::DebtReport,
        narrative: Option<&str>,
    ) {
        let dim = Style::default().fg(Color::DarkGray);
        let accent = Style::default().fg(Color::White);
        let warn = Style::default().fg(Color::Yellow);

        // Split vertically: separator line + columns + optional narrative
        let narrative_h: u16 = if narrative.is_some() { 3 } else { 0 };
        let col_h = area.height.saturating_sub(narrative_h);

        let (col_area, narrative_area) = if narrative.is_some() {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(col_h), Constraint::Length(narrative_h)])
                .split(area);
            (rows[0], Some(rows[1]))
        } else {
            (area, None)
        };

        // Three equal columns
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .split(col_area);

        // ── Intent column ─────────────────────────────────────────────────────
        let impl_pct = (report.intent.implementation_rate() * 100.0).round() as u32;
        let intent_title = Line::from(Span::styled("  intent debt", dim));
        let mut intent_lines = vec![
            intent_title,
            Line::from(Span::styled(
                format!("  {impl_pct}% of ADRs implemented"),
                if impl_pct >= 90 { accent } else { warn },
            )),
        ];
        if report.intent.accepted_pending > 0 {
            intent_lines.push(Line::from(Span::styled(
                format!("  {} accepted, unbuilt", report.intent.accepted_pending),
                dim,
            )));
        }
        if report.intent.stale_count > 0 {
            intent_lines.push(Line::from(Span::styled(
                format!("  {} stale (>18 mo)", report.intent.stale_count),
                dim,
            )));
        }
        if report.intent.accepted_pending == 0 && report.intent.stale_count == 0 {
            intent_lines.push(Line::from(Span::styled("  \u{2713} all decisions current", dim)));
        }
        intent_lines.push(Line::from(Span::styled(
            format!("  velocity: {} / 6 mo", report.intent.recent_velocity),
            dim,
        )));
        frame.render_widget(Paragraph::new(intent_lines).block(Block::default()), cols[0]);

        // ── Technical column ──────────────────────────────────────────────────
        let tech_title = Line::from(Span::styled("  technical debt", dim));
        let mut tech_lines = vec![tech_title];
        if report.technical.high_complexity_fns > 0 {
            tech_lines.push(Line::from(Span::styled(
                format!("  {} high-complexity fns", report.technical.high_complexity_fns),
                warn,
            )));
            if let Some(site) = report.technical.worst_complexity_sites.first() {
                let truncated = if site.len() > 28 { &site[..28] } else { site };
                tech_lines.push(Line::from(Span::styled(format!("  \u{21b3} {truncated}"), dim)));
            }
        } else {
            tech_lines.push(Line::from(Span::styled("  \u{2713} no complex fns", dim)));
        }
        let marker_sum = report.technical.todo_macros + report.technical.unwraps_outside_tests;
        if marker_sum > 0 {
            tech_lines.push(Line::from(Span::styled(
                format!("  {} todo!/unwrap() markers", marker_sum),
                dim,
            )));
        }
        if report.technical.dead_code_suppressed > 0 {
            tech_lines.push(Line::from(Span::styled(
                format!("  {} dead_code suppressed", report.technical.dead_code_suppressed),
                dim,
            )));
        }
        frame.render_widget(Paragraph::new(tech_lines).block(Block::default()), cols[1]);

        // ── Cognitive column ──────────────────────────────────────────────────
        let cog_title = Line::from(Span::styled("  cognitive debt", dim));
        let mut cog_lines = vec![cog_title];

        if report.cognitive.has_git_data {
            let pct = report.cognitive.active_surface_pct.round() as u32;
            cog_lines.push(Line::from(Span::styled(
                format!("  {pct}% active (last 30d)"),
                if pct >= 50 { accent } else { warn },
            )));
            if !report.cognitive.stale_modules.is_empty() {
                cog_lines.push(Line::from(Span::styled(
                    format!("  stale: {}", report.cognitive.stale_modules.join(", ")),
                    dim,
                )));
            }
            if report.cognitive.reentry_risk_count > 0 {
                cog_lines.push(Line::from(Span::styled(
                    format!("  {} re-entry risk fns", report.cognitive.reentry_risk_count),
                    warn,
                )));
            }
        } else {
            cog_lines.push(Line::from(Span::styled("  no git data", dim)));
        }

        if !report.cognitive.error_hotspots.is_empty() {
            cog_lines.push(Line::from(Span::styled(
                format!("  errors: {}", report.cognitive.error_hotspots.join(" ")),
                dim,
            )));
        } else if report.cognitive.has_session_data {
            cog_lines.push(Line::from(Span::styled("  \u{2713} no tool error spikes", dim)));
        }

        frame.render_widget(Paragraph::new(cog_lines).block(Block::default()), cols[2]);

        // ── Narrative ─────────────────────────────────────────────────────────
        if let (Some(na), Some(text)) = (narrative_area, narrative) {
            let wrap_width = na.width.saturating_sub(4) as usize;
            let wrapped = word_wrap(text, wrap_width.max(20));
            let narrative_lines: Vec<Line> = wrapped
                .into_iter()
                .take(narrative_h as usize)
                .map(|l| {
                    Line::from(Span::styled(
                        format!("  {l}"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                    ))
                })
                .collect();
            frame.render_widget(Paragraph::new(narrative_lines), na);
        }
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
            let mut col_budget = text_width;
            let mut skipped = 0usize;

            for span in spans {
                if col_budget == 0 {
                    break;
                }
                let span_len = span.content.chars().count();

                if skipped + span_len <= scroll_col {
                    // Entire span is before the viewport — skip without allocating.
                    skipped += span_len;
                    continue;
                }

                if skipped < scroll_col {
                    let skip_here = scroll_col - skipped;
                    let rest: String =
                        span.content.chars().skip(skip_here).take(col_budget).collect();
                    col_budget = col_budget.saturating_sub(rest.chars().count());
                    skipped = scroll_col;
                    if !rest.is_empty() {
                        out_spans.push(Span::styled(rest, span.style));
                    }
                } else {
                    let take: String = span.content.chars().take(col_budget).collect();
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

/// Wrap `text` to lines of at most `width` chars, splitting on whitespace.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}
