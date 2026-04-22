# ADR 0137 — Soft-Wrap Toggle and Long-Line Rendering Fix

**Status:** Implemented
**Date:** 2026-04-22

---

## Context

Pasting a large block of LLM-generated text into a new markdown file produced a single
line of 39,333+ characters (no newlines). Two problems followed:

1. **Cursor not visible / editing unusable.** With `soft_wrap = false` (the default),
   horizontal scrolling is the only way to navigate a line that wide. The cursor ends up
   at column 39,333, and there is no visual indication of position within the line.
   Toggling soft wrap required editing `~/.config/forgiven/config.toml` and restarting.

2. **Content overflowed the agent panel boundary.** Opening the agent panel while the
   cursor was at the far right of a huge line caused the editor's rendering to appear to
   bleed into the panel area, because the old scroll state and the new viewport width were
   briefly inconsistent from the user's perspective.

A secondary issue was also identified during investigation: the fast path in
`render_highlighted_line` collected `Vec<char>` for **every** syntect span on a line,
including spans entirely before `scroll_col`. On a 39,333-character line this meant
~39k character copies per render frame before the viewport even started.

---

## Decision

### 1. Add `SPC m w` — toggle soft wrap at runtime

A new `SoftWrapToggle` action is registered under the `SPC m` (markdown/preview) leader,
keyed to `w`. Pressing `SPC m w` flips `editor.config.soft_wrap` and displays a status
message (`Soft wrap on` / `Soft wrap off`). No restart or config edit is needed.

When soft wrap is on, lines reflow to `text_width = viewport_width − 2` columns. The
cursor is always visible (it wraps with the text), and content naturally stays within the
editor area regardless of whether the agent panel or file explorer is open.

### 2. Fix `render_highlighted_line` fast-path allocation

The fast path (no selection on the current row) previously did:

```rust
let span_chars: Vec<char> = span.content.chars().collect();
let span_len = span_chars.len();
```

for **every** span, even spans whose entire character range falls before `scroll_col`.

The fix adds an early-exit guard:

```rust
let span_len = span.content.chars().count();

if skipped + span_len <= scroll_col {
    // Entire span is before the viewport — skip without allocating.
    skipped += span_len;
    continue;
}
```

Spans that straddle the `scroll_col` boundary now use `chars().skip(n).take(budget)`
directly on the `&str` content rather than a pre-collected `Vec<char>`. This keeps
allocations proportional to the **visible** text width rather than the total line length.

---

## Consequences

**Fixed:** Pasting large single-line text into any buffer is now navigable. Pressing
`SPC m w` enables soft wrap, making the text reflow within the visible editor columns,
cursor always visible, content constrained to the editor area.

**Fixed:** Opening the agent panel while at a large horizontal scroll offset no longer
produces a visual overflow artifact, because soft wrap removes the horizontal scroll
dimension entirely.

**Improved:** Rendering performance for long lines with a large `scroll_col` is now
O(visible columns) instead of O(line length) in the no-selection fast path.

**No behaviour change:** The default remains `soft_wrap = false`. Files already using
`soft_wrap = true` via config are unaffected. The `SPC m w` toggle is additive.

---

## Implementation

| File | Change |
|------|--------|
| `src/keymap/mod.rs` | Add `SoftWrapToggle` to `Action` enum; register `SPC m w` in the `m` leader node |
| `src/editor/actions.rs` | Handle `Action::SoftWrapToggle` — flip `self.config.soft_wrap`, emit status |
| `src/ui/buffer_view.rs` | Fix `render_highlighted_line` fast path to skip pre-viewport spans without `Vec<char>` allocation |
