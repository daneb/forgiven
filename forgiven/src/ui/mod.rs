use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::path::PathBuf;

use crate::buffer::{Cursor, Selection};
use crate::keymap::Mode;

// Buffer data tuple: (name, is_modified, cursor, scroll_row, scroll_col, lines, selection)
type BufferData = (String, bool, Cursor, usize, usize, Vec<String>, Option<Selection>);
// Buffer list tuple: (buffer names with modified flags, selected index)
type BufferList = (Vec<(String, bool)>, usize);
// File list tuple: (file paths, selected index)
type FileList = (Vec<PathBuf>, usize);

/// UI rendering for the editor
pub struct UI;

impl UI {
    /// Render the entire UI
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

        // Calculate layout constraints based on which-key visibility
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
            .split(size);

        let main_area = chunks[0];
        let status_area = if which_key_options.is_some() {
            let which_key_area = chunks[1];
            Self::render_which_key(frame, which_key_options.unwrap(), which_key_area);
            chunks[2]
        } else {
            chunks[1]
        };

        // Render buffer content
        Self::render_buffer(frame, buffer_data, mode, main_area);

        // Render status line
        Self::render_status_line(frame, buffer_data, mode, status_message, command_buffer, key_sequence, status_area);
    }

    /// Render the buffer content
    fn render_buffer(frame: &mut Frame, buffer_data: Option<&BufferData>, mode: Mode, area: Rect) {
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
                    let line = Self::render_line(line_text, *scroll_col, viewport_width, row, selection, *scroll_row);
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

            // Render cursor (only in Normal, Insert modes)
            if mode != Mode::PickBuffer {
                let cursor_row = cursor.row.saturating_sub(*scroll_row);
                let cursor_col = cursor.col.saturating_sub(*scroll_col);

                if cursor_row < viewport_height && cursor_col < viewport_width {
                    frame.set_cursor_position((
                        area.x + cursor_col as u16,
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

    /// Render a single line with optional selection highlighting
    fn render_line(
        line_text: &str,
        scroll_col: usize,
        viewport_width: usize,
        row: usize,
        selection: &Option<Selection>,
        _scroll_row: usize,
    ) -> Line<'static> {
        let chars: Vec<char> = line_text.chars().collect();

        // If there's a selection, highlight the selected portion
        if let Some(sel) = selection {
            let (start, end) = sel.normalized();
            
            let mut spans = Vec::new();
            for (col_idx, ch) in chars.iter().enumerate() {
                if col_idx < scroll_col {
                    continue;
                }
                if col_idx >= scroll_col + viewport_width {
                    break;
                }

                // Check if this character is in the selection
                let is_selected = if start.row == end.row && row == start.row {
                    col_idx >= start.col && col_idx < end.col
                } else if row == start.row {
                    col_idx >= start.col
                } else if row == end.row {
                    col_idx < end.col
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

            Line::from(spans)
        } else {
            // No selection, just render normally
            let visible_text: String = chars
                .iter()
                .skip(scroll_col)
                .take(viewport_width)
                .collect();
            Line::from(visible_text)
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
        if let Some((files, selected_idx)) = file_list {
            let mut lines = vec![Line::from(Span::styled(
                "Find File:",
                Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow),
            ))];
            lines.push(Line::from(""));

            // Get current directory for relative path display
            let current_dir = std::env::current_dir().unwrap_or_default();

            for (idx, path) in files.iter().enumerate() {
                // Display relative path if possible, otherwise full path
                let display_path = path.strip_prefix(&current_dir)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                let style = if idx == *selected_idx {
                    Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                lines.push(Line::from(Span::styled(
                    format!("  {}", display_path),
                    style,
                )));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "↑/↓ or j/k to navigate, Enter to open, Esc to cancel",
                Style::default().fg(Color::Gray),
            )));

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightCyan))
                .title(" File Finder ");

            // Center the picker
            let picker_width = 80.min(area.width);
            let picker_height = (files.len().min(30) + 6).min(area.height as usize);
            
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

    /// Render the status line
    fn render_status_line(
        frame: &mut Frame,
        buffer_data: Option<&BufferData>,
        mode: Mode,
        status_message: Option<&str>,
        command_buffer: Option<&str>,
        key_sequence: &str,
        area: Rect,
    ) {
        let mode_str = match mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Visual => "VISUAL",
            Mode::PickBuffer => "PICK",
            Mode::PickFile => "FIND",
        };

        let mode_color = match mode {
            Mode::Normal => Color::Blue,
            Mode::Insert => Color::Green,
            Mode::Command => Color::Yellow,
            Mode::Visual => Color::Magenta,
            Mode::PickBuffer => Color::Cyan,
            Mode::PickFile => Color::LightCyan,
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
