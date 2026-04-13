//! JSON → ratatui `Line` renderer.
//!
//! Parses and pretty-prints JSON, then applies token-level colour highlighting
//! to produce a `Vec<Line<'static>>` for the editor's JSON-preview mode.
//!
//! Colour scheme
//! -------------
//! Object keys   → bold blue
//! String values → green
//! Numbers       → yellow
//! true / false  → cyan
//! null          → red
//! Punctuation   → dim

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Render `content` (JSON text) into syntax-coloured ratatui [`Line`]s.
pub fn render(content: &str) -> Vec<Line<'static>> {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(value) => {
            let pretty =
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| content.to_string());
            highlight_json(&pretty)
        },
        Err(e) => {
            // Show error banner, then raw content.
            let mut lines: Vec<Line<'static>> = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("⚠ JSON parse error: {}", e),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            for raw_line in content.lines() {
                lines.push(Line::from(Span::raw(raw_line.to_string())));
            }
            lines
        },
    }
}

// ── Highlighter ───────────────────────────────────────────────────────────────

fn highlight_json(pretty: &str) -> Vec<Line<'static>> {
    pretty.lines().map(highlight_line).collect()
}

/// Highlight a single line of pretty-printed JSON.
///
/// We use a simple state machine rather than a full lexer — serde_json's
/// pretty-printer produces predictable output that we can parse reliably.
fn highlight_line(line: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let trimmed = line.trim_end();

    // Preserve leading whitespace unstyled.
    let indent_len = line.len() - line.trim_start().len();
    if indent_len > 0 {
        spans.push(Span::raw(line[..indent_len].to_string()));
    }

    let rest = &line[indent_len..];
    if rest.is_empty() {
        return Line::from(spans);
    }

    // Structural-only lines: `{`, `}`, `[`, `]`, `},`, `],`
    if matches!(rest, "{" | "}" | "}," | "[" | "]" | "],") {
        spans.push(Span::styled(rest.to_string(), Style::default().fg(Color::DarkGray)));
        return Line::from(spans);
    }

    // Key-value line: `"key": <value>` or `"key": {` / `"key": [`
    if rest.starts_with('"') {
        if let Some((_key_span, after_key)) = parse_quoted(rest) {
            // Check if this looks like a key (followed by `: `)
            if after_key.starts_with("\": ") || after_key.starts_with("\":") {
                // Render: key (bold blue) + colon+space (dim) + value
                // Find the closing quote of the key.
                if let Some(close) = closing_quote_pos(rest) {
                    let key_str = rest[..=close].to_string(); // `"key"`
                    let after = &rest[close + 1..]; // `: value...`
                    spans.push(Span::styled(
                        key_str,
                        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                    ));
                    if let Some(colon_idx) = after.find(':') {
                        let colon_and_space = &after[..colon_idx + 1];
                        let value_str = &after[colon_idx + 1..];
                        spans.push(Span::styled(
                            colon_and_space.to_string(),
                            Style::default().fg(Color::DarkGray),
                        ));
                        // value_str starts with a space then the value.
                        let (space, val) = split_leading_space(value_str);
                        if !space.is_empty() {
                            spans.push(Span::raw(space.to_string()));
                        }
                        append_value_spans(&mut spans, val, trimmed);
                        return Line::from(spans);
                    }
                }
            }
        }
    }

    // Bare value line (array element): number, bool, null, string, `{`, `[`
    append_value_spans(&mut spans, rest, trimmed);
    Line::from(spans)
}

/// Append styled span(s) for a JSON value token (the right-hand side).
/// `full_trimmed` is the full trimmed line for trailing-comma detection.
fn append_value_spans(spans: &mut Vec<Span<'static>>, val: &str, _full: &str) {
    // Strip trailing comma for type detection, keep it for display.
    let (core, trailer) =
        if let Some(stripped) = val.strip_suffix(',') { (stripped, ",") } else { (val, "") };

    let (style, text): (Style, String) = if core == "true" || core == "false" {
        (Style::default().fg(Color::Cyan), core.to_string())
    } else if core == "null" {
        (Style::default().fg(Color::Red), core.to_string())
    } else if core.starts_with('"') {
        (Style::default().fg(Color::Green), core.to_string())
    } else if is_number(core) {
        (Style::default().fg(Color::Yellow), core.to_string())
    } else {
        // structural (`{`, `[`, `}`, `]`, etc.)
        (Style::default().fg(Color::DarkGray), core.to_string())
    };

    spans.push(Span::styled(text, style));
    if !trailer.is_empty() {
        spans.push(Span::styled(trailer.to_string(), Style::default().fg(Color::DarkGray)));
    }
}

// ── Small helpers ─────────────────────────────────────────────────────────────

/// Find the position of the closing `"` in a JSON string starting at index 0,
/// accounting for `\"` escapes.
fn closing_quote_pos(s: &str) -> Option<usize> {
    if !s.starts_with('"') {
        return None;
    }
    let mut chars = s.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next(); // skip escaped char
        } else if c == '"' {
            return Some(i);
        }
    }
    None
}

/// Returns `(key_content, rest_after_closing_quote)` for a quoted string
/// starting at position 0.  Used only to check if a colon follows.
fn parse_quoted(s: &str) -> Option<(String, String)> {
    let close = closing_quote_pos(s)?;
    Some((s[1..close].to_string(), s[close..].to_string()))
}

fn split_leading_space(s: &str) -> (&str, &str) {
    let idx = s.find(|c: char| c != ' ').unwrap_or(s.len());
    (&s[..idx], &s[idx..])
}

fn is_number(s: &str) -> bool {
    let s = s.trim_start_matches('-');
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
}
