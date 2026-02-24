# ADR 0019 — Snapshot-based Undo / Redo

**Status:** Accepted

---

## Context

The editor needed `u` (undo) and `Ctrl+R` (redo) — standard vim operations.

The pre-existing `EditHistory` type used a fine-grained operation log (`EditOp` enum with
`InsertChar`, `InsertNewline`, `DeleteCharBefore`, `DeleteCharAt`).  This approach had two
fatal shortcomings:

1. **Coverage gap** — None of the block-level operations (`delete_lines`, `delete_word`,
   `paste_linewise`, `delete_selection`, `delete_selection_lines`, etc.) ever called
   `history.record()`, so they were invisible to undo.
2. **Inversion complexity** — Every new buffer mutation would require a hand-written inverse.
   Paste-linewise, visual-line delete, and word-change all operate on variable numbers of
   lines; computing exact inverses is error-prone.

---

## Decision

Replace the op-log approach with **full-state snapshots**.

### `BufferSnapshot`

```rust
pub struct BufferSnapshot {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}
```

### `EditHistory`

```rust
pub struct EditHistory {
    past:   Vec<BufferSnapshot>,   // oldest … most-recent
    future: Vec<BufferSnapshot>,   // states saved during undo (for redo)
}
```

* `save(lines, row, col)` — push a snapshot onto `past`; clear `future` (new edit
  invalidates redo chain).
* `undo(current, row, col)` → pops `past`, pushes current state onto `future`, returns
  the snapshot to restore.
* `redo(current, row, col)` — mirror image of `undo`.
* Maximum 100 snapshots per stack (`MAX_SNAPSHOTS = 100`; oldest evicted when full).
  At ~10 KB average per snapshot that caps memory at ~1 MB per buffer.

### Where snapshots are saved

Snapshots are saved in `Editor::execute_action()` via a `needs_snapshot` guard that fires
before any mutating action:

```
Insert / InsertAppend / InsertLineStart / InsertLineEnd
InsertNewlineBelow / InsertNewlineAbove
DeleteChar / DeleteLine / DeleteToLineEnd / DeleteWord
DeleteSelection / ChangeLine / ChangeWord
PasteAfter / PasteBefore
```

Visual-mode operators that bypass `execute_action` save the snapshot inline before the
buffer mutation:

| Handler | Operator | Snapshot inline? |
|---------|----------|-----------------|
| `handle_visual_mode` | `c` | yes (delete_selection) |
| `handle_visual_line_mode` | `d` / `x` | yes (delete_selection_lines) |
| `handle_visual_line_mode` | `c` | yes (delete_selection_lines) |

Charwise-visual `y`/`d`/`x` already route through `execute_action` (via
`Action::YankSelection` / `Action::DeleteSelection`), so they are covered automatically.
Visual-line `y` is a read-only operation and needs no snapshot.

### Insert-mode coalescing

The snapshot is saved **once** when the editor _enters_ Insert mode (on
`Action::Insert` / `Action::InsertAppend` / etc.), not on every keystroke.  This means
pressing `u` after a long Insert session undoes the whole session in one step — matching
standard vim behaviour.

### Keybindings

| Key | Action |
|-----|--------|
| `u` | `Action::Undo` |
| `Ctrl+R` | `Action::Redo` |

Status messages:
- `"Already at oldest change"` when nothing left to undo.
- `"Already at newest change"` when nothing left to redo.

---

## Consequences

**Positive**
- Every buffer mutation is undoable with zero per-operation bookkeeping.
- Adding new buffer operations does not require writing an inverse.
- Insert-mode coalescing is trivially correct.

**Negative / trade-offs**
- Memory: up to ~1 MB per open buffer for the snapshot stacks (capped at 100 each).
- Snapshots are not persisted — undo history is lost when the buffer is closed.
- Undo granularity is coarser than character-level op logs for Insert mode (entire
  Insert session = one undo step).  This matches vim's default behaviour and is
  generally desirable.
