# ADR 0105 — Tree-sitter Text Objects

**Date:** 2026-04-04
**Status:** Accepted

---

## Context

ADR 0104 introduced the `TsEngine` / `TsSnapshot` foundation. The first consumer
of that foundation is **text objects** — a core modal-editing feature.

Text objects let the user operate on semantic regions of code rather than
navigating character-by-character. They are composed of:

- An **operator**: `v` (select), `d` (delete), `y` (yank), `c` (change)
- A **motion modifier**: `i` (inner — body only) or `a` (outer — whole node)
- A **kind character**: `f` (function), `c` (class/struct/impl), `b` (block)

Examples:
| Sequence | Meaning |
|----------|---------|
| `vif` | Visual-select the body of the enclosing function |
| `vaf` | Visual-select the entire enclosing function (incl. signature) |
| `vic` | Visual-select the body of the enclosing class/struct/impl |
| `vac` | Visual-select the entire enclosing class/struct/impl |
| `vib` | Visual-select the enclosing `{}` block |
| `daf` | Delete the entire enclosing function |
| `yif` | Yank the function body |
| `cac` | Change (delete + enter Insert) the entire class |

Without tree-sitter these operations are impossible to implement reliably
because function/class boundaries cannot be determined with a line-oriented
regex pass.

---

## Decision

### 1. `TextObjectKind` in `src/keymap/mod.rs`

A new enum in the keymap module (alongside `Action`):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextObjectKind {
    Function, // 'f' — function_item / function_definition / …
    Class,    // 'c' — struct_item / class_definition / impl_item / …
    Block,    // 'b' — block / statement_block / compound_statement / …
}
```

`TextObjectKind::from_char(ch)` maps `f → Function`, `c → Class`,
`b → Block`, anything else → `None`.

### 2. Four new `Action` variants

```rust
SelectTextObject { inner: bool, kind: TextObjectKind }, // v + i/a + f/c/b
DeleteTextObject { inner: bool, kind: TextObjectKind }, // d + i/a + f/c/b
YankTextObject   { inner: bool, kind: TextObjectKind }, // y + i/a + f/c/b
ChangeTextObject { inner: bool, kind: TextObjectKind }, // c + i/a + f/c/b
```

### 3. Keymap wiring

**Normal mode — three-key sequences:**

The existing `pending_key` / `pending_second_key` mechanism is extended:

- When `pending_key ∈ {d, y, c}` and the next char is `i` or `a`, store both
  in pending and wait for the third key (just like `dt/df/yt/yf/ct/cf`).
- On the third key, if it matches a `TextObjectKind`, emit the appropriate
  Action variant.

**Visual mode:**

`handle_visual_mode` gains a `visual_text_obj_prefix: Option<char>` field on
`Editor`. When `i` or `a` is pressed in Visual mode, the prefix is stored.
On the next keypress, the kind character is matched and
`Action::SelectTextObject` is executed.

### 4. `src/treesitter/query.rs`

New module with:

- `row_col_to_byte(source, row, char_col)` — converts buffer `(row, char_col)`
  (where `char_col` is a Unicode char index) to a byte offset in the joined
  source string.
- `exclusive_byte_end_to_cursor(source, row, byte_col)` — converts a tree-sitter
  exclusive-end position (byte-based) to an inclusive buffer cursor position
  (char-based).
- `ancestor_matching(snap, row, col, predicate)` — walks up from the leaf node
  at the cursor to find the innermost ancestor matching a predicate.
- Language-specific `is_function_node`, `is_class_node`, `is_block_node`
  classifiers.
- `find_body_child(parent, lang)` — returns the `block` / `statement_block`
  child of a function/class node (used for `inner` selections).
- `text_object_range(snap, row, col, inner, kind)` — the public entry point,
  returns `Option<(start_row, start_col, end_row, end_col)>` in buffer
  char-index coordinates.

### 5. Action handlers in `src/editor/actions.rs`

`execute_action` handles the four new variants:

- **`SelectTextObject`**: enter Visual mode, move cursor to range start,
  `start_selection()`, move cursor to range end, `update_selection()`.
- **`DeleteTextObject`**: save undo snapshot, set selection to range,
  `delete_selection()`, sync clipboard, back to Normal.
- **`YankTextObject`**: set selection to range, `yank_selection()`,
  sync clipboard, clear selection, stay in Normal.
- **`ChangeTextObject`**: save undo snapshot, set selection to range,
  `delete_selection()`, sync clipboard, enter Insert.

---

## Implementation

### New file: `src/treesitter/query.rs`

Contains all node-finding and range-computation logic.

### Modified: `src/treesitter/mod.rs`

Add `pub mod query;`.

### Modified: `src/keymap/mod.rs`

- New `TextObjectKind` enum (above `Action`)
- Four new `Action` variants
- `pending_key`/`pending_second_key` handling extended to include `i`/`a`
  as valid second keys after `d`/`y`/`c`
- Three-key resolver extended to emit `*TextObject` actions for `di/da/yi/ya/ci/ca`

### Modified: `src/editor/mod.rs`

New field:

```rust
// ── Visual mode text object state ─────────────────────────────────────────────
/// Pending `i`/`a` prefix for visual text object selection.
/// Set when `i` or `a` is pressed in Visual mode; cleared on the next key.
visual_text_obj_prefix: Option<char>,
```

Initialised `None` in `Editor::new()`.

### Modified: `src/editor/input.rs`

`handle_visual_mode` checks `visual_text_obj_prefix` at the start of each call.
`'i'` and `'a'` keypresses in Visual mode set the prefix.

### Modified: `src/editor/actions.rs`

Four new arms in the `execute_action` match.

---

## Consequences

**Positive**

- `vif` / `vaf` / `vic` / `vac` / `vib` work for all supported languages
  (Rust, Python, JS, TS, Go, JSON, Bash).
- `dif`, `yif`, `caf`, etc. work as single-operation commands.
- Graceful fallback: if no tree-sitter node is found (unknown language, parse
  error, cursor outside any function), a status message is shown and no
  selection is made.
- Zero performance impact when not in use — trees are already cached lazily
  from ADR 0104.

**Negative / trade-offs**

- Argument text objects (`ia`/`aa`) are not implemented. Argument boundaries
  require finding sibling nodes around comma separators — deferred to a
  follow-up.
- The `inner` vs `outer` distinction for `Block` text objects is simplified:
  both `ib` and `ab` select the full block node including its braces. Trimming
  to content-only requires byte-accurate brace detection — deferred.
- Tree-sitter end positions are EXCLUSIVE (point to the byte after the last
  byte of the node). The `exclusive_byte_end_to_cursor` helper handles the
  conversion, including the col=0 edge case.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0104](0104-tree-sitter-core-integration.md) | TsEngine foundation — prerequisite |
| ADR 0106 (planned) | Code folding — next consumer of `TsSnapshot` |
| ADR 0107 (planned) | Sticky scroll — uses `ancestor_matching` from this ADR |
