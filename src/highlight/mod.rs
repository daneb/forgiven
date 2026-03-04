//! Syntax highlighting via syntect (TextMate grammars).
//!
//! `Highlighter` is constructed once at editor startup (loading grammar/theme data
//! takes ~50 ms) and then `highlight_line()` is called per visible line each frame.
//! Only the visible viewport is highlighted — the full buffer is never processed.

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color as SColor, FontStyle, ThemeSet},
    parsing::SyntaxSet,
};

/// Stateful highlighter. Holds the compiled grammar and theme data.
pub struct Highlighter {
    ps: SyntaxSet,
    ts: ThemeSet,
    /// Default theme name (must be present in `ts.themes`).
    theme: String,
}

impl Highlighter {
    /// Create a new highlighter, loading built-in grammars and themes.
    /// This is the expensive call — do it once at startup.
    pub fn new() -> Self {
        Self {
            ps: SyntaxSet::load_defaults_newlines(),
            ts: ThemeSet::load_defaults(),
            theme: "base16-ocean.dark".to_string(),
        }
    }

    /// Highlight a single line of text.
    ///
    /// `extension` is the file extension without a leading dot, e.g. `"rs"`, `"py"`.
    /// Falls back to plain text if the extension is unknown.
    ///
    /// Returns a `Vec<Span<'static>>` ready for ratatui rendering.
    pub fn highlight_line(&self, line: &str, extension: &str) -> Vec<Span<'static>> {
        let syntax = self
            .ps
            .find_syntax_by_extension(extension)
            .unwrap_or_else(|| self.ps.find_syntax_plain_text());

        let theme =
            self.ts.themes.get(&self.theme).unwrap_or_else(|| &self.ts.themes["base16-ocean.dark"]);

        let mut h = HighlightLines::new(syntax, theme);

        // `highlight_line` expects a line with a trailing newline; add one if absent.
        let line_with_nl =
            if line.ends_with('\n') { line.to_string() } else { format!("{}\n", line) };

        let ranges = match h.highlight_line(&line_with_nl, &self.ps) {
            Ok(r) => r,
            Err(_) => return vec![Span::raw(line.to_string())],
        };

        ranges
            .into_iter()
            .filter_map(|(style, text)| {
                // Strip the trailing newline we added.
                let text = text.trim_end_matches('\n');
                if text.is_empty() {
                    return None;
                }
                Some(Span::styled(text.to_string(), syntect_to_ratatui(style)))
            })
            .collect()
    }

    /// Return the file extension for a given path, or empty string for no extension.
    pub fn extension_for(path: &std::path::Path) -> String {
        path.extension().and_then(|e| e.to_str()).unwrap_or("").to_string()
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

// ── Color / style conversion ──────────────────────────────────────────────────

fn syntect_to_ratatui(style: syntect::highlighting::Style) -> Style {
    let fg = convert_color(style.foreground);
    let mut rat_style = Style::default().fg(fg);

    if style.font_style.contains(FontStyle::BOLD) {
        rat_style = rat_style.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        rat_style = rat_style.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        rat_style = rat_style.add_modifier(Modifier::UNDERLINED);
    }

    rat_style
}

fn convert_color(c: SColor) -> Color {
    // syntect uses a: 0 = opaque in some themes. Treat a == 0 as fully opaque.
    Color::Rgb(c.r, c.g, c.b)
}
