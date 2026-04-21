# ADR 0133 — Markdown Readability Upgrade

**Status:** Implemented
**Date:** 2026-04-21
**Amends:** ADR 0022 (Markdown Rendering)

---

## Context

The agent panel is the most-read surface in forgiven. Despite producing richly formatted markdown responses, the renderer was presenting all fenced code blocks as plain white text — the syntax highlighting pipeline (syntect / base16-ocean.dark) used by the main editor was simply never threaded through to the markdown renderer. Beyond that, headings used basic terminal colour names (`Color::Yellow / Cyan / Green`), code blocks had no visual frame to distinguish them from prose, and consecutive block elements ran together with no breathing room.

The goal is a blissful reading experience for markdown and code, assuming a modern truecolor terminal with Unicode support. The dependency on font and terminal quality is accepted — forgiven already requires a capable terminal for its box-drawing characters and colour palette.

---

## Decision

### 1. Syntax highlighting in fenced code blocks

`markdown::render()` now accepts `Option<&Highlighter>`. When `Some(hl)` is provided, fenced code blocks are highlighted using a single stateful `HighlightLines` instance per block — the same base16-ocean.dark theme as the main editor. Using one instance per block (rather than one per line) ensures multi-line constructs (block comments, string literals, heredocs) are tokenised correctly across line boundaries.

A new `Highlighter::highlight_block(text, ext) -> Vec<Vec<Span<'static>>>` method encapsulates this: it creates a `HighlightLines`, iterates all lines, and returns styled spans ready for ratatui.

Call sites that have no highlighter (future popup renderers, tests) pass `None` and receive prose-coloured text, preserving backwards compatibility.

**Language → extension mapping** (`lang_to_extension()`): normalises markdown fence labels (`rust`, `python`, `javascript`, `ts`, `bash`, `sh`, …) to syntect extension keys. Unknown labels map to `""` which triggers syntect's plain-text fallback.

### 2. Code block framing

Blocks are wrapped in box-drawing borders in muted blue-gray (`Rgb(70, 80, 105)`):

```
    ╭─ rust ──────────────────────────────╮
    ▏ fn main() {                          
    ▏     println!("Hello, world!");       
    ▏ }                                    
    ╰─────────────────────────────────────╯
```

- Top border includes the language label in dim italic `Rgb(140, 160, 190)`.
- Mermaid blocks use a warm gold label (`Rgb(220, 185, 80)`) to match their special status.
- The gutter character changes from `│` to `▏` (U+258F, LEFT ONE EIGHTH BLOCK) to visually distinguish content lines from the corner pieces.
- A blank line is inserted before each block when preceded by non-blank output, giving code visual separation from surrounding prose.

### 3. Rgb heading palette

Basic terminal colour names replaced with truecolor values tuned for dark terminal backgrounds (luma 15–30):

| Level | Text | Underline char | Underline colour |
|-------|------|----------------|-----------------|
| H1 | `Rgb(255, 200, 80)` warm gold | `═` | `Rgb(130, 90, 25)` dim gold |
| H2 | `Rgb(100, 200, 210)` soft teal | `─` | `Rgb(45, 100, 110)` dim teal |
| H3 | `Rgb(145, 205, 125)` sage green | — | — |
| H4–H6 | `Rgb(175, 175, 190)` pale lavender | — | — |

### 4. Prose text

`Color::White` → `Rgb(210, 215, 220)`. A slightly warm white that reduces eye strain during long reading sessions without breaking contrast against dark backgrounds.

### 5. Inline code background

```rust
// Before
Style::default().fg(Color::Cyan)

// After
Style::default().fg(Color::Rgb(175, 230, 215)).bg(Color::Rgb(28, 42, 52))
```

The dark chip background (`Rgb(28, 42, 52)`) creates a subtle "pill" effect that visually anchors inline code without requiring backtick punctuation.

### 6. Paragraph breathing room

`flush_para()` inserts a leading blank line before top-level paragraphs that follow non-blank output (code blocks, lists, other paragraphs). This only applies outside list items and blockquotes, where the existing indentation already provides structure.

### 7. Blockquote gutter

The `│` prefix is now emitted as a styled `Span` in warm amber (`Rgb(185, 145, 75)`) rather than a plain `DarkGray` string prefix. A post-reflow pass on wrapped lines splits the first span at the gutter boundary and re-styles it — no change to the `reflow()` word-wrap logic itself.

### 8. List bullet hierarchy

```
depth 1  ●  (MEDIUM BLACK CIRCLE, U+25CF)
depth 2  ◦  (WHITE BULLET, U+25E6)
depth 3+ ▸  (BLACK RIGHT-POINTING SMALL TRIANGLE, U+25B8)
```

### 9. Horizontal rules

`Color::DarkGray` → `Rgb(80, 82, 105)` muted indigo, consistent with the code block border tone.

---

## Implementation

| File | Change |
|---|---|
| `src/highlight/mod.rs` | Add `highlight_block(text, ext) -> Vec<Vec<Span>>` |
| `src/markdown/mod.rs` | Renderer lifetime `'h`; `highlighter: Option<&'h Highlighter>` field; `lang_to_extension()`; all visual changes above |
| `src/ui/markdown.rs` | `render_message_content(content, width, hl)` — pass `Some(hl)` to `markdown::render` |
| `src/ui/agent_panel.rs` | `render_agent_panel(…, highlighter)` — thread to `render_message_content` |
| `src/ui/mod.rs` | `RenderContext.highlighter: &'a Highlighter`; pass to `render_agent_panel` |
| `src/editor/render.rs` | Populate `highlighter: &self.highlighter` in `RenderContext`; update markdown preview call |

---

## Cache invalidation

`PanelRenderCache` keys on `(msg_count, content_width)`. The `Highlighter` is constructed once at startup and is immutable thereafter, so no cache key change is required — the rendered output for a given message at a given width is stable.

---

## Trade-offs accepted

**Syntect startup cost.** The `Highlighter` was already paying the ~50 ms startup cost for the main editor; threading it into markdown rendering adds no new startup cost.

**Language label case-sensitivity.** `lang_to_extension()` lowercases the fence label before matching. Labels in unusual casing (e.g. `Python`, `BASH`) map correctly. Truly unrecognised labels fall back to plain text — no error, no crash.

**Inline code in `reflow()`.** The word-wrap function splits spans on whitespace. An inline code span containing spaces (e.g. `` `foo bar` ``) will be split across two words with a gap in the background chip. This is a pre-existing limitation of `reflow()` and is accepted for now; fixing it requires making spans with a `bg` colour atomic (a future improvement).

**Terminal requirement.** Rgb colours require a truecolor terminal. Terminals that do not support truecolor will approximate or drop the colours. This is consistent with forgiven's existing stance — the TUI already relies on Unicode box-drawing and 256-colour rendering.

---

## Relationship to existing ADRs

| ADR | Relationship |
|---|---|
| ADR 0009 | Syntect / base16-ocean.dark pipeline; `highlight_block` reuses the same theme and grammar bundle |
| ADR 0021 | `PanelRenderCache` unchanged; cache key analysis above |
| ADR 0022 | This ADR amends the element appearance table and adds the `Highlighter` parameter to the public API |
