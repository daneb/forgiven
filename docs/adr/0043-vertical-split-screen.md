# ADR 0043 — Vertical Split Screen

**Date:** 2026-03-05
**Status:** Accepted

## Context

The editor showed a single active buffer in the centre pane. Users working across
two files (e.g. reading a test while editing its implementation, or comparing two
source files) had to switch buffers constantly with `SPC b b` / `SPC b n/p`.
Vim-style vertical splits are a well-understood solution: two panes side-by-side
with independent cursors, scroll positions, and editing state.

Buffers already carry fully independent state (cursor, scroll, selection, undo
history via snapshot), so no new buffer infrastructure was required — the work
was wiring up split metadata, routing key events, and updating the render
pipeline.

## Decision

### Core invariant

`current_buffer_idx` **always points to the focused pane's buffer**.  All
existing key handlers keep operating on `current_buffer_idx` unchanged.
Switching focus = swap `current_buffer_idx` ↔ `split_other_idx`.

### New keybindings (`src/keymap/mod.rs`)

Three new `Action` variants and a `SPC w` leader node:

| Binding | Action | Effect |
|---------|--------|--------|
| `SPC w v` | `WindowSplit` | Open vertical split (right pane = previous buffer) |
| `SPC w w` | `WindowFocusNext` | Cycle focus between left and right panes |
| `SPC w c` | `WindowClose` | Close split, return to single pane |

Which-key hints appear automatically from the existing leader-tree infrastructure.

### Editor state (`src/editor/mod.rs`)

Three new fields on `Editor`:

```rust
pub split_other_idx: Option<usize>,      // None = no split
pub split_right_focused: bool,            // true when right pane is focused
split_highlight_cache: Option<HighlightCache>, // syntax cache for inactive pane
```

**`Action::WindowSplit`** — guards: requires ≥2 open buffers; rejects if split
is already active. Sets `split_other_idx` to the most-recently-opened other
buffer (i.e. `current_buffer_idx - 1`, wrapping).

**`Action::WindowFocusNext`** — `mem::swap(current_buffer_idx, split_other_idx)`
then flips `split_right_focused`. All key handlers continue to operate on
`current_buffer_idx` without modification.

**`Action::WindowClose`** — clears `split_other_idx`, `split_right_focused`, and
`split_highlight_cache`.

**`Action::BufferClose` guard** — if the buffer being closed is the inactive
split pane's buffer, the split is cleared. If it is the focused buffer while a
split is active, focus is moved to the other pane before the buffer is removed,
preventing an invalid `current_buffer_idx`.

Before `terminal.draw()`, `split_buffer_data` (a snapshot of the inactive pane's
buffer) and `split_highlighted_lines` (syntax-highlighted spans with their own
`HighlightCache`) are extracted alongside the existing active-pane equivalents.

### UI rendering (`src/ui/mod.rs`)

`UI::render()` gains three new parameters:
- `split_buffer_data: Option<&BufferData>` — inactive pane snapshot
- `split_highlighted_lines: Option<&[Vec<Span<'static>>]>` — pre-highlighted spans
- `split_right_focused: bool` — which pane is focused

`render_buffer()` gains one new parameter:
- `show_cursor: bool` — gates `frame.set_cursor_position()` so only the focused
  pane's cursor is rendered (the terminal supports only one cursor position per
  frame).

When `split_buffer_data` is `Some`, `main_area` is subdivided into three columns:

```
[50% left pane] [1 char │ separator] [50% right pane]
```

`ghost_text` and `preview_lines` are routed to the focused pane only; `diagnostics`
are passed to both (a file may be open in both panes simultaneously).

## Consequences

- **No handler changes** — the core invariant means every existing editing
  operation, motion, undo/redo, LSP action, and search continue to work on the
  focused buffer without modification.
- **Single cursor** — ratatui (crossterm) supports one cursor position per frame,
  so only the focused pane shows a cursor. This matches Vim behaviour.
- **50/50 split only** — the initial implementation uses a fixed 50 % / 50 %
  split. Resizable splits are deferred.
- **No horizontal split** — only vertical (`SPC w v`) is implemented. A
  horizontal split variant (`SPC w s`) would follow the same pattern.
- **Scroll independence** — each pane scrolls its own buffer independently, so
  viewing the top of one file while editing the middle of another works naturally.
- **Split cleared on buffer close** — closing either pane's buffer clears the
  split rather than attempting to find a replacement buffer, keeping state simple.
