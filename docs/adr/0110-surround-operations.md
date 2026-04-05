# ADR 0110 — Surround Operations

**Date:** 2026-04-05
**Status:** Accepted

---

## Context

Surround operations (`cs`, `ds`, `ys`) are among the most used editing shortcuts in
the Vim ecosystem (vim-surround, nvim-surround, Helix). They allow the user to:

- Delete a surrounding delimiter pair: `ds(`
- Change one surrounding delimiter to another: `cs"'`
- Add a surrounding delimiter around a word: `ys{ch}`

These are pure keymap + buffer string operations — no new crates, no external
dependencies, no AST integration required. They fit within the existing 2–3 key
pending sequence mechanism already used for text objects and fold commands.

---

## Decision

### Supported operations

| Sequence | Action |
|----------|--------|
| `ds{ch}` | Delete surrounding `{ch}` — finds the nearest enclosing pair on the current line and removes both characters |
| `cs{from}{to}` | Change surrounding `{from}` to `{to}` — replaces the enclosing pair |
| `ys{ch}` | Add surrounding `{ch}` around the word under the cursor |

### Delimiter pairs

```
( or )  →  ( )
[ or ]  →  [ ]
{ or }  →  { }
< or >  →  < >
any other char (", ', `, |, …)  →  symmetric pair
```

When searching backwards for the opening delimiter, either the open or close character
of a pair is accepted as the search key (e.g., `ds)` and `ds(` both target `( )`).

### Scope: single-line only (v1)

Surround search is confined to the current line. Multi-line surround (e.g., delimiters
on different lines) is deferred to a future revision. A status message is shown when no
enclosing pair is found.

---

## Implementation

### `src/buffer/buffer.rs`

Three new public methods:

```rust
pub fn surround_delete_chars(&mut self, row: usize, open_col: usize, close_col: usize)
pub fn surround_replace_chars(&mut self, row: usize, open_col: usize, close_col: usize, new_open: char, new_close: char)
pub fn surround_insert_chars(&mut self, row: usize, word_start: usize, word_end: usize, open: char, close: char)
```

Each method mutates the line in-place and calls `mark_modified()`.

### `src/keymap/mod.rs`

New `Action` variants:

```rust
SurroundDelete { ch: char },
SurroundChangePrepare { from: char },
SurroundChange { from: char, to: char },
SurroundAddWord { ch: char },
```

The `('d' | 'c' | 'y', 's')` two-key combination is added to the pending-second-key
routing check, routing these to the 3-key resolver. The resolver emits:

- `('d', 's', ch)` → `SurroundDelete { ch }`
- `('c', 's', ch)` → `SurroundChangePrepare { from: ch }`
- `('y', 's', ch)` → `SurroundAddWord { ch }`

### `src/editor/mod.rs`

New field:

```rust
surround_change_from: Option<char>,
```

Stores the `from` char between `SurroundChangePrepare` and the next keypress.

### `src/editor/input.rs` — `handle_normal_mode`

Before dispatching to `key_handler.handle_normal()`, check `surround_change_from`:

```rust
if let Some(from) = self.surround_change_from.take() {
    if let KeyCode::Char(to) = key.code {
        return self.execute_action(Action::SurroundChange { from, to });
    }
    return Ok(()); // non-char cancels
}
```

### `src/editor/actions.rs`

Four new arms in `execute_action` calling private helper methods:

- `SurroundDelete` → `apply_surround_delete(ch)`
- `SurroundChangePrepare` → `self.surround_change_from = Some(from)`
- `SurroundChange` → `apply_surround_change(from, to)`
- `SurroundAddWord` → `apply_surround_add_word(ch)`

Helper methods search the current line for the enclosing pair and call the Buffer
primitives. `apply_surround_add_word` finds word bounds (non-whitespace run containing
`cursor.col`) and inserts the open/close characters around them.

---

## Consequences

**Positive**

- Three heavily-used editing operations available with minimal keystrokes.
- Pure safe Rust, zero new dependencies, zero unsafe.
- Single-line scope keeps the implementation simple and the edge-case surface small.
- Undo works automatically — `save_undo_snapshot()` is called before each operation.

**Negative / trade-offs**

- Multi-line surround is not supported in v1. Delimiters on different lines require
  a future extension.
- `ys{ch}` surrounds the word under the cursor (non-whitespace run), not a
  tree-sitter-aware text object. For precise structural surround, use the agent.
- `cs{from}{to}` uses a two-phase key sequence (`SurroundChangePrepare` stores
  intermediate state in the Editor). The status bar shows the pending `from` char
  during the intermediate state.

**Future work**

- Multi-line surround: walk lines outward when no pair is found on the current line.
- `ysiw`/`ysaf` — tree-sitter text object as surround target.
- Surround with HTML/XML tags: `ysiw<div>`.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0007](0007-vim-modal-keybindings.md) | Pending-key mechanism — extended |
| [0008](0008-normal-mode-editing-operations.md) | Edit operations — surround follows same snapshot/notify pattern |
| [0105](0105-tree-sitter-text-objects.md) | Text objects — same operator prefix keys (`d`, `y`, `c`) |
