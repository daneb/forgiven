use super::markdown::render_message_content;
use super::*;

impl UI {
    /// Render the Copilot Chat / agent panel on the right side.
    pub(super) fn render_agent_panel(
        frame: &mut Frame,
        panel: &AgentPanel,
        mode: Mode,
        area: Rect,
    ) {
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
        // Each pasted/image/file block adds one summary line.
        let paste_summary_lines = panel.pasted_blocks.len() as u16;
        let image_summary_lines = panel.image_blocks.len() as u16;
        let file_summary_lines = panel.file_blocks.len() as u16;
        let input_height =
            input_text_lines + paste_summary_lines + image_summary_lines + file_summary_lines + 2;

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
                        Role::User => (
                            format!("{} You", panel.provider.user_emoji()),
                            Color::Green,
                        ),
                        Role::Assistant => {
                            let name = panel.ai_label_name();
                            let emoji = panel.provider.ai_emoji();
                            let color = match panel.provider {
                                ProviderKind::Copilot => Color::Cyan,
                                ProviderKind::Ollama => Color::Magenta,
                            };
                            (format!("{emoji} {name}"), color)
                        },
                        Role::System => unreachable!(),
                    };
                    ml.push(Line::from(vec![Span::styled(
                        format!("╔ {label} "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )]));
                    ml.extend(render_message_content(&msg.content, content_width));
                    // Render image attachment placeholders.
                    if !msg.images.is_empty() {
                        let img_style =
                            Style::default().fg(Color::Magenta).add_modifier(Modifier::DIM);
                        for (w, h) in &msg.images {
                            ml.push(Line::from(Span::styled(
                                format!("  Image ({w}x{h}) [attached]"),
                                img_style,
                            )));
                        }
                    }
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
                    // Provider-aware streaming header:
                    //   Copilot → "╔ 🤖 Copilot ▋"  (cyan)
                    //   Ollama  → "╔ 🦙 qwen2.5-coder ▋"  (magenta, model name)
                    let stream_label =
                        format!("╔ {} {} ", panel.provider.ai_emoji(), panel.ai_label_name());
                    let stream_color = match panel.provider {
                        ProviderKind::Copilot => Color::Cyan,
                        ProviderKind::Ollama => Color::Magenta,
                    };
                    let mut sl: Vec<Line<'static>> = vec![Line::from(vec![
                        Span::styled(
                            stream_label,
                            Style::default().fg(stream_color).add_modifier(Modifier::BOLD),
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
            let base = format!("  {k_used:.1}k/{k_total:.0}k");
            let label = if panel.last_cached_tokens > 0 {
                let k_cached = panel.last_cached_tokens as f32 / 1000.0;
                format!("{base} ({k_cached:.1}k cached)")
            } else {
                base
            };
            Span::styled(label, Style::default().fg(color))
        } else {
            Span::raw("")
        };

        let panel_title =
            format!(" {} [{model_label}]", panel.provider.display_name());
        let title_line = Line::from(vec![
            Span::raw(panel_title),
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
            " Ask Copilot… (Enter=send, Alt+Enter=newline, Ctrl+V=paste image, Ctrl+P=attach file, Ctrl+T=model)"
                .to_string()
        } else if panel.has_code_to_apply()
            && panel.input.is_empty()
            && panel.pasted_blocks.is_empty()
            && panel.image_blocks.is_empty()
            && panel.file_blocks.is_empty()
        {
            " Message Copilot… | [a] diff+apply  Ctrl+P=attach  Ctrl+T=model ".to_string()
        } else {
            " Message Copilot… (Ctrl+T=model) ".to_string()
        };
        let hint = hint.as_str();
        let input_block =
            Block::default().title(hint).borders(Borders::ALL).border_style(border_style);

        // Build input content: file block badges (green), image badges (magenta),
        // pasted block badges (cyan), then the typed text.
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
        let image_style = Style::default().fg(Color::Magenta).add_modifier(Modifier::DIM);
        input_lines.extend(panel.image_blocks.iter().map(|img| {
            let label = format!("  Image ({}x{})", img.width, img.height);
            Line::from(Span::styled(label, image_style))
        }));
        let paste_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM);
        input_lines.extend(panel.pasted_blocks.iter().map(|(_, n)| {
            let label = format!("⎘  Pasted {} line{}", n, if *n == 1 { "" } else { "s" });
            Line::from(Span::styled(label, paste_style))
        }));
        let typed = if focused { format!("{}_", panel.input) } else { panel.input.clone() };
        for line in typed.split('\n') {
            input_lines.push(Line::from(line.to_string()));
        }
        // When the input content exceeds the visible area, scroll so the cursor
        // (last line) stays visible.  The badges (file/image/paste) count as
        // lines above the typed text; the box interior is input_height − 2.
        let badge_lines = (paste_summary_lines + image_summary_lines + file_summary_lines) as usize;
        let total_content_lines = badge_lines + total_wrapped;
        let visible_lines = input_height.saturating_sub(2) as usize; // interior rows
        let input_scroll = if total_content_lines > visible_lines {
            (total_content_lines - visible_lines) as u16
        } else {
            0
        };
        let input_para = Paragraph::new(input_lines)
            .block(input_block)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false })
            .scroll((input_scroll, 0));
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
    pub(super) fn render_slash_menu(frame: &mut Frame, menu: &SlashMenuState, input_area: Rect) {
        if menu.items.is_empty() {
            return;
        }

        let n = menu.items.len() as u16;
        let has_desc = menu.description.is_some();
        // Width matches the input box; height = items + 2 borders (+ 2 for hint line) capped.
        let popup_width = input_area.width;
        let list_rows = n.min(10);
        let popup_height = list_rows + 2 + if has_desc { 2 } else { 0 };

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

        // Split inner area: list rows at the top, optional hint at the bottom.
        let (list_area, hint_area) = if has_desc && inner.height >= 2 {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(inner.height.saturating_sub(2)),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(inner);
            (chunks[0], Some((chunks[1], chunks[2])))
        } else {
            (inner, None)
        };

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
        let visible_rows = list_area.height as usize;
        let scroll = if menu.selected >= visible_rows {
            (menu.selected + 1).saturating_sub(visible_rows) as u16
        } else {
            0
        };

        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), list_area);

        // Hint line: separator + description of the selected command.
        if let (Some(desc), Some((sep_area, desc_area))) = (&menu.description, hint_area) {
            let sep = "─".repeat(sep_area.width as usize);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(sep, Style::default().fg(Color::DarkGray)))),
                sep_area,
            );
            let truncated = if desc.len() > desc_area.width.saturating_sub(2) as usize {
                format!(" {}…", &desc[..desc_area.width.saturating_sub(3) as usize])
            } else {
                format!(" {desc}")
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    truncated,
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC),
                ))),
                desc_area,
            );
        }
    }

    /// Render the Ctrl+P file-context picker overlay above the agent input box.
    ///
    /// `file_blocks` is the list of currently attached files so the picker can
    /// show a ✓ indicator and let the user toggle files off as well as on.
    pub(super) fn render_at_picker(
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
    pub(super) fn render_continuation_dialog(
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
    /// Supports newlines in the question text and scrolls if the content is
    /// taller than the available area.
    pub(super) fn render_ask_user_dialog(frame: &mut Frame, state: &AskUserState, area: Rect) {
        // Use 92% of the panel width.
        let dialog_width = ((area.width * 92) / 100).max(20);
        let inner_w = dialog_width.saturating_sub(2) as usize;

        // ── Build question lines respecting newlines + word wrap ────────────
        let q_style = Style::default().fg(Color::White);
        let mut q_display_rows: u16 = 0;
        let q_lines: Vec<Line> = state
            .question
            .lines()
            .flat_map(|raw_line| {
                let trimmed = raw_line.trim();
                if trimmed.is_empty() {
                    q_display_rows += 1;
                    return vec![Line::from("")];
                }
                // Estimate how many display rows this logical line occupies
                // after word-wrap (character count / inner width, rounded up).
                let wrapped = if inner_w == 0 {
                    1u16
                } else {
                    (trimmed.chars().count() as u16).div_ceil(inner_w as u16).max(1)
                };
                q_display_rows += wrapped;
                vec![Line::from(Span::styled(trimmed.to_string(), q_style))]
            })
            .collect();

        // ── Build option + hint lines ──────────────────────────────────────
        let mut opt_lines: Vec<Line> = vec![Line::from("")]; // blank separator
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

        let opts_rows = opt_lines.len() as u16;
        // Total content height: question display rows + options/hint.
        let content_height = q_display_rows + opts_rows;
        // 2 for borders, clamped to available panel area.
        let dialog_height = (content_height + 2).min(area.height.saturating_sub(2));

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

        // ── Combine question + options into a single scrollable paragraph ──
        let mut all_lines = q_lines;
        all_lines.extend(opt_lines);

        // Scroll: if content overflows, scroll so the options section is
        // always visible at the bottom (question text scrolls off the top).
        let scroll_row = content_height.saturating_sub(inner.height);

        let para = Paragraph::new(all_lines).wrap(Wrap { trim: false }).scroll((scroll_row, 0));
        frame.render_widget(para, inner);
    }

    /// Render the file explorer tree on the left side.
    pub(super) fn render_file_explorer(
        frame: &mut Frame,
        explorer: &FileExplorer,
        mode: Mode,
        area: Rect,
    ) {
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
    pub(super) fn render_task_strip(
        frame: &mut Frame,
        tasks: &[AgentTask],
        border_style: Style,
        area: Rect,
    ) {
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
}
