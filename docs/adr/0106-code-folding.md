# ADR 0106 — AST-Based Code Folding

**Date:** 2026-04-05
**Status:** Accepted

---

## Context

Forgiven now has an in-process Tree-sitter parse tree for every open buffer
(ADR 0104). The parse tree makes fold regions trivially computable — every
function and class node that spans more than one line is a foldable region.

Code folding is one of the most used editor features for navigating large files.
Vim-style bindings (`za` toggle, `zM` close all, `zR` open all) are the
expected interface for modal-editor users.

---

## Decision

### 1. Fold state storage in `Editor`

```rust
fold_closed: HashMap<usize, HashSet<usize>>,
// buf_idx → set of fold-start rows that are currently collapsed
```

The fold state is keyed by buffer index (not path), so it resets when a buffer
is closed and reopened.  Each entry in the set is the start row of a collapsed
fold region.

### 2. Fold region computation in `src/treesitter/query.rs`

`fold_ranges(snap: &TsSnapshot) -> Vec<(usize, usize)>` returns all foldable
`(start_row, end_row)` pairs by walking the parse tree and collecting
`is_function_node` and `is_class_node` nodes that span more than one line.
Inner `{}` blocks are excluded to keep the fold list focused on declaration-
level regions.

### 3. Keybindings

| Key | Action |
|-----|--------|
| `za` | Toggle fold at cursor (`FoldToggle`) |
| `zM` | Close all folds (`FoldCloseAll`) |
| `zR` | Open all folds (`FoldOpenAll`) |

`z` is added to the Normal-mode pending-key list alongside `d`, `g`, `y`, `c`
etc.  The second key resolves to the fold action.

### 4. Cursor management

When a fold is closed with the cursor inside the fold body, the cursor is moved
to the fold-start row so it remains visible.  `fold_close_all` also relocates
the cursor if the current row becomes hidden.

### 5. Rendering

`FoldData` is a new type in `src/ui/mod.rs`:

```rust
pub struct FoldData {
    pub hidden_rows: HashSet<usize>,     // rows inside a closed fold
    pub fold_starts: HashMap<usize, usize>, // start_row → end_row (closed folds)
}
```

`render_buffer` now receives `Option<&FoldData>` and `Option<&str>` (sticky
header — see ADR 0107).  When `FoldData` is supplied:

- The inner loop iterates buffer rows from `scroll_row`, skipping any row in
  `hidden_rows`.
- For fold-start rows (`fold_starts.contains_key(&buf_row)`), a
  `··· N lines` stub is appended in `DarkGray` after the fold-start line.
- The terminal cursor Y position is correctly computed because the caller
  (editor `render()`) subtracts the number of hidden rows above the cursor from
  `cursor.row` before building `BufferData`.

The syntax-highlight cache is extended by `hidden_rows.len()` additional lines
so that rows beyond the normal viewport still have highlight spans available via
`line_idx = buf_row - scroll_row`.

---

## Implementation

### `src/treesitter/query.rs`

New public functions `fold_ranges` and `collect_fold_ranges` (private).

### `src/keymap/mod.rs`

- New `Action` variants: `FoldToggle`, `FoldCloseAll`, `FoldOpenAll`.
- `z` added to the pending-key list.
- Three new arms in the pending-key resolver: `('z', 'a')`, `('z', 'M')`,
  `('z', 'R')`.

### `src/editor/mod.rs`

- New field `fold_closed: HashMap<usize, HashSet<usize>>`.
- New methods: `fold_toggle()`, `fold_close_all()`, `fold_open_all()`.
- `render()` computes `FoldData` (hidden_rows + fold_starts) and `sticky_header`
  before building `buffer_data` and `highlighted_lines`.  The `buffer_data.cursor`
  row is adjusted to the visual row (fold-skipped row count subtracted).

### `src/editor/actions.rs`

Three new arms in `execute_action` dispatching to the fold methods above.

### `src/ui/mod.rs`

- New `FoldData` struct (public).
- Two new fields on `RenderContext`: `fold_data: Option<&'a FoldData>`,
  `sticky_header: Option<&'a str>`.

### `src/ui/buffer_view.rs`

`render_buffer` rewritten to:
1. Render the sticky header (1 row) when present.
2. Iterate buffer rows, skipping hidden rows.
3. Append fold stubs to fold-start lines.
4. Position the terminal cursor accounting for the sticky header offset.

---

## Consequences

**Positive**

- `za`/`zM`/`zR` work for all Tree-sitter supported languages (Rust, Python,
  JS, TS, Go, JSON, Bash).
- Graceful degradation: for files with no tree-sitter parse (unknown extension
  or parse failure), a status message is shown and nothing is folded.
- Zero per-frame overhead when no folds are active.
- Highlight cache correctly covers fold-extended row ranges.

**Negative / trade-offs**

- Fold state is not persisted across sessions (resets on buffer close).
- Selection rendering across fold boundaries may render oddly — selections
  spanning hidden rows are an edge case deferred to a future ADR.
- `scroll_to_cursor` (in `Buffer`) does not yet account for fold-hidden rows;
  scrolling near folds may be slightly imprecise.

**Future work**

- Fold state persistence (per-file in `~/.local/share/forgiven/folds.json`).
- Fold-aware `scroll_to_cursor` (pass hidden-row count to `Buffer`).
- Argument text objects as fold regions (comma-separated parameter lists).
- Incremental re-parse using `InputEdit` to keep fold ranges current without
  re-computing after every keystroke.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0104](0104-tree-sitter-core-integration.md) | TsEngine + TsSnapshot — prerequisite |
| [0105](0105-tree-sitter-text-objects.md) | Text objects — first consumer of the AST |
| ADR 0107 | Sticky scroll — rendered in the same `render_buffer` pass |
