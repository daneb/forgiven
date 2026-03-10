# ADR 0054 — Editor Quality-of-Life Improvements

**Date:** 2026-03-10
**Status:** Accepted

---

## Context

Four small but frequently-needed editing behaviours were missing:

1. **Tab indent** — pressing `Tab` in Insert mode inserted a literal `\t` character instead of spaces, violating the `use_spaces`/`tab_width` config.
2. **Shift+Tab dedent** — no way to remove a leading indent from the current line without manually deleting characters.
3. **Go to line** — `:12` in Command mode was not recognised; users had to count `j` presses or use search.
4. **Force-close buffer** — the only way to close a buffer with unsaved changes was `:q!`, which quit the entire application. There was no per-buffer discard.

---

## Decision

### 1. Tab indent (`Tab` in Insert mode)

`Tab` now respects `config.use_spaces` and `config.tab_width`:

- If `use_spaces = true`: inserts `tab_width` space characters.
- If `use_spaces = false`: inserts a single `\t`.

Config is extracted before the mutable buffer borrow to satisfy the borrow checker:

```rust
let use_spaces = self.config.use_spaces;
let tab_width  = self.config.tab_width;
if let Some(buf) = self.current_buffer_mut() {
    if use_spaces {
        for _ in 0..tab_width { buf.insert_char(' '); }
    } else {
        buf.insert_char('\t');
    }
}
```

### 2. Shift+Tab dedent (`BackTab` in Insert mode)

A new `Buffer::dedent_line(use_spaces, tab_width)` method removes one indent unit from the start of the current line:

- Spaces mode: removes up to `tab_width` leading spaces.
- Tab mode: removes one leading `\t`.
- No-op if the line has no leading indent.
- Adjusts `cursor.col` with `saturating_sub` so it never underflows.

```rust
pub fn dedent_line(&mut self, use_spaces: bool, tab_width: usize) {
    let row = self.cursor.row;
    let line = &self.lines[row];
    let to_remove = if use_spaces {
        line.chars().take_while(|&c| c == ' ').count().min(tab_width)
    } else {
        usize::from(line.starts_with('\t'))
    };
    if to_remove == 0 { return; }
    let byte_count: usize =
        self.lines[row].chars().take(to_remove).map(|c| c.len_utf8()).sum();
    self.lines[row].drain(..byte_count);
    self.cursor.col = self.cursor.col.saturating_sub(to_remove);
    self.mark_modified();
}
```

The `BackTab` handler in `editor/mod.rs` mirrors the Tab handler, extracting config before the mutable borrow.

### 3. Go to line (`:N` in Command mode)

`execute_command()` gained a numeric fallthrough arm before the "unknown command" error:

```rust
_ if cmd.chars().all(|c| c.is_ascii_digit()) => {
    if let Ok(n) = cmd.parse::<usize>() {
        if let Some(buf) = self.current_buffer_mut() {
            buf.goto_line(n);
        }
    }
}
```

`Buffer::goto_line` (pre-existing) clamps to valid line bounds and centres the viewport. This matches Vim's `:12` behaviour exactly.

### 4. Force-close buffer (`SPC b D`)

A new `Action::BufferForceClose` was added alongside the existing `Action::BufferClose`.

**Keymap** (`src/keymap/mod.rs`):

```
SPC b d  →  BufferClose        (existing — prompts if modified)
SPC b D  →  BufferForceClose   (new — discards without prompt)
```

**Handler** (`src/editor/mod.rs`):

Mirrors `BufferClose` exactly but omits the `is_modified` guard. Also handles the vertical-split case: if the closing buffer is the split pane, the split is torn down; if it is the primary pane, focus is transferred to the other before removing the buffer. The buffer index is clamped after removal so it never goes out of bounds.

```
SPC b D  →  remove current buffer, discard unsaved changes, no confirmation
```

---

## Implementation

| File | Change |
|---|---|
| `src/buffer/buffer.rs` | Added `dedent_line(use_spaces, tab_width)` |
| `src/keymap/mod.rs` | Added `Action::BufferForceClose`; registered `SPC b D` |
| `src/editor/mod.rs` | Tab: spaces/tab branch; BackTab: calls `dedent_line`; `:N` go-to-line arm; `BufferForceClose` handler |

No new dependencies. No breaking changes to existing keybindings or config schema.

---

## Consequences

- **Positive**: Tab/Shift+Tab now behave identically to VS Code / Neovim with `expandtab`.
- **Positive**: `:12` is a standard Vim workflow — removing the friction of searching for a line.
- **Positive**: `SPC b D` closes scratch / agent-modified buffers instantly without killing the session.
- **Negative**: `SPC b D` has no confirmation — mistyping `D` instead of `d` discards changes silently. The uppercase convention (capital = destructive, matching `DeleteFile`) is the only guard.
