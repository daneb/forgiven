# ADR 0017 — Multi-line Yank / Delete and Visual Line Mode

**Date:** 2026-02-24
**Status:** Accepted

---

## Context

ADR 0016 introduced the yank/paste register and `ClipboardType`, but all operations
acted on a single line or a character-wise selection. Two common vim workflows were
still missing:

* **Count prefix** (`3yy`, `5dd`, `10j`) — repeating an operation N times.
* **Visual Line mode** (`V`) — selecting entire lines and operating on them as a unit.

Without these, bulk editing requires either manual repetition or entering Visual mode
and extending the selection character-by-character across line boundaries.

---

## Decision

### 1. Count-prefix accumulation (`src/keymap/mod.rs`)

`KeyHandler` gained `pending_count: Option<usize>` and a `take_count()` method.

At the top of `handle_normal()`, before any other matching, digits `1`–`9` (and `0`
after an earlier digit) accumulate into `pending_count`:

```rust
if ch.is_ascii_digit() && (ch != '0' || self.pending_count.is_some()) {
    let digit = (ch as usize) - ('0' as usize);
    self.pending_count = Some(self.pending_count.unwrap_or(0) * 10 + digit);
    return Action::Noop;
}
```

`take_count()` consumes and returns the count (defaulting to `1`):

```rust
pub fn take_count(&mut self) -> usize {
    self.pending_count.take().unwrap_or(1)
}
```

`clear_sequence()` was updated to also clear `pending_count`, and `sequence()` now
prepends the count prefix in the status display (e.g. `3d` while building `3dd`).

### 2. Count-safe `execute_action` guard (`src/editor/mod.rs`)

```rust
// Don't consume count for Noop (user may still be building prefix)
if matches!(action, Action::Noop) { return Ok(()); }
let count = self.key_handler.take_count();
```

This ensures that typing `3` → `Noop` → `d` → resolves pending_key `d` → `Noop` →
`d` → `DeleteLine` correctly delivers `count = 3` to the action handler.

### 3. Count-aware motion and yank/delete actions

All movement actions loop `count` times:

```rust
Action::MoveDown => {
    if let Some(buf) = self.current_buffer_mut() {
        for _ in 0..count { buf.move_cursor_down(); }
    }
}
```

Navigation (`GotoFileTop` / `GotoFileBottom`) treat a count > 1 as a line number
(`5G` = goto line 5, `5gg` = goto line 5):

```rust
Action::GotoFileBottom => {
    if count > 1 { buf.goto_line(count); } else { buf.goto_last_line(); }
}
```

`DeleteLine`, `YankLine`, and `ChangeLine` now use the count-based buffer methods:

| Action | Before | After |
|--------|--------|-------|
| `YankLine` | `buf.yank_current_line()` | `buf.yank_lines(count)` |
| `DeleteLine` | `buf.delete_current_line()` | `buf.delete_lines(count)` |
| `ChangeLine` | `buf.delete_current_line()` | `buf.delete_lines(count)` |

Status messages reflect the actual count (e.g. `"3 lines yanked"`).

### 4. New buffer methods (`src/buffer/buffer.rs`)

| Method | Description |
|--------|-------------|
| `yank_lines(count) → String` | Join `count` lines starting at cursor with `\n` |
| `delete_lines(count) → String` | Remove `count` lines, clamp cursor, return text |
| `goto_line(one_based)` | Jump to line N (1-based), clamp to file bounds |

### 5. Visual Line mode (`V`) — `Mode::VisualLine` + `Action::VisualLine`

`Mode::VisualLine` was added alongside the existing `Mode::Visual`.

`Buffer` gained `visual_line_anchor: Option<usize>` — the row where `V` was pressed.
The selection always covers whole lines from `min(anchor, cursor)` to
`max(anchor, cursor)`, with `col = 0` and `col = usize::MAX` so the full line
highlights with the existing renderer without any changes to the render logic:

```rust
pub fn start_selection_line(&mut self) {
    self.visual_line_anchor = Some(self.cursor.row);
    self.update_selection_line();
}

pub fn update_selection_line(&mut self) {
    let anchor = self.visual_line_anchor.unwrap_or(self.cursor.row);
    let cur = self.cursor.row;
    let (min_row, max_row) = if anchor <= cur { (anchor, cur) } else { (cur, anchor) };
    self.selection = Some(Selection {
        start: Cursor { row: min_row, col: 0 },
        end:   Cursor { row: max_row, col: usize::MAX },
    });
}
```

`clear_selection()` was updated to also clear `visual_line_anchor`.

### 6. New buffer methods for Visual Line operations

| Method | Description |
|--------|-------------|
| `yank_selection_lines() → Option<String>` | Copy selection rows, joined with `\n` |
| `delete_selection_lines() → Option<String>` | Remove selection rows, return text |

### 7. `handle_visual_line_mode()` (`src/editor/mod.rs`)

New method dispatched by `Mode::VisualLine` in `handle_key()`:

| Key(s) | Action |
|--------|--------|
| `Esc` / `V` | Clear selection, return to Normal |
| `y` | `yank_selection_lines` → `ClipboardType::Linewise`, Normal |
| `d` / `x` | `delete_selection_lines` → `ClipboardType::Linewise`, Normal |
| `c` | `delete_selection_lines` → `ClipboardType::Linewise`, Insert |
| `j` / `↓` | `move_cursor_down` + `update_selection_line` |
| `k` / `↑` | `move_cursor_up` + `update_selection_line` |
| `G` | `goto_last_line` + `update_selection_line` |
| `g` | `goto_first_line` + `update_selection_line` |

### 8. UI status line (`src/ui/mod.rs`)

`Mode::VisualLine` added to both the mode name and colour maps:

```rust
Mode::VisualLine => "VISUAL LINE",  // same magenta as Visual
```

---

## Consequences

### Positive

* `3yy`, `5dd`, `10j`, `5G` all work as in vim.
* `V` enters Visual Line mode; `j`/`k` extend the selection by whole lines.
* `Vy`, `Vd`, `Vc` operate on the selected line range with correct `Linewise` paste semantics.
* `usize::MAX` end-column trick avoids any changes to the existing highlight renderer.
* `goto_line()` enables `Ngg` / `NG` line-jump shortcuts.

### Negative / trade-offs

* `V` + `p` (paste over selection) is not implemented — a dedicated
  `ReplaceSelection` action is needed in a follow-up.
* Visual Line `g` is treated as a standalone go-to-first-line key rather than
  the two-key `gg` sequence, since `pending_key` is a Normal-mode concept and is
  not threaded into `handle_visual_line_mode`.  This is a pragmatic simplification
  that matches common `Vgg` select-all usage.
* Count prefix is consumed once at the top of `execute_action`; it cannot be
  split between the operator and the motion (e.g. `2d3j` is not supported — use
  `d5j` instead). This matches the majority of real-world vim muscle memory.

---

## Alternatives Considered

| Option | Reason rejected |
|--------|----------------|
| Implement `gg` two-key sequence inside `handle_visual_line_mode` | Requires threading `pending_key` state into the Visual Line handler; over-engineered for the g-to-top use case |
| Consume count inside `Action::Noop` arm | Would lose the count when the user presses a prefix digit then a pending-key first char |
| Use `i64::MAX` instead of `usize::MAX` for end column | Unnecessary — `end.col` is always compared as `usize`; `usize::MAX` is the natural sentinel |
