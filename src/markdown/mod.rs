//! Markdown → ratatui `Line` renderer.
//!
//! Converts a CommonMark string into a `Vec<Line<'static>>` ready for use in
//! ratatui `Paragraph` widgets.  Used by the agent-panel chat history and the
//! editor's markdown-preview mode.
//!
//! Supported elements
//! ------------------
//! Block:  headings (H1–H6), paragraphs, fenced / indented code blocks,
//!         unordered + ordered lists (nested), block-quotes, horizontal rules.
//! Inline: **bold**, *italic*, `inline code`, soft/hard breaks.
//! Extra:  tool-call lines starting with ⚙ are rendered dim (agent panel
//!         convention) and are not re-wrapped.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

// ── String helpers ────────────────────────────────────────────────────────────

/// Truncate `s` to at most `max_chars` Unicode scalar values, appending `…`
/// when truncation occurs.
fn truncate_str(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars || max_chars == 0 {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated)
}
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Render `content` (CommonMark markdown) into ratatui [`Line`]s that fit
/// within `width` terminal columns.
pub fn render(content: &str, width: usize) -> Vec<Line<'static>> {
    Renderer::new(width).process(content)
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Left margin for body text and headings.
const MARGIN: &str = "    ";

// ── Renderer ──────────────────────────────────────────────────────────────────

struct Renderer {
    width: usize,
    output: Vec<Line<'static>>,

    // ── Inline state ─────────────────────────────────────────────────────────
    bold: bool,
    italic: bool,
    /// Accumulated inline spans for the current block element.
    pending: Vec<Span<'static>>,

    // ── Block state ──────────────────────────────────────────────────────────
    heading: Option<HeadingLevel>,
    in_code_block: bool,
    code_lang: String,

    // ── List state ───────────────────────────────────────────────────────────
    /// Stack entry: (is_ordered, current_item_counter).
    list_stack: Vec<(bool, u64)>,
    in_item: bool,
    item_bullet_emitted: bool,

    // ── Blockquote ───────────────────────────────────────────────────────────
    blockquote_depth: usize,

    // ── Agent-panel tool-line detection ──────────────────────────────────────
    /// Set to true when the first text in a paragraph starts with ⚙.
    is_tool_line: bool,
    /// True when the current paragraph lives inside a list item.
    paragraph_in_item: bool,

    // ── Table state ───────────────────────────────────────────────────────────
    in_table_cell: bool,
    table_is_header_row: bool,
    table_header: Vec<String>,
    table_body: Vec<Vec<String>>,
    table_current_row: Vec<String>,
    table_current_cell: String,
}

impl Renderer {
    fn new(width: usize) -> Self {
        Self {
            width,
            output: Vec::new(),
            bold: false,
            italic: false,
            pending: Vec::new(),
            heading: None,
            in_code_block: false,
            code_lang: String::new(),
            list_stack: Vec::new(),
            in_item: false,
            item_bullet_emitted: false,
            blockquote_depth: 0,
            is_tool_line: false,
            paragraph_in_item: false,
            in_table_cell: false,
            table_is_header_row: false,
            table_header: Vec::new(),
            table_body: Vec::new(),
            table_current_row: Vec::new(),
            table_current_cell: String::new(),
        }
    }

    // ── Style helpers ─────────────────────────────────────────────────────────

    fn prose_style(&self) -> Style {
        let mut s = Style::default().fg(Color::White);
        if self.bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            s = s.add_modifier(Modifier::ITALIC);
        }
        s
    }

    /// Append `text` to the pending span list, merging with the last span if
    /// it carries the same style (keeps the Vec small).
    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let style = self.prose_style();
        if let Some(last) = self.pending.last_mut() {
            if last.style == style {
                let mut s = last.content.to_string();
                s.push_str(text);
                *last = Span::styled(s, style);
                return;
            }
        }
        self.pending.push(Span::styled(text.to_string(), style));
    }

    // ── Heading flush ─────────────────────────────────────────────────────────

    fn flush_heading(&mut self) {
        // Blank line before heading — but not at the very start of the document.
        if !self.output.is_empty() {
            self.output.push(Line::from(""));
        }

        let text: String = self.pending.iter().map(|s| s.content.as_ref()).collect();
        let level = self.heading.unwrap_or(HeadingLevel::H3);
        let rule_width = self.width.saturating_sub(MARGIN.len()).max(4);

        match level {
            HeadingLevel::H1 => {
                let style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
                self.output.push(Line::from(Span::styled(format!("{MARGIN}{text}"), style)));
                // Full-width underline with ═
                self.output.push(Line::from(Span::styled(
                    format!("{MARGIN}{}", "═".repeat(rule_width)),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM),
                )));
            },
            HeadingLevel::H2 => {
                let style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
                self.output.push(Line::from(Span::styled(format!("{MARGIN}{text}"), style)));
                // Full-width underline with ─
                self.output.push(Line::from(Span::styled(
                    format!("{MARGIN}{}", "─".repeat(rule_width)),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                )));
            },
            HeadingLevel::H3 => {
                self.output.push(Line::from(Span::styled(
                    format!("{MARGIN}{text}"),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                )));
            },
            _ => {
                self.output.push(Line::from(Span::styled(
                    format!("{MARGIN}{text}"),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                )));
            },
        }

        self.output.push(Line::from(""));
        self.pending.clear();
        self.heading = None;
    }

    // ── Paragraph flush ───────────────────────────────────────────────────────

    /// Return the `(first_prefix, rest_prefix)` for the current block context.
    fn para_prefixes(&mut self) -> (String, String) {
        if self.in_item && !self.item_bullet_emitted {
            self.item_bullet_emitted = true;
            let bullet = self.current_bullet();
            let cont = self.item_continuation_indent();
            (bullet, cont)
        } else if self.in_item {
            let cont = self.item_continuation_indent();
            (cont.clone(), cont)
        } else if self.blockquote_depth > 0 {
            let pfx = format!("{MARGIN}{}", "│  ".repeat(self.blockquote_depth));
            (pfx.clone(), pfx)
        } else {
            (MARGIN.to_string(), MARGIN.to_string())
        }
    }

    /// Flush accumulated inline spans as word-wrapped lines.
    fn flush_para(&mut self, trail_blank: bool) {
        if self.pending.is_empty() {
            return;
        }

        if self.is_tool_line {
            // Tool lines: render dim, no re-wrapping.
            let text: String = self.pending.iter().map(|s| s.content.as_ref()).collect();
            self.output.push(Line::from(Span::styled(
                format!("{MARGIN}{}", text.trim()),
                Style::default().fg(Color::DarkGray),
            )));
            self.pending.clear();
            self.is_tool_line = false;
            return;
        }

        let (first, rest) = self.para_prefixes();
        let wrapped = reflow(std::mem::take(&mut self.pending), self.width, &first, &rest);
        self.output.extend(wrapped);
        if trail_blank {
            self.output.push(Line::from(""));
        }
    }

    // ── Table flush ───────────────────────────────────────────────────────────

    fn flush_table(&mut self) {
        let n_cols =
            self.table_header.len().max(self.table_body.iter().map(|r| r.len()).max().unwrap_or(0));
        if n_cols == 0 {
            return;
        }

        // Natural column widths (max content char-width across header + body).
        let mut col_widths: Vec<usize> = (0..n_cols)
            .map(|i| {
                let h = self.table_header.get(i).map(|s| s.chars().count()).unwrap_or(0);
                let b = self
                    .table_body
                    .iter()
                    .filter_map(|row| row.get(i))
                    .map(|s| s.chars().count())
                    .max()
                    .unwrap_or(0);
                h.max(b).max(1)
            })
            .collect();

        // Available chars for cell content: width - margin - border chars (n+1) - padding (2 per col).
        let margin_len = MARGIN.len();
        let borders = n_cols + 1;
        let padding = n_cols * 2;
        let available = self.width.saturating_sub(margin_len + borders + padding).max(n_cols); // at least 1 char per column

        // Scale down proportionally if we exceed available width.
        let natural_total: usize = col_widths.iter().sum();
        if natural_total > available {
            for w in &mut col_widths {
                *w = (*w * available / natural_total).max(1);
            }
        }

        // ── Border line builders ──────────────────────────────────────────────
        let top_border: String = format!(
            "{MARGIN}┌{}┐",
            col_widths.iter().map(|&w| "─".repeat(w + 2)).collect::<Vec<_>>().join("┬")
        );
        let mid_border: String = format!(
            "{MARGIN}├{}┤",
            col_widths.iter().map(|&w| "─".repeat(w + 2)).collect::<Vec<_>>().join("┼")
        );
        let bot_border: String = format!(
            "{MARGIN}└{}┘",
            col_widths.iter().map(|&w| "─".repeat(w + 2)).collect::<Vec<_>>().join("┴")
        );

        let border_style = Style::default().fg(Color::DarkGray);

        // ── Row renderer (returns a Line) ─────────────────────────────────────
        let render_row = |cells: &[String], widths: &[usize], is_header: bool| -> Line<'static> {
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::styled(format!("{MARGIN}│"), border_style));
            for (i, &w) in widths.iter().enumerate() {
                let raw = cells.get(i).map(|s| s.as_str()).unwrap_or("");
                let text = truncate_str(raw, w);
                let padded = format!(" {:<width$} ", text, width = w);
                let style = if is_header {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                spans.push(Span::styled(padded, style));
                spans.push(Span::styled("│".to_string(), border_style));
            }
            Line::from(spans)
        };

        // ── Emit lines ────────────────────────────────────────────────────────
        self.output.push(Line::from(Span::styled(top_border, border_style)));
        if !self.table_header.is_empty() {
            let header = self.table_header.clone();
            self.output.push(render_row(&header, &col_widths, true));
            self.output.push(Line::from(Span::styled(mid_border, border_style)));
        }
        let body = std::mem::take(&mut self.table_body);
        for row in &body {
            self.output.push(render_row(row, &col_widths, false));
        }
        self.output.push(Line::from(Span::styled(bot_border, border_style)));
        self.output.push(Line::from(""));

        self.table_header.clear();
    }

    // ── List helpers ──────────────────────────────────────────────────────────

    fn current_bullet(&self) -> String {
        if let Some((is_ordered, counter)) = self.list_stack.last() {
            let depth = self.list_stack.len().saturating_sub(1);
            let indent = "    ".repeat(depth);
            if *is_ordered {
                format!("{MARGIN}{indent}{}. ", counter)
            } else {
                format!("{MARGIN}{indent}•  ")
            }
        } else {
            MARGIN.to_string()
        }
    }

    fn item_continuation_indent(&self) -> String {
        if let Some((is_ordered, counter)) = self.list_stack.last() {
            let depth = self.list_stack.len().saturating_sub(1);
            let indent = "    ".repeat(depth);
            if *is_ordered {
                let num_width = counter.to_string().len() + 2; // "N. "
                format!("{MARGIN}{indent}{}", " ".repeat(num_width))
            } else {
                format!("{MARGIN}{indent}   ") // "•  " = 3 chars
            }
        } else {
            MARGIN.to_string()
        }
    }

    // ── Main event processor ──────────────────────────────────────────────────

    fn process(mut self, content: &str) -> Vec<Line<'static>> {
        let parser = Parser::new_ext(content, Options::all());

        for event in parser {
            match event {
                // ── Headings ──────────────────────────────────────────────────
                Event::Start(Tag::Heading { level, .. }) => {
                    self.heading = Some(level);
                },
                Event::End(TagEnd::Heading(_)) => {
                    self.flush_heading();
                },

                // ── Paragraphs ────────────────────────────────────────────────
                Event::Start(Tag::Paragraph) => {
                    self.is_tool_line = false;
                    self.paragraph_in_item = self.in_item;
                },
                Event::End(TagEnd::Paragraph) => {
                    self.flush_para(!self.paragraph_in_item);
                },

                // ── Code blocks ───────────────────────────────────────────────
                Event::Start(Tag::CodeBlock(kind)) => {
                    self.in_code_block = true;
                    self.code_lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    // Language label — dim italic, only when present
                    if self.code_lang == "mermaid" {
                        self.output.push(Line::from(Span::styled(
                            format!("{MARGIN}  mermaid"),
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC),
                        )));
                    } else if !self.code_lang.is_empty() {
                        self.output.push(Line::from(Span::styled(
                            format!("{MARGIN}  {}", self.code_lang),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                    }
                },
                Event::End(TagEnd::CodeBlock) => {
                    let is_mermaid = self.code_lang == "mermaid";
                    self.in_code_block = false;
                    if is_mermaid {
                        self.output.push(Line::from(Span::styled(
                            format!("{MARGIN}  · open in a browser to render"),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                    }
                    self.output.push(Line::from(""));
                },

                // ── Lists ─────────────────────────────────────────────────────
                Event::Start(Tag::List(ordered)) => {
                    let start = ordered.unwrap_or(1);
                    self.list_stack.push((ordered.is_some(), start.saturating_sub(1)));
                },
                Event::End(TagEnd::List(_)) => {
                    self.list_stack.pop();
                    if self.list_stack.is_empty() {
                        self.output.push(Line::from(""));
                    }
                },
                Event::Start(Tag::Item) => {
                    self.in_item = true;
                    self.item_bullet_emitted = false;
                    if let Some(last) = self.list_stack.last_mut() {
                        last.1 += 1;
                    }
                },
                Event::End(TagEnd::Item) => {
                    if !self.pending.is_empty() {
                        self.flush_para(false);
                    }
                    self.in_item = false;
                    self.item_bullet_emitted = false;
                },

                // ── Blockquotes ───────────────────────────────────────────────
                Event::Start(Tag::BlockQuote(_)) => {
                    self.blockquote_depth += 1;
                },
                Event::End(TagEnd::BlockQuote(_)) => {
                    self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
                    self.output.push(Line::from(""));
                },

                // ── Inline formatting ─────────────────────────────────────────
                Event::Start(Tag::Strong) => {
                    self.bold = true;
                },
                Event::End(TagEnd::Strong) => {
                    self.bold = false;
                },
                Event::Start(Tag::Emphasis) => {
                    self.italic = true;
                },
                Event::End(TagEnd::Emphasis) => {
                    self.italic = false;
                },

                // ── Leaf events ───────────────────────────────────────────────
                Event::Code(code) => {
                    if self.in_table_cell {
                        self.table_current_cell.push_str(&code);
                    } else {
                        // Inline code — cyan; color alone signals code, no backticks.
                        self.pending
                            .push(Span::styled(code.to_string(), Style::default().fg(Color::Cyan)));
                    }
                },
                Event::Text(text) => {
                    if self.in_code_block {
                        let (gutter_color, text_color) = if self.code_lang == "mermaid" {
                            (Color::Yellow, Color::DarkGray)
                        } else {
                            (Color::DarkGray, Color::White)
                        };
                        for line in text.lines() {
                            self.output.push(Line::from(vec![
                                Span::styled(
                                    format!("{MARGIN}  │ "),
                                    Style::default().fg(gutter_color),
                                ),
                                Span::styled(line.to_string(), Style::default().fg(text_color)),
                            ]));
                        }
                    } else if self.in_table_cell {
                        self.table_current_cell.push_str(&text);
                    } else {
                        if self.pending.is_empty() && text.trim_start().starts_with('⚙') {
                            self.is_tool_line = true;
                        }
                        self.push_text(&text);
                    }
                },
                Event::SoftBreak => {
                    if self.in_table_cell {
                        self.table_current_cell.push(' ');
                    } else if !self.in_code_block {
                        self.push_text(" ");
                    }
                },
                Event::HardBreak => {
                    if !self.pending.is_empty() {
                        let (first, rest) = self.para_prefixes();
                        let wrapped =
                            reflow(std::mem::take(&mut self.pending), self.width, &first, &rest);
                        self.output.extend(wrapped);
                    }
                },
                Event::Rule => {
                    let rule_width = self.width.saturating_sub(MARGIN.len() * 2).max(4);
                    self.output.push(Line::from(Span::styled(
                        format!("{MARGIN}{}", "─".repeat(rule_width)),
                        Style::default().fg(Color::DarkGray),
                    )));
                    self.output.push(Line::from(""));
                },

                // ── Tables ────────────────────────────────────────────────────
                Event::Start(Tag::Table(_)) => {
                    self.table_header.clear();
                    self.table_body.clear();
                },
                Event::End(TagEnd::Table) => {
                    self.flush_table();
                },
                Event::Start(Tag::TableHead) => {
                    self.table_is_header_row = true;
                    self.table_current_row.clear();
                },
                Event::End(TagEnd::TableHead) => {
                    self.table_header = std::mem::take(&mut self.table_current_row);
                    self.table_is_header_row = false;
                },
                Event::Start(Tag::TableRow) => {
                    self.table_current_row.clear();
                },
                Event::End(TagEnd::TableRow) => {
                    let row = std::mem::take(&mut self.table_current_row);
                    if !row.is_empty() {
                        self.table_body.push(row);
                    }
                },
                Event::Start(Tag::TableCell) => {
                    self.in_table_cell = true;
                    self.table_current_cell.clear();
                },
                Event::End(TagEnd::TableCell) => {
                    self.in_table_cell = false;
                    self.table_current_row.push(std::mem::take(&mut self.table_current_cell));
                },

                _ => {},
            }
        }

        // Flush any trailing content (e.g. incomplete streaming response).
        if !self.pending.is_empty() {
            self.flush_para(false);
        }

        self.output
    }
}

// ── Word-wrap + span reflow ───────────────────────────────────────────────────

/// Reflow styled inline spans into word-wrapped ratatui [`Line`]s.
///
/// * `first_prefix` — prepended to the first output line (e.g. `"    • "`).
/// * `rest_prefix`  — prepended to all continuation lines (must align with
///   text that follows `first_prefix`).
///
/// The prefix counts toward `width`, so text stays within the terminal column
/// budget.
fn reflow(
    spans: Vec<Span<'static>>,
    width: usize,
    first_prefix: &str,
    rest_prefix: &str,
) -> Vec<Line<'static>> {
    // Collect (word, style) pairs from all spans.
    let mut words: Vec<(String, Style)> = Vec::new();
    for span in &spans {
        let style = span.style;
        for word in span.content.split_whitespace() {
            if !word.is_empty() {
                words.push((word.to_string(), style));
            }
        }
    }

    if words.is_empty() {
        return Vec::new();
    }

    let first_pfx_len = first_prefix.chars().count();
    let rest_pfx_len = rest_prefix.chars().count();

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = vec![Span::raw(first_prefix.to_string())];
    let mut current_len = 0usize;
    let mut max_len = width.saturating_sub(first_pfx_len).max(8);

    for (word, style) in words {
        let word_len = word.chars().count();
        let sep = if current_len > 0 { 1 } else { 0 };

        if current_len > 0 && current_len + sep + word_len > max_len {
            // Wrap: emit current line, start a new one with rest_prefix.
            lines.push(Line::from(std::mem::take(&mut current)));
            current.push(Span::raw(rest_prefix.to_string()));
            current_len = 0;
            max_len = width.saturating_sub(rest_pfx_len).max(8);
        }

        if current_len > 0 {
            current.push(Span::raw(" ".to_string()));
            current_len += 1;
        }

        current.push(Span::styled(word, style));
        current_len += word_len;
    }

    if current_len > 0 {
        lines.push(Line::from(current));
    }

    lines
}
