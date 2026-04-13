//! CSV → ratatui `Line` renderer.
//!
//! Converts a CSV string into a `Vec<Line<'static>>` for use in the editor's
//! CSV-preview mode.  The first row is treated as a header and rendered bold.
//! Columns are padded to their maximum content width (capped at 40 chars).

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Maximum column width before content is truncated.
const MAX_COL_WIDTH: usize = 40;
/// Separator between columns.
const SEP: &str = " │ ";

/// Render `content` (CSV text) into ratatui [`Line`]s.
pub fn render(content: &str) -> Vec<Line<'static>> {
    let rows = parse_csv(content);
    if rows.is_empty() {
        return vec![Line::from(Span::styled(
            "(empty file)",
            Style::default().fg(Color::DarkGray),
        ))];
    }

    // Compute per-column max width (capped).
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths: Vec<usize> = vec![0; num_cols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            let w = cell.chars().count().min(MAX_COL_WIDTH);
            if w > col_widths[i] {
                col_widths[i] = w;
            }
        }
    }

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows.len() + 2);

    for (row_idx, row) in rows.iter().enumerate() {
        let is_header = row_idx == 0;
        let mut spans: Vec<Span<'static>> = Vec::new();

        for (col_idx, width) in col_widths.iter().enumerate() {
            if col_idx > 0 {
                let sep_style = Style::default().fg(Color::DarkGray);
                spans.push(Span::styled(SEP.to_string(), sep_style));
            }
            let cell = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
            let truncated = truncate(cell, *width);
            let padded = format!("{:<width$}", truncated, width = width);

            let style = if is_header {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default()
            };
            spans.push(Span::styled(padded, style));
        }

        lines.push(Line::from(spans));

        // Divider after header row.
        if is_header {
            let div: String = col_widths
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let bar = "─".repeat(*w);
                    if i == 0 {
                        bar
                    } else {
                        format!("─┼─{}", bar)
                    }
                })
                .collect();
            lines.push(Line::from(Span::styled(div, Style::default().fg(Color::DarkGray))));
        }
    }

    lines
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max || max == 0 {
        s.to_string()
    } else {
        let t: String = chars.iter().take(max.saturating_sub(1)).collect();
        format!("{}…", t)
    }
}

/// Parse CSV content into rows of owned strings.
/// Falls back to a single error-banner line on parse failure.
fn parse_csv(content: &str) -> Vec<Vec<String>> {
    let mut rdr =
        csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(content.as_bytes());

    let mut rows: Vec<Vec<String>> = Vec::new();
    for result in rdr.records() {
        match result {
            Ok(record) => {
                rows.push(record.iter().map(|f| f.to_string()).collect());
            },
            Err(e) => {
                // Return what we have so far, plus an error row.
                rows.push(vec![format!("⚠ CSV parse error: {}", e)]);
                break;
            },
        }
    }
    rows
}
