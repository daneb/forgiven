# ADR 0008 — Normal Mode Editing Operations and Multi-key Sequences

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

After the initial modal editing scaffold (ADR 0007), the editor could navigate but not
meaningfully edit text in Normal mode. Three classes of issues needed to be addressed
simultaneously:

**Bug: cursor visually offset from text.** Every rendered line is prefixed with a
2-character diagnostic gutter (`"  "` or `"● "`). The cursor position was set using
raw `cursor.col` without accounting for this offset, making the cursor appear 2 columns
to the left of the character being edited.

**Bug: `$` movement overshot.** `move_cursor_line_end()` sets `col = line_len` — one
past the last character. In Normal mode `$` should land *on* the last character
(`col = line_len - 1`), matching real Vim behaviour.

**Bug: `h`/`l` wrapped across lines.** The left/right motion methods followed
Vim's `whichwrap` convention (wrapping at line boundaries). Vim's `h`/`l` motions do
*not* wrap — only arrow keys optionally do.

**Missing: Normal mode edit operations.** The editor had no `x`, `dd`, `D`, `yy`,
`p`/`P`, `gg`, `G`, or `u` — the minimum viable set for real editing without a mouse.

**Multi-key sequences.** `dd`, `gg`, and `yy` require two identical keypresses.
The existing `KeyHandler` only handled single keys and the `SPC`-prefixed leader tree.

## Decision

### 1. Cursor gutter offset fix

A compile-time constant `GUTTER_WIDTH: u16 = 2` is added to `render_buffer()`.
`set_cursor_position()` now offsets by this amount:

```rust
const GUTTER_WIDTH: u16 = 2;
frame.set_cursor_position((
    area.x + GUTTER_WIDTH + cursor_col as u16,
    area.y + cursor_row as u16,
));
```

### 2. Separate Normal-mode `$` motion

A new buffer method `move_cursor_line_end_normal()` is added alongside the existing
`move_cursor_line_end()`. The new method clamps to `line_len - 1`:

```rust
pub fn move_cursor_line_end_normal(&mut self) {
    let len = self.current_line_len();
    self.cursor.col = if len == 0 { 0 } else { len - 1 };
}
```

`$` in Normal mode maps to the new `MoveLineEndNormal` action; `A` (append at end of
line) continues to use `move_cursor_line_end()` to place the cursor past the last
character for immediate insertion.

### 3. Clamping `h`/`l` variants

Two new buffer methods provide non-wrapping left/right movement:

```rust
pub fn move_cursor_left_clamp(&mut self) {
    if self.cursor.col > 0 { self.cursor.col -= 1; }
}
pub fn move_cursor_right_clamp(&mut self) {
    let max = self.current_line_len().saturating_sub(1);
    if self.cursor.col < max { self.cursor.col += 1; }
}
```

The `MoveLeft`/`MoveRight` actions in `execute_action()` now call the clamping
variants. The wrapping versions are kept for Insert-mode arrow keys where wrapping is
expected.

### 4. New Normal mode edit operations

Nine new `Action` variants are added to `keymap/mod.rs`:

| Action | Key | Description |
|--------|-----|-------------|
| `DeleteChar` | `x` | Delete character at cursor |
| `DeleteLine` | `dd` | Delete current line into clipboard |
| `DeleteToLineEnd` | `D` | Delete from cursor to end of line |
| `GotoFileTop` | `gg` | Jump to first line |
| `GotoFileBottom` | `G` | Jump to last line |
| `YankLine` | `yy` | Copy current line into clipboard |
| `PasteAfter` | `p` | Paste clipboard contents as a new line below cursor |
| `PasteBefore` | `P` | Paste clipboard contents as a new line above cursor |
| `Undo` | `u` | Undo last edit (stub — history not yet implemented) |

Corresponding buffer methods implement the operations:

```rust
pub fn delete_char_at_cursor(&mut self)
pub fn delete_current_line(&mut self) -> String
pub fn delete_to_line_end(&mut self) -> String
pub fn yank_current_line(&self) -> String
pub fn paste_after_cursor(&mut self, text: &str)
pub fn paste_before_cursor(&mut self, text: &str)
pub fn goto_first_line(&mut self)
pub fn goto_last_line(&mut self)
```

The `Editor` struct gains a `clipboard: Option<String>` field. Delete and yank
operations store their text there; paste operations read from it.

### 5. Two-key prefix sequences

`KeyHandler` gains a `pending_key: Option<char>` field. On the first keypress of a
potentially doubled sequence (`d`, `g`, `y`) the key is stored and `Action::Noop` is
returned. On the second keypress the pair is resolved:

```
'd' + 'd' → DeleteLine
'g' + 'g' → GotoFileTop
'y' + 'y' → YankLine
```

Any other second key clears `pending_key` and returns `Noop`. The `sequence()` display
method includes the pending key so the user can see the partial sequence in the status
bar. New single-character bindings (`D`, `G`, `x`, `p`, `P`, `u`) are dispatched
directly without prefix handling.

### 6. Explorer mode groundwork

Two new `Action` variants — `ExplorerToggle` and `ExplorerFocus` — and a new
`Mode::Explorer` variant are added in this ADR, wired to leader keys `SPC e e` and
`SPC e f`. The full explorer implementation is covered in ADR 0010.

## Consequences

- The cursor now sits precisely on the character being edited — fixing the most
  visually jarring bug in the editor
- `h` at column 0 no longer jumps to the end of the previous line; `l` at the last
  character no longer jumps to the start of the next line
- `$` in Normal mode correctly lands on the last character; `A` still places the
  cursor past the last character for immediate typing
- `dd` / `yy` / `gg` require exactly two keypresses of the same key; any other
  second key silently cancels the prefix — consistent with Vim's behaviour
- `u` (undo) is a stub that sets a status message; a full undo history requires
  a persistent `EditHistory` structure (planned ADR)
- `p` / `P` paste whole lines, matching `yy`/`dd` line-wise semantics; character-wise
  yanking (e.g. `yw`) is not yet supported
- All edit operations call `notify_lsp_change()` so the language server stays in sync
  with the buffer contents after every mutation
