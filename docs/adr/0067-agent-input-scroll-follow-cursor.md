# ADR 0067 — Agent Input Box Scroll-to-Cursor

**Date:** 2026-03-17
**Status:** Accepted

---

## Context

The agent panel input box grows dynamically as the user types, up to a maximum of 10 visible text lines (clamped to keep the chat history visible). When the typed content exceeds this limit — via word-wrap on long lines or explicit newlines (Alt+Enter) — the `Paragraph` widget rendered from the top, hiding the cursor and most-recently-typed text at the bottom. The user could not see what they were typing.

## Decision

Compute a scroll offset for the input `Paragraph` so the last line (where the cursor lives) is always visible. The offset accounts for badge lines (file attachments, image attachments, pasted blocks) that sit above the typed text within the same widget.

```rust
let total_content_lines = badge_lines + total_wrapped;
let visible_lines = input_height.saturating_sub(2) as usize; // interior rows
let input_scroll = if total_content_lines > visible_lines {
    (total_content_lines - visible_lines) as u16
} else {
    0
};
```

Applied via `Paragraph::scroll((input_scroll, 0))` in `src/ui/mod.rs`.

## Consequences

- The cursor and trailing text are always visible, regardless of input length.
- Badge lines (file/image/paste summaries) scroll off the top when there is not enough room, which is the expected trade-off — the user cares about what they are actively typing.
- No new fields or state required; the offset is computed each render frame from existing values.
