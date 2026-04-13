use super::*;

impl UI {
    /// Render the status line
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_status_line(
        frame: &mut Frame,
        buffer_data: Option<&BufferData>,
        mode: Mode,
        status_message: Option<&str>,
        command_buffer: Option<&str>,
        in_file_search_query: Option<&str>,
        key_sequence: &str,
        area: Rect,
        diagnostics: &[Diagnostic],
        // Context window usage % from last agent invocation (0–100); None before first submit.
        agent_fuel: Option<u32>,
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
            Mode::CsvPreview => "CSV",
            Mode::JsonPreview => "JSON",
            Mode::Search => "SEARCH",
            Mode::InFileSearch => "SEARCH",
            Mode::RenameFile => "RENAME",
            Mode::DeleteFile => "DELETE",
            Mode::NewFolder => "MKDIR",
            Mode::CommitMsg => "COMMIT",
            Mode::ReleaseNotes => "RELEASE",
            Mode::Diagnostics => "DIAG",
            Mode::BinaryFile => "BINARY",
            Mode::LocationList => "LSP",
            Mode::LspHover => "HOVER",
            Mode::LspRename => "RENAME",
            Mode::InlineAssist => "INLINE AI",
            Mode::ReviewChanges => "REVIEW",
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
            Mode::CsvPreview => Color::LightGreen,
            Mode::JsonPreview => Color::LightYellow,
            Mode::Search => Color::LightRed,
            Mode::InFileSearch => Color::LightRed,
            Mode::RenameFile => Color::Yellow,
            Mode::DeleteFile => Color::Red,
            Mode::NewFolder => Color::LightGreen,
            Mode::CommitMsg => Color::LightYellow,
            Mode::ReleaseNotes => Color::LightCyan,
            Mode::Diagnostics => Color::LightCyan,
            Mode::BinaryFile => Color::Yellow,
            Mode::LocationList => Color::LightCyan,
            Mode::LspHover => Color::LightYellow,
            Mode::LspRename => Color::LightGreen,
            Mode::InlineAssist => Color::LightCyan,
            Mode::ReviewChanges => Color::LightGreen,
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

        // Status message or command buffer or in-file search query
        if let Some(cmd) = command_buffer {
            spans = vec![Span::raw(format!(":{}", cmd))];
        } else if let Some(query) = in_file_search_query {
            spans = vec![Span::raw(format!("/{}", query))];
        } else if let Some(msg) = status_message {
            // Show status message on the right
            let msg_span = Span::styled(msg, Style::default().fg(Color::Yellow));
            spans.push(Span::raw(" "));
            spans.push(msg_span);
        }

        // Fuel gauge: only when not showing a command/search prompt and data is available.
        if command_buffer.is_none() && in_file_search_query.is_none() {
            if let Some(pct) = agent_fuel {
                let filled = (pct as usize * 6 / 100).min(6);
                let bar: String = "█".repeat(filled) + &"░".repeat(6_usize.saturating_sub(filled));
                let color = if pct >= 80 {
                    Color::Red
                } else if pct >= 50 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                spans.push(Span::raw("  "));
                spans.push(Span::styled(format!("[{bar} {pct}%]"), Style::default().fg(color)));
            }
        }

        let status_line = Line::from(spans);
        let paragraph = Paragraph::new(status_line).style(Style::default().bg(Color::Black));

        frame.render_widget(paragraph, area);
    }
}
