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
    parsing::{SyntaxDefinition, SyntaxSet, SyntaxSetBuilder},
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
            ps: Self::build_syntax_set(),
            ts: ThemeSet::load_defaults(),
            theme: "base16-ocean.dark".to_string(),
        }
    }

    /// Build the syntax set: start from the syntect defaults, then layer in the
    /// extra grammars that are not shipped with syntect (TOML, PowerShell, …).
    /// Each grammar is embedded at compile time via `include_str!` so no files
    /// need to be present at runtime.
    fn build_syntax_set() -> SyntaxSet {
        // Extra grammars not in syntect's default bundle.
        const EXTRA: &[(&str, &str)] = &[
            ("TOML", include_str!("syntaxes/TOML.sublime-syntax")),
            ("PowerShell", include_str!("syntaxes/PowerShell.sublime-syntax")),
        ];

        let mut builder: SyntaxSetBuilder = SyntaxSet::load_defaults_newlines().into_builder();

        for (name, src) in EXTRA {
            match SyntaxDefinition::load_from_str(src, true, None) {
                Ok(def) => builder.add(def),
                Err(e) => {
                    // Non-fatal — fall back to plain text for this grammar.
                    tracing::warn!("Failed to load bundled syntax '{}': {}", name, e);
                },
            }
        }

        builder.build()
    }

    /// Highlight a single line of text.
    ///
    /// `extension` is the file extension without a leading dot, e.g. `"rs"`, `"py"`.
    /// `filename` is the bare filename used for extensionless files like `"Dockerfile"`.
    /// Falls back to plain text if no matching syntax is found.
    ///
    /// Returns a `Vec<Span<'static>>` ready for ratatui rendering.
    pub fn highlight_line(
        &self,
        line: &str,
        extension: &str,
        filename: &str,
    ) -> Vec<Span<'static>> {
        let syntax = self
            .ps
            .find_syntax_by_extension(extension)
            .or_else(|| self.find_syntax_by_filename(filename))
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

    /// Return the bare filename (no directory) for a given path.
    pub fn filename_for(path: &std::path::Path) -> String {
        path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string()
    }

    /// Map extensionless filenames to a known syntax.
    /// Returns `None` when the filename is unrecognised.
    fn find_syntax_by_filename(
        &self,
        filename: &str,
    ) -> Option<&syntect::parsing::SyntaxReference> {
        match filename.to_ascii_lowercase().as_str() {
            // Dockerfiles — use Shell Script (closest bundled grammar)
            "dockerfile" | "dockerfile.dev" | "dockerfile.prod" | "dockerfile.test" => {
                self.ps.find_syntax_by_extension("sh")
            },
            // Make / Rake
            "makefile" | "gnumakefile" | "rakefile" => self.ps.find_syntax_by_name("Makefile"),
            // Common shell dot-files without an extension
            ".bashrc" | ".zshrc" | ".bash_profile" | ".profile" | ".bash_aliases" => {
                self.ps.find_syntax_by_extension("sh")
            },
            _ => None,
        }
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
    // Guard 1 — syntect uses a == 0 as the "no explicit color" sentinel.
    // Rendering it as RGB(0,0,0) makes text invisible on dark backgrounds.
    if c.a == 0 {
        return Color::Reset;
    }
    // Guard 2 — some themes assign background-level dark colours (e.g. base00
    // #2b303b, luma ≈ 47) to foreground scopes, making text invisible on a
    // matching dark terminal background. Treat any colour whose perceived
    // luminance is below 50 as "inherit terminal default foreground".
    let luma = (c.r as u32 * 299 + c.g as u32 * 587 + c.b as u32 * 114) / 1000;
    if luma < 50 {
        return Color::Reset;
    }
    Color::Rgb(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extra_syntaxes_resolve() {
        let h = Highlighter::new();
        for (ext, label) in [
            ("toml", "TOML"),
            ("ps1", "PowerShell"),
            ("sh", "Shell"),
            ("yml", "YAML"),
            ("html", "HTML"),
        ] {
            assert!(
                h.ps.find_syntax_by_extension(ext).is_some(),
                "missing syntax for .{ext} ({label})"
            );
        }
    }
}
