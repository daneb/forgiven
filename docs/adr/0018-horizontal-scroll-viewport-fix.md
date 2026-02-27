# ADR 0018 — Horizontal Scroll Viewport Fix

**Date:** 2026-02-24
**Status:** Accepted

---

## Context

Long lines that extended past the visible text area were inaccessible: the user could
not scroll right to see or edit content beyond the right edge of the screen. The
symptom was most severe when the file explorer (25 cols) or agent panel (35–40% of
terminal width) were open, but it existed even in the single-panel layout because
of the 2-column diagnostic gutter.

Two separate bugs combined to produce this behaviour.

---

## Root Causes

### Bug 1 — `scroll_to_cursor` used full terminal width (primary)

In `Editor::render()` (`src/editor/mod.rs`), the call to `buf.scroll_to_cursor()` was
passed `viewport_width = size.width` — the raw terminal width — regardless of which
panels were visible:

```rust
// Before — always used the whole terminal width
let viewport_width = size.width as usize;
buf.scroll_to_cursor(viewport_height, viewport_width);
```

`scroll_to_cursor` only triggers horizontal scrolling when
`cursor.col >= scroll_col + viewport_cols`. With `viewport_cols = terminal_width`,
the trigger threshold was far too high — on a 200-column terminal with the explorer
open the user would have to type 173 characters on a single line before the view
scrolled at all.

The code even carried a comment acknowledging this:

> `viewport_width is the full terminal width as a conservative approximation`

### Bug 2 — Selection render loop had an off-by-2 error (secondary)

The character loop in `render_line`'s selection path broke at
`scroll_col + viewport_width` where `viewport_width = area.width` (already includes
the 2-char gutter). The non-selection path correctly used
`.take(viewport_width.saturating_sub(2))`, but the selection path rendered 2 extra
characters past the visible area on every selected line.

---

## Decision

### Fix 1 — Mirror the UI layout constraints to compute the real text area width

`Editor::render()` now replicates the three-panel layout math that `UI::render()`
uses, then subtracts the 2-column diagnostic gutter:

```rust
const GUTTER: usize = 2;
let total_w = size.width as usize;
let editor_area_w = match (self.file_explorer.visible, self.agent_panel.visible) {
    (true,  true)  => total_w.saturating_sub(25).saturating_sub(total_w * 35 / 100),
    (true,  false) => total_w.saturating_sub(25),
    (false, true)  => total_w * 60 / 100,
    (false, false) => total_w,
};
let viewport_width = editor_area_w.saturating_sub(GUTTER);
buf.scroll_to_cursor(viewport_height, viewport_width);
```

The layout constraints being mirrored:

| Panels open | Constraints | Editor area width |
|-------------|-------------|-------------------|
| Explorer + Agent | `[Length(25), Min(1), Percentage(35)]` | `W - 25 - W×35/100` |
| Explorer only | `[Length(25), Min(1)]` | `W - 25` |
| Agent only | `[Percentage(60), Percentage(40)]` | `W × 60/100` |
| Neither | `[Min(1)]` | `W` |

The gutter constant mirrors `GUTTER_WIDTH: u16 = 2` already defined in `render_buffer`.

### Fix 2 — Align the selection render loop with the non-selection path

In `render_line` (`src/ui/mod.rs`), the selection-path loop now uses
`text_width = viewport_width.saturating_sub(2)` for its break condition, matching
the `.take(viewport_width.saturating_sub(2))` already used in the non-selection path:

```rust
// Before
if col_idx >= scroll_col + viewport_width { break; }

// After
let text_width = viewport_width.saturating_sub(2);
if col_idx >= scroll_col + text_width { break; }
```

---

## Consequences

### Positive

* Typing or pasting text on a long line now causes the view to scroll right as soon
  as the cursor reaches the right edge of the visible text area (not the terminal).
* `h`/`l` and arrow keys correctly pan the view left when returning from a scrolled
  position.
* The fix applies in all panel combinations: solo editor, editor + explorer,
  editor + agent, editor + both panels.
* Selected text on long lines no longer renders 2 ghost characters past the boundary.

### Negative / trade-offs

* The viewport width calculation duplicates the layout constraint values from
  `UI::render()`. If the panel widths or split percentages change, this calculation
  must be updated in sync.  A future refactor could expose the computed `editor_area`
  `Rect` from `UI::render()` back to the editor (e.g. via a shared `LayoutCache`
  struct) to remove the duplication.
* The `Percentage` calculation uses integer division (`total_w * 35 / 100`) which
  matches ratatui's own internal truncation, but edge cases at small terminal sizes
  may differ by 1 column. This is acceptable — a 1-column error in the scroll trigger
  is invisible in practice.

---

## Alternatives Considered

| Option | Reason rejected |
|--------|----------------|
| Add a `layout_cache: Option<Rect>` field and pass the actual editor area from `render()` to `scroll_to_cursor` | Clean but requires a larger refactor; the duplication approach is self-contained and correct for all current layouts |
| Move scroll computation into `UI::render()` | Mixes model logic into the view; `Buffer` should remain unaware of the UI |
| Wrap long lines (soft-wrap) | Changes the editing model; horizontal scrolling is the standard vim behaviour and was the original intent |
