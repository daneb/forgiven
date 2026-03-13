# ADR 0060: Vim Character Motions (f/t/F/T and dt/df/yt/yf/ct/cf)

**Date:** 2026-03-13
**Status:** Accepted

## Context

Forgiven's Normal-mode keymap supported a fixed set of two-key operator-motion pairs (`dd`, `dw`, `d$`, `yy`, `yw`, `y$`, `cc`, `cw`) and the `gg` goto. Vim's character-find motions — `f{c}` (move to char), `t{c}` (move till char), and their operator forms (`dt{c}`, `df{c}`, etc.) — were not implemented. These are among the most-used motions in everyday Vim editing (e.g. `dt"` to delete up to the next quote, `cf(` to change up to the next parenthesis).

The existing key-handling state machine in `src/keymap/mod.rs` tracked only a single `pending_key: Option<char>` for two-key sequences. There was no mechanism to buffer a third key for sequences of the form `operator + motion-type + char-argument`.

## Decision

### New `Action` variants (`src/keymap/mod.rs`)

Five new variants added to the `Action` enum:

| Variant | Keys | Behaviour |
|---------|------|-----------|
| `DeleteToChar { ch, inclusive: false }` | `dt{c}` | Delete from cursor up to (not including) next `{c}` |
| `DeleteToChar { ch, inclusive: true }` | `df{c}` | Delete from cursor through (including) next `{c}` |
| `YankToChar { ch, inclusive }` | `yt{c}` / `yf{c}` | Yank to/through next `{c}` |
| `ChangeToChar { ch, inclusive }` | `ct{c}` / `cf{c}` | Delete to/through next `{c}` + enter Insert |
| `FindCharForward { ch, inclusive }` | `f{c}` / `t{c}` | Move cursor onto / before next `{c}` on line |
| `FindCharBackward { ch, inclusive }` | `F{c}` / `T{c}` | Move cursor onto / after previous `{c}` on line |

### Three-key state machine (`src/keymap/mod.rs`)

Added `pending_second_key: Option<char>` to `KeyHandler`. The resolution logic now runs in three priority stages each call to `handle_normal()`:

1. **Three-key resolution** — if `pending_second_key.is_some()`, consume both `pending_key` and `pending_second_key` together with the incoming char to produce a `*ToChar` action.
2. **Two-key resolution** — if `pending_key.is_some()`, check whether the second key (`f` or `t`) requires a char argument. If so, store both keys and return `Noop`; otherwise resolve the existing two-key combinations as before.
3. **Single-key dispatch** — `f`, `t`, `F`, `T` are added to the pending-key trigger set so that standalone motions (`f"`) also go through stage 2, resolving as `FindChar*` actions.

The `sequence()` display method was updated to show the full pending prefix (e.g. `dt` in the status bar while waiting for the target char).

### New buffer methods (`src/buffer/buffer.rs`)

| Method | Purpose |
|--------|---------|
| `find_char_forward(ch) -> Option<usize>` | Column of the next occurrence of `ch` after the cursor on the current line |
| `find_char_backward(ch) -> Option<usize>` | Column of the previous occurrence of `ch` before the cursor |
| `delete_to_col(end_col) -> String` | Delete `[cursor.col, end_col)` and return deleted text |
| `yank_to_col(end_col) -> String` | Return `[cursor.col, end_col)` without modifying the buffer |
| `move_to_col(col)` | Move cursor to `col`, clamping to line length |

### `execute_action` handlers (`src/editor/mod.rs`)

- `DeleteToChar` / `YankToChar` / `ChangeToChar` call `find_char_forward`, compute the inclusive or exclusive end column, then call `delete_to_col` or `yank_to_col`. Deleted text goes into `self.clipboard` as `ClipboardType::Charwise` and is synced to the system clipboard.
- `ChangeToChar` additionally sets `self.mode = Mode::Insert` after deletion.
- `FindCharForward` / `FindCharBackward` call the corresponding find method and `move_to_col`. No buffer modification, no clipboard interaction.
- `DeleteToChar`, `YankToChar`, and `ChangeToChar` are included in the undo-snapshot `needs_snapshot` match so that destructive operations are undoable.

## Consequences

- All motions are scoped to the current line (matching Vim behaviour — `f`/`t` do not cross line boundaries).
- Backward operator forms (`dF{c}`, `dT{c}`) are not implemented; only forward deletion is supported for now.
- The state machine cleanly generalises: adding further three-key sequences (e.g. `di"` text-objects) requires only adding another branch to the three-key resolver without restructuring the existing two-key logic.
