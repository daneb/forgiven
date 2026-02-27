# ADR 0009 — Syntax Highlighting with syntect

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

The editor rendered all buffer text as plain white text with no colouring. For a
code editor this severely reduces readability — keywords, strings, comments, and
identifiers are visually indistinguishable.

Two main options were evaluated:

| Approach | Pros | Cons |
|----------|------|------|
| **syntect** (TextMate grammars, pure Rust) | Ships grammars + themes out of the box; proven in many TUI editors; no native deps with `regex-fancy` feature; ~50 ms startup load, then very fast per-line | Grammar accuracy is behind tree-sitter for complex languages; no incremental parsing |
| **tree-sitter** | Industry-leading accuracy; incremental re-parse on every edit | Requires C bindings per language; grammar crates are separate; more complex integration with ratatui spans |

Since the editor currently highlights only the visible viewport (never the full
file), tree-sitter's incremental parsing advantage is less relevant. syntect's
self-contained grammar + theme bundles were preferred for simplicity at this stage.

## Decision

### Library

```toml
syntect = { version = "5", default-features = false,
            features = ["default-themes", "default-syntaxes", "regex-fancy"] }
```

`default-features = false` avoids pulling in `onig` (a C regex library). `regex-fancy`
selects the pure-Rust Fancy Regex backend. `default-themes` and `default-syntaxes`
bundle the TextMate grammars and Base16 / Monokai theme sets directly into the binary.

### `Highlighter` struct (`src/highlight/mod.rs`)

```rust
pub struct Highlighter {
    ps: SyntaxSet,   // loaded once at startup
    ts: ThemeSet,    // loaded once at startup
    theme: String,   // "base16-ocean.dark"
}
```

`SyntaxSet::load_defaults_newlines()` and `ThemeSet::load_defaults()` are expensive
(~50 ms combined) and are called once in `Editor::new()`. All subsequent calls to
`highlight_line()` are cheap.

### Highlight call

```rust
pub fn highlight_line(&self, line: &str, extension: &str) -> Vec<Span<'static>>
```

- Looks up the syntax by file extension via `find_syntax_by_extension()`; falls back
  to plain text if the extension is unknown.
- Creates a fresh `HighlightLines` per call (stack-allocated; negligible cost).
- Returns `Vec<Span<'static>>` (ratatui `Span` with `Style`) ready for direct rendering.

### Color and style mapping

syntect `Style` fields are converted to ratatui equivalents:

```rust
fn syntect_to_ratatui(style: syntect::highlighting::Style) -> Style {
    Style::default()
        .fg(Color::Rgb(c.r, c.g, c.b))          // true-colour foreground
        + optional Modifier::BOLD / ITALIC / UNDERLINED
}
```

syntect uses RGBA; the alpha channel is ignored because ratatui does not model
transparency.

### Viewport-only rendering

The expensive `highlight_line()` call is made only for the lines currently visible on
screen, not the entire file. In `Editor::render()`:

```rust
let term_height = self.terminal.size()?.height as usize - 1; // minus status line
let start = buf.scroll_row;
let end = (start + term_height).min(buf.lines().len());
let highlighted: Vec<Vec<Span>> = buf.lines()[start..end]
    .iter()
    .map(|line| self.highlighter.highlight_line(line, &ext))
    .collect();
```

The resulting `Option<Vec<Vec<Span<'static>>>>` is passed to `UI::render()` as
`highlighted_lines`.

### Renderer integration (`src/ui/mod.rs`)

`render_buffer()` receives `highlighted_lines: Option<&[Vec<Span<'static>>]>`. For
each visible line, if a pre-computed span list is available it is routed through
`render_highlighted_line()`; otherwise `render_line()` (plain text) is used as a
fallback.

`render_highlighted_line()` handles:
- Prepending the 2-character diagnostic gutter (`"  "` / `"● "`)
- Horizontal scrolling via `scroll_col` offset — spans are trimmed to fit within
  `viewport_width - 2` columns, character by character, preserving syntect styles
- Appending ghost text (Copilot inline completion) after the last span

## Consequences

- Syntax colours are available for every language supported by TextMate grammars
  (Rust, Python, TypeScript, Go, TOML, Markdown, and ~100 others) with no per-language
  configuration
- True-colour (`Color::Rgb`) is used; terminals that do not support 24-bit colour will
  fall back to their closest palette colour automatically
- The `"base16-ocean.dark"` theme is hard-coded; theme selection via
  `config.toml` is a planned future addition
- Highlighting is computed every frame for the visible viewport (~40–50 lines).
  For typical line lengths this is fast enough to stay within the 50 ms event-poll
  budget, but very long lines (>500 chars) may occasionally cause a visible delay
- There is no incremental parse: a full re-highlight of all visible lines happens
  after every edit. If this becomes a bottleneck, a per-line cache keyed on
  (line content hash, scroll_row) would reduce repeated work
- Grammar accuracy is lower than tree-sitter for nested or ambiguous constructs
  (e.g. complex Rust macros, JSX); upgrading to tree-sitter is a tracked future option
