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
        // 2 borders + 14 (" Binary file '") + 29 ("'   [o] open   [Esc] dismiss ")
        let overhead: u16 = 45;
        let desired_width = (name.chars().count() as u16 + overhead).min(area.width);
        let popup_width = desired_width.max(overhead);
        let max_name = popup_width.saturating_sub(overhead) as usize;
        let name = trunc(&name, max_name);
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
    pub(super) fn render_commit_msg_popup(frame: &mut Frame, msg: &str, cursor: usize, area: Rect) {
        let popup_width = 100.min(area.width);
        let inner_width = popup_width.saturating_sub(2) as usize; // subtract borders
                                                                  // Height: 2 borders + hint line + content lines (min 6, max 20)
        let content_lines = msg.lines().count().clamp(6, 20) as u16;
        let popup_height = (content_lines + 3).min(area.height);
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let hint = Line::from(Span::styled(
            " Enter=commit   Esc=discard   ←→=move   (edit freely) ",
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

        // Position the terminal cursor so the user can see where they're editing.
        // Walk the text up to the cursor byte offset, tracking line/col.
        let mut cur_row: u16 = 0;
        let mut cur_col: usize = 0;
        let mut byte_pos: usize = 0;
        'outer: for line in msg.lines() {
            for (char_idx, ch) in line.char_indices() {
                if byte_pos == cursor {
                    cur_col = char_idx;
                    break 'outer;
                }
                byte_pos += ch.len_utf8();
            }
            if byte_pos == cursor {
                // Cursor is at end of this line
                cur_col = line.len();
                break;
            }
            // Account for the '\n' separator
            byte_pos += 1;
            if byte_pos > cursor {
                break;
            }
            cur_row += 1;
            // Wrap long lines
            cur_row += (line.chars().count() / inner_width.max(1)) as u16;
            cur_col = 0;
        }
        // +1 border, +1 hint line, +1 border top = row offset of 2; +1 border left + 1 space indent = col offset of 2
        let screen_col = x + 1 + 1 + (cur_col % inner_width.max(1)) as u16;
        let screen_row = y + 2 + cur_row + (cur_col / inner_width.max(1)) as u16;
        frame.set_cursor_position((screen_col, screen_row));
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

        // ── Companion / Nexus sidecar ─────────────────────────────────────────
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " Companion ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )]));
        {
            let (socket_bound, process_running, client_connected) = data.sidecar_status;
            let socket_icon = if socket_bound { "✓" } else { "✗" };
            let socket_color = if socket_bound { Color::Green } else { Color::DarkGray };
            lines.push(Line::from(vec![
                Span::styled(format!("  {socket_icon} "), Style::default().fg(socket_color)),
                Span::styled("Nexus socket  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if socket_bound { "bound" } else { "unbound" },
                    Style::default().fg(socket_color),
                ),
            ]));

            let proc_icon = if process_running { "✓" } else { "–" };
            let proc_color = if process_running { Color::Green } else { Color::DarkGray };
            lines.push(Line::from(vec![
                Span::styled(format!("  {proc_icon} "), Style::default().fg(proc_color)),
                Span::styled("Process       ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if process_running { "running" } else { "stopped" },
                    Style::default().fg(proc_color),
                ),
            ]));

            let conn_icon = if client_connected { "✓" } else { "–" };
            let conn_color = if client_connected { Color::Green } else { Color::DarkGray };
            lines.push(Line::from(vec![
                Span::styled(format!("  {conn_icon} "), Style::default().fg(conn_color)),
                Span::styled("Client        ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if client_connected { "connected" } else { "waiting" },
                    Style::default().fg(conn_color),
                ),
            ]));

            if process_running && !client_connected {
                lines.push(Line::from(vec![Span::styled(
                    "    companion launched but not yet connected to socket",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM),
                )]));
            }
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
            let mask = data.observation_mask_threshold_chars;
            let (mask_text, mask_color) = if mask == 0 {
                ("obs mask   disabled".to_string(), Color::Yellow)
            } else {
                (format!("obs mask   {mask} chars  (~{} t)", mask / 4), Color::DarkGray)
            };
            lines.push(Line::from(vec![Span::styled(
                format!("  {mask_text}"),
                Style::default().fg(mask_color),
            )]));
        }

        // ── Retrieval tool ratio ──────────────────────────────────────────────
        if let Some((reads, symbols, outlines)) = data.tool_retrieval_counts {
            let total_symbol = symbols + outlines;
            let ratio_text = if reads == 0 {
                "∞ (no read_file calls)".to_string()
            } else {
                format!("{:.1}x", total_symbol as f32 / reads as f32)
            };
            let ratio_color =
                if reads == 0 || total_symbol >= reads { Color::Green } else { Color::Yellow };
            lines.push(Line::from(vec![
                Span::styled("  reads    ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{reads} read_file"), Style::default().fg(Color::White)),
                Span::styled(
                    format!("  /  {symbols} get_symbol_context  /  {outlines} get_file_outline"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  ratio    ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("symbol:read_file = {ratio_text}"),
                    Style::default().fg(ratio_color),
                ),
            ]));
        }

        // ── Codified Context ──────────────────────────────────────────────────
        if let Some((ctokens, max_tokens, scount, kcount)) = data.codified_context_info {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                " Codified Context ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )]));
            let (tok_color, warn) = if ctokens > max_tokens {
                (Color::Red, "  !! exceeds max")
            } else if ctokens > max_tokens * 4 / 5 {
                (Color::Yellow, "  ! near limit")
            } else {
                (Color::Green, "")
            };
            lines.push(Line::from(vec![
                Span::styled("  constitution  ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{ctokens} t"), Style::default().fg(tok_color)),
                Span::styled(
                    format!("  / {max_tokens} t cap{warn}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  specialists   ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{scount} loaded"), Style::default().fg(Color::White)),
                Span::styled("  knowledge  ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{kcount} docs"), Style::default().fg(Color::White)),
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

    // ── Inline assistant overlay (ADR 0111) ───────────────────────────────────

    /// Render the inline AI assist overlay (Mode::InlineAssist).
    ///
    /// - Input phase:     shows a prompt bar at the bottom of the screen.
    /// - Generating phase: shows the same bar with a "generating…" indicator
    ///   and the accumulating response.
    /// - Preview phase:   shows the full response with accept/reject hints.
    pub(super) fn render_inline_assist_overlay(
        frame: &mut Frame,
        view: &super::InlineAssistView<'_>,
        area: Rect,
    ) {
        use crate::editor::InlineAssistPhase;

        let popup_width = area.width.saturating_sub(4).max(20);

        match view.phase {
            InlineAssistPhase::Input => {
                // Single-row prompt bar pinned to bottom of the buffer area.
                let popup_height = 3u16;
                let x = (area.width.saturating_sub(popup_width)) / 2;
                let y = area.height.saturating_sub(popup_height + 1);
                let popup_area = Rect::new(x, y, popup_width, popup_height);

                frame.render_widget(Clear, popup_area);

                let display = format!(" > {}_", view.prompt);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::LightCyan))
                    .title(Span::styled(
                        " Inline AI  Enter=submit  Esc=cancel ",
                        Style::default().fg(Color::LightCyan),
                    ));
                frame.render_widget(Paragraph::new(display).block(block), popup_area);
            },

            InlineAssistPhase::Generating => {
                let response_lines: Vec<&str> = view.response.lines().collect();
                let visible_lines = response_lines.len().min(8);
                let popup_height = (visible_lines as u16 + 4).min(area.height);
                let x = (area.width.saturating_sub(popup_width)) / 2;
                let y = area.height.saturating_sub(popup_height + 1);
                let popup_area = Rect::new(x, y, popup_width, popup_height);

                frame.render_widget(Clear, popup_area);

                let hint = Line::from(vec![
                    Span::styled(" generating…  ", Style::default().fg(Color::Yellow)),
                    Span::styled("Esc=cancel", Style::default().fg(Color::DarkGray)),
                ]);

                let start = response_lines.len().saturating_sub(visible_lines);
                let mut body: Vec<Line<'static>> = response_lines[start..]
                    .iter()
                    .map(|l| {
                        Line::from(Span::styled(
                            format!(" {l}"),
                            Style::default().fg(Color::DarkGray),
                        ))
                    })
                    .collect();
                body.insert(0, hint);

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow))
                    .title(Span::styled(
                        " Inline AI ",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ));
                frame.render_widget(Paragraph::new(body).block(block), popup_area);
            },

            InlineAssistPhase::Preview => {
                let response_lines: Vec<&str> = view.response.lines().collect();
                let visible_lines = response_lines.len().min(16);
                let popup_height = (visible_lines as u16 + 4).min(area.height);
                let x = (area.width.saturating_sub(popup_width)) / 2;
                let y = area.height.saturating_sub(popup_height + 1);
                let popup_area = Rect::new(x, y, popup_width, popup_height);

                frame.render_widget(Clear, popup_area);

                let hint = Line::from(vec![
                    Span::styled(" Enter", Style::default().fg(Color::Green)),
                    Span::styled("=accept  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Esc", Style::default().fg(Color::Red)),
                    Span::styled("=cancel ", Style::default().fg(Color::DarkGray)),
                ]);

                let start = response_lines.len().saturating_sub(visible_lines);
                let mut body: Vec<Line<'static>> = response_lines[start..]
                    .iter()
                    .map(|l| {
                        Line::from(Span::styled(format!(" {l}"), Style::default().fg(Color::White)))
                    })
                    .collect();
                body.insert(0, hint);

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green))
                    .title(Span::styled(
                        " Inline AI  ready ",
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    ));
                frame.render_widget(Paragraph::new(body).block(block), popup_area);
            },
        }
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

    // ── Review changes overlay (Mode::ReviewChanges, ADR 0113) ───────────────

    /// Full-screen overlay listing every file the agent touched this session.
    /// Each file shows a compact unified diff (3-line context) with per-file
    /// accept / reject controls.
    pub(super) fn render_review_changes_overlay(
        frame: &mut Frame,
        state: &crate::editor::ReviewChangesState,
        area: Rect,
    ) {
        use crate::editor::{DiffLine, Verdict};

        frame.render_widget(Clear, area);

        let total = state.diffs.len();
        let focused = state.focused_file;

        // ── Build flat rendered line list ──────────────────────────────────────
        // Each entry is (text, style).  We collect into an owned Vec so we can
        // slice it for scrolling without lifetime issues.
        let mut lines: Vec<(String, Style)> = Vec::new();

        for (fi, file_diff) in state.diffs.iter().enumerate() {
            // File header
            let file_v = file_diff.file_verdict();
            let (verdict_tag, verdict_color) = match file_v {
                Verdict::Pending => ("pending", Color::Yellow),
                Verdict::Accepted => ("accepted", Color::Green),
                Verdict::Rejected => ("rejected", Color::Red),
            };
            let is_focused = fi == focused;
            let header_bg = if is_focused { Color::DarkGray } else { Color::Reset };
            let header_text = format!(" ── {} [{}] ", file_diff.rel_path, verdict_tag);
            let header_style =
                Style::default().fg(verdict_color).bg(header_bg).add_modifier(Modifier::BOLD);
            lines.push((header_text, header_style));

            if file_diff.lines.is_empty() {
                lines.push(("  (no changes)".to_string(), Style::default().fg(Color::DarkGray)));
            } else {
                let hunk_count = file_diff.hunk_verdicts.len();
                for dl in &file_diff.lines {
                    let (text, style) = match dl {
                        DiffLine::HunkStart(idx) => {
                            let hv = file_diff
                                .hunk_verdicts
                                .get(*idx)
                                .copied()
                                .unwrap_or(Verdict::Pending);
                            let (htag, hcolor) = match hv {
                                Verdict::Pending => ("pending", Color::Yellow),
                                Verdict::Accepted => ("accepted", Color::Green),
                                Verdict::Rejected => ("rejected", Color::Red),
                            };
                            let is_focused_hunk = fi == focused && state.focused_hunk == Some(*idx);
                            let hunk_bg =
                                if is_focused_hunk { Color::DarkGray } else { Color::Reset };
                            let t = if hunk_count > 1 {
                                format!("  ··· hunk {}/{} [{}] ···", idx + 1, hunk_count, htag)
                            } else {
                                format!("  ··· [{}] ···", htag)
                            };
                            (t, Style::default().fg(hcolor).bg(hunk_bg))
                        },
                        DiffLine::Added(s) => {
                            (format!("  + {s}"), Style::default().fg(Color::LightGreen))
                        },
                        DiffLine::Removed(s) => {
                            (format!("  - {s}"), Style::default().fg(Color::Red))
                        },
                        DiffLine::Context(s) => {
                            (format!("    {s}"), Style::default().fg(Color::DarkGray))
                        },
                    };
                    lines.push((text, style));
                }
            }

            // Blank spacer between files
            if fi + 1 < total {
                lines.push((String::new(), Style::default()));
            }
        }

        // ── Layout: 3-row header + scrollable body ─────────────────────────────
        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);

        // Header block
        let hunk_hint = if state.focused_hunk.is_some() {
            "  tab/T=hunk nav  a=accept hunk  r=reject hunk"
        } else {
            "  tab=focus hunk"
        };
        let hint = format!(
            " Review Changes  ({}/{})  y/n=file  Y/N=all  [/]=jump{}  q=quit",
            focused + 1,
            total,
            hunk_hint,
        );
        let header_block =
            Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::White)).block(header_block),
            header_area,
        );

        // Body: apply scroll, render visible rows
        let viewport = body_area.height as usize;
        let total_lines = lines.len();
        let scroll = state.scroll.min(total_lines.saturating_sub(1));
        let visible: Vec<Line<'static>> = lines
            .into_iter()
            .skip(scroll)
            .take(viewport)
            .map(|(text, style)| Line::styled(text, style))
            .collect();

        // Scroll indicator in the bottom-right corner when content overflows
        frame.render_widget(Paragraph::new(visible), body_area);
        if total_lines > viewport {
            let indicator = format!(" {}/{} ", scroll + 1, total_lines);
            let ind_width = indicator.len() as u16;
            let ind_area = Rect {
                x: body_area.right().saturating_sub(ind_width + 1),
                y: body_area.bottom().saturating_sub(1),
                width: ind_width,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(indicator).style(Style::default().fg(Color::DarkGray)),
                ind_area,
            );
        }
    }
}
