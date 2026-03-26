use super::*;

/// Render an assistant message with `<think>` blocks styled as plain dim text
/// and everything else rendered as formatted markdown.
///
/// Thinking blocks get a `◌ thinking` header and word-wrapped dim-gray text;
/// the actual reply beneath is passed unchanged through the markdown renderer.
pub(super) fn render_message_content(content: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    for segment in split_thinking(content) {
        match segment {
            ContentSegment::Thinking(text) => {
                // Header line.
                lines.push(Line::from(vec![Span::styled(
                    "◌ thinking",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )]));
                // Plain word-wrap — no markdown parsing inside thinking blocks.
                for paragraph in text.split('\n') {
                    if paragraph.trim().is_empty() {
                        lines.push(Line::from(""));
                        continue;
                    }
                    let mut col = 0usize;
                    let mut current = String::new();
                    for word in paragraph.split_whitespace() {
                        let wlen = word.chars().count();
                        if col > 0 && width > 0 && col + 1 + wlen > width {
                            lines.push(Line::from(vec![Span::styled(
                                current.clone(),
                                Style::default().fg(Color::DarkGray),
                            )]));
                            current = word.to_owned();
                            col = wlen;
                        } else {
                            if col > 0 {
                                current.push(' ');
                                col += 1;
                            }
                            current.push_str(word);
                            col += wlen;
                        }
                    }
                    if !current.is_empty() {
                        lines.push(Line::from(vec![Span::styled(
                            current,
                            Style::default().fg(Color::DarkGray),
                        )]));
                    }
                }
                // Spacer between thinking block and the answer.
                lines.push(Line::from(""));
            },
            ContentSegment::Normal(text) => {
                lines.extend(crate::markdown::render(&text, width));
            },
        }
    }

    lines
}
