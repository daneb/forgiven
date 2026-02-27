# ADR 0022 — Markdown Rendering (Agent Panel + Editor Preview)

**Status:** Accepted

---

## Context

Two distinct UI surfaces needed better markdown support:

1. **Agent panel** — chat responses from the Copilot LLM arrive as CommonMark markdown
   (headings, bold, inline code, fenced code blocks, bulleted/ordered lists, blockquotes).
   The previous renderer was a hand-rolled closure that could only word-wrap plain prose
   and grey-out lines starting with `⚙`.  Structural markdown was rendered literally
   (asterisks, backticks, `#` sigils) rather than visually.

2. **Editor — .md file viewing** — no rendered preview existed.  Users editing markdown
   documentation had to open a separate tool to see how the content looked.

---

## Decision

### Shared renderer: `src/markdown/mod.rs`

A single `pub fn render(content: &str, width: usize) -> Vec<Line<'static>>` is the
public API consumed by both surfaces.  Internally it uses a `Renderer` struct that drives
a `pulldown_cmark::Parser` event loop and accumulates ratatui `Span`s / `Line`s.

**Why `pulldown-cmark`?**
- Lightweight (~300 KB compiled, no native deps).
- Event-based pull parser: safe for incomplete / streaming markdown (partially-assembled
  LLM tokens never cause a panic — unclosed elements become plain text).
- Widely used across the Rust ecosystem; well-maintained.
- Alternatives (`comrak`, `termimad`) were heavier or opinionated about ANSI output.

**Rendered elements**

| Element | Appearance |
|---------|------------|
| H1 | `  ▌ text` — Yellow + Bold |
| H2 | `  ▍ text` — Cyan + Bold |
| H3 | `  ▎ text` — Green + Bold |
| H4–H6 | `  ▏ text` — White + Bold |
| Paragraph | Word-wrapped, `  ` indent, White |
| **Bold** | `Modifier::BOLD` |
| *Italic* | `Modifier::ITALIC` |
| `` `inline code` `` | Backtick-wrapped, Cyan |
| Fenced code block | `╭─ lang ` header / `│ line` in Green / `╰─` footer — DarkGray borders |
| Unordered list | `  • ` bullet, 4-space indent per nesting level |
| Ordered list | `  N. ` counter, aligned continuation indent |
| Blockquote | `  │ ` prefix, DarkGray |
| Horizontal rule | `  ──────────────────────────────────────` in DarkGray |
| ⚙ tool-call lines | DarkGray, no re-wrap (existing agent-panel convention) |

**Word-wrap / hanging indent (`reflow()`)**

A standalone `reflow(spans, width, first_prefix, rest_prefix)` function handles
word-wrapping styled span sequences.  The `(first_prefix, rest_prefix)` pair lets bullet
points render correctly:
```
  • First item text that wraps
    across multiple lines here
```

### Agent panel (`src/ui/mod.rs`)

The hand-rolled `render_content` closure was removed.  Both committed messages and the
in-progress `streaming_reply` now call `crate::markdown::render(content, content_width)`.

The ⚙ tool-call convention is preserved inside the renderer itself (no special-casing
needed at the call site).

### Editor markdown preview (`Mode::MarkdownPreview`)

`SPC m p` toggles a read-only rendered preview overlay for any buffer:

- `execute_action(Action::MarkdownPreviewToggle)` sets `mode = Mode::MarkdownPreview`
  and resets `preview_scroll = 0`.
- Inside `render()`, when in preview mode the full buffer content is rendered via
  `crate::markdown::render(content, viewport_width)`, then the slice starting at
  `preview_scroll` is passed to `UI::render_buffer()` as `preview_lines`.
- `render_buffer()` detects `preview_lines.is_some()` and renders the pre-built lines
  directly, bypassing the syntax-highlighter path.  No cursor is emitted.
- `handle_preview_mode()` handles `j/k` (line scroll), `Ctrl+D/U` (half-page),
  `g/G` (top/bottom), `Esc/q` (exit).

The preview is intentionally available for **any** buffer (not just `.md`), because
`render()` accepts arbitrary CommonMark.  In practice it is useful only for markdown
files — the status bar shows `PREVIEW` in Magenta as a visual cue.

---

## Consequences

**Positive**
- Agent chat responses are now fully rendered: code blocks with language labels, nested
  lists, bold section headings, etc.  Dramatic improvement for complex answers.
- Markdown preview inside the editor requires zero external tools; `SPC m p` is instant.
- The shared renderer means both surfaces are consistent and only one code path to
  maintain.
- `pulldown-cmark` handles streaming / incomplete markdown gracefully — safe for the
  live `streaming_reply` accumulation.

**Negative / trade-offs**
- `pulldown-cmark` adds ~300 KB to the binary (negligible).
- The preview does not yet support scrolling the syntax-highlighted edit view and the
  rendered view simultaneously in a split (planned for a future ADR).
- Very large markdown files (10 000+ lines) will render all lines on each frame when in
  preview mode.  A future optimisation could cache the rendered lines keyed on
  `lsp_version`, similar to the highlight cache.

---

## Amendment 1 — Ordered list counter off-by-one

**Problem:** Numbered lists rendered starting at `2.` instead of `1.` (and `3.` instead
of `2.`, etc.). The `Tag::List(ordered)` handler pushed the starting number directly onto
the list stack, and `Tag::Item` incremented it *before* rendering the bullet:

```rust
// Before — broken
Event::Start(Tag::List(ordered)) => {
    let start = ordered.unwrap_or(0);
    self.list_stack.push((ordered.is_some(), start));  // pushed 1
}
Event::Start(Tag::Item) => {
    if let Some(last) = self.list_stack.last_mut() {
        last.1 += 1;  // bumped to 2 before first bullet rendered
    }
}
```

For a standard `1.` list, `start = 1` was pushed and then incremented to `2` on the
first item, so every number was one too high.

**Fix:** Initialise the counter one below the start value so the first `+= 1` lands on
the correct number:

```rust
// After — correct
Event::Start(Tag::List(ordered)) => {
    let start = ordered.unwrap_or(1);
    self.list_stack.push((ordered.is_some(), start.saturating_sub(1)));
}
```

This correctly handles lists that start at any number (e.g. `3.` through `5.`), and is a
no-op for unordered lists where the counter value is never used for display.

**File changed:** `src/markdown/mod.rs` — one-line fix to the `Tag::List` arm.
