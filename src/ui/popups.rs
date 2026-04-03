use super::*;

/// Truncate `s` to `max` chars, appending `…` if cut.
fn trunc(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

impl UI {
    /// Render the centred delete confirmation popup (Mode::DeleteFile).
    pub(super) fn render_delete_popup(frame: &mut Frame, name: &str, area: Rect) {
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

    /// Render the centred binary-file popup (Mode::BinaryFile).
    pub(super) fn render_binary_file_popup(frame: &mut Frame, path: &std::path::Path, area: Rect) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let popup_width = 64.min(area.width);
        let popup_height = 3u16;
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let display = format!(" Binary file '{name}'   [o] open   [Esc] dismiss ");
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" Unsupported file ");
        frame.render_widget(Paragraph::new(display).block(block), popup_area);
    }

    /// Render the centred new-folder popup (Mode::NewFolder).
    pub(super) fn render_new_folder_popup(frame: &mut Frame, folder_buffer: &str, area: Rect) {
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
    pub(super) fn render_commit_msg_popup(frame: &mut Frame, msg: &str, area: Rect) {
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
    pub(super) fn render_release_notes_popup(
        frame: &mut Frame,
        view: &ReleaseNotesView<'_>,
        area: Rect,
    ) {
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
    pub(super) fn render_rename_popup(frame: &mut Frame, rename_buffer: &str, area: Rect) {
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
    pub(super) fn render_apply_diff_overlay(
        frame: &mut Frame,
        view: &ApplyDiffView<'_>,
        area: Rect,
    ) {
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
    pub(super) fn render_diagnostics_overlay(
        frame: &mut Frame,
        data: &DiagnosticsData<'_>,
        area: Rect,
    ) {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // ── Version ───────────────────────────────────────────────────────────
        lines.push(Line::from(vec![
            Span::styled(
                "  forgiven ",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("v{}", data.version),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

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

        // ── Agent session token usage ─────────────────────────────────────────
        if let Some((prompt_total, completion_total, window, rounds)) = data.agent_session_tokens {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                " Agent Session ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )]));
            let last_pct = prompt_total * 100 / window.max(1);
            let avg_prompt = prompt_total / rounds.max(1);
            let avg_pct = avg_prompt * 100 / window.max(1);
            let gauge_color = if avg_pct >= 80 {
                Color::Red
            } else if avg_pct >= 50 {
                Color::Yellow
            } else {
                Color::Green
            };
            lines.push(Line::from(vec![
                Span::styled("  invocations   ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{rounds}"), Style::default().fg(Color::White)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  avg prompt    ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{avg_prompt}t"),
                    Style::default().fg(gauge_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ({avg_pct}% of {window}t window)"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  session total ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{prompt_total}t prompt"),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  ({last_pct}% cumulative re-send)"),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  completion    ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{completion_total}t"), Style::default().fg(Color::White)),
            ]));
        }

        // ── Context Breakdown (last invocation) ───────────────────────────────
        if let Some(bd) = data.agent_ctx_breakdown {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                " Context Breakdown ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )]));
            let w = bd.ctx_window.max(1);
            let rows: [(&str, u32); 4] = [
                ("sys rules", bd.sys_rules_t),
                ("open file", bd.ctx_file_t),
                ("history  ", bd.history_t),
                ("user msg ", bd.user_msg_t),
            ];
            for (label, t) in rows {
                let pct = t * 100 / w;
                let filled = (pct as usize * 8 / 100).min(8);
                let bar: String = "█".repeat(filled) + &"░".repeat(8_usize.saturating_sub(filled));
                let color = if pct >= 80 {
                    Color::Red
                } else if pct >= 40 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {label}  "), Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{t:>6}t  "), Style::default().fg(Color::White)),
                    Span::styled(bar, Style::default().fg(color)),
                    Span::styled(format!("  {pct:>3}%"), Style::default().fg(Color::DarkGray)),
                ]));
            }
            let total = bd.total();
            let used_pct = bd.used_pct();
            let total_color = if used_pct >= 80 {
                Color::Red
            } else if used_pct >= 50 {
                Color::Yellow
            } else {
                Color::Green
            };
            lines.push(Line::from(vec![Span::styled(
                "  ──────────────────────────────────────",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
            )]));
            lines.push(Line::from(vec![
                Span::styled("  total    ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{total:>6}t"),
                    Style::default().fg(total_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  of {w}t  ({used_pct}%)"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }

        // ── MCP Activity ──────────────────────────────────────────────────────
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " MCP Activity ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )]));

        if data.mcp_call_log.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  no tool calls this session",
                Style::default().fg(Color::DarkGray),
            )]));
        } else {
            // Show last 5 calls, one line each.
            let start = data.mcp_call_log.len().saturating_sub(5);
            for record in &data.mcp_call_log[start..] {
                let (icon, icon_color) =
                    if record.is_error { ("  ✗ ", Color::Red) } else { ("  ✓ ", Color::Green) };
                let dur = if record.duration_ms < 1000 {
                    format!("{}ms", record.duration_ms)
                } else {
                    format!("{:.1}s", record.duration_ms as f64 / 1000.0)
                };
                let result_color = if record.is_error { Color::Red } else { Color::DarkGray };
                // Truncate args + result to keep the line short.
                let args = trunc(&record.args_summary, 22);
                let result = trunc(&record.result_summary, 22);
                lines.push(Line::from(vec![
                    Span::styled(icon, Style::default().fg(icon_color)),
                    Span::styled(
                        record.tool_name.clone(),
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {args} → {result}"),
                        Style::default().fg(result_color).add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        format!("  {dur}"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                    ),
                ]));
            }
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

        for (level, msg) in data.recent_logs.iter().rev().take(5).rev() {
            let (prefix, color) = match level.as_str() {
                "ERROR" => ("  ERROR ", Color::Red),
                "WARN" => ("  WARN  ", Color::Yellow),
                _ => ("  INFO  ", Color::DarkGray),
            };
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(trunc(msg, 46), Style::default().fg(Color::White)),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " press any key to close ",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        )]));

        // Centre the popup.
        let popup_width = 64.min(area.width);
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
    pub(super) fn format_file_size(bytes: u64) -> String {
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
    pub(super) fn format_system_time(t: std::time::SystemTime) -> String {
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
    pub(super) fn render_file_info_popup(
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
