# ADR 0034 — Explorer File Deletion

**Status:** Accepted

---

## Context

The file explorer sidebar (ADR 0010) allows navigation and, since ADR 0015, file
creation via the `n` key.  ADR 0025 added a hidden-files toggle and ADR 0030
introduced an explorer rename flow (`r` key, `Mode::RenameFile`).

One fundamental operation was still missing: **deleting** a file or directory directly
from the explorer.  Users had to leave the editor and use a shell to remove entries,
which broke the single-window workflow.

The main design concern is safety.  Deletion is irreversible.  Any implementation must
give the user a clear opportunity to cancel before data is lost.

---

## Decision

### Mode: `Mode::DeleteFile`

A new editor mode, `Mode::DeleteFile`, is added to `src/keymap/mod.rs` alongside the
existing `Mode::RenameFile`.  It is entered when the user presses `d` in
`Mode::Explorer` with an entry selected.

### Trigger

In `handle_explorer_mode` (`src/editor/mod.rs`):

```rust
KeyCode::Char('d') => {
    if let Some(path) = self.file_explorer.selected_path() {
        self.delete_confirm_path = Some(path);
        self.file_explorer.blur();
        self.mode = Mode::DeleteFile;
    }
}
```

The path is stored in the new `delete_confirm_path: Option<PathBuf>` field on the
`Editor` struct.  The explorer is blurred so cursor keys feed the confirmation popup,
not the tree.

### Confirmation popup

`UI::render_delete_popup` (in `src/ui/mod.rs`) renders a centred, 3-row popup with a
red border and the filename:

```
┌────────── Delete ──────────┐
│  Delete 'filename'?  [y/N] │
└────────────────────────────┘
```

The popup is drawn after all other layers, so it appears above the explorer and editor
panes.  The mode indicator in the status bar shows `DELETE` in red.

### Key handling (`handle_delete_mode`)

| Key | Effect |
|-----|--------|
| `y` / `Y` | Confirm — call `do_delete()` |
| `n` / `N` / `Esc` | Cancel — clear state, return to `Mode::Explorer` |
| *(anything else)* | Ignored |

### Deletion logic (`do_delete`)

```rust
if path.is_dir() {
    std::fs::remove_dir_all(&path)?;
} else {
    std::fs::remove_file(&path)?;
}
```

After deletion:

1. **Open buffers are closed** — any buffer whose `file_path` starts with the deleted
   path (handles directory removal correctly) is removed from `self.buffers`.  The
   current buffer index is clamped to the new length.
2. **Explorer is reloaded** — `self.file_explorer.reload()` refreshes the tree from
   disk so the deleted entry disappears immediately.
3. **Status message** — `"Deleted '<name>'"` is shown in the status bar.
4. Mode returns to `Mode::Explorer`.

### Render wiring

The `delete_name` (filename string extracted from the path) is computed in the render
function and passed to `UI::render` as a new `delete_name: Option<&str>` parameter,
keeping the render call stateless and consistent with how `rename_buffer` is passed.

---

## Alternatives considered

**Undo-able soft delete (trash / recycle bin)**
Moving deleted files to the system trash would be reversible but adds a platform
dependency (`trash` crate or OS-specific APIs).  Given that the editor already
requires confirmation, a hard delete is acceptable and keeps the dependency footprint
small.

**No confirmation (immediate delete on `d`)**
Rejected.  A single accidental keypress would cause irreversible data loss.  The one
extra keypress (`y`) is a small price for safety.

**`:delete` command-mode command**
Command mode is an alternative entry point but requires the user to know the exact
path string.  Explorer-context deletion is more ergonomic because the target is
already visually selected.

---

## Consequences

**Positive**
- Files and directories can be removed without leaving the editor.
- Destructive operations are guarded by a mandatory `y` confirmation, consistent with
  the rename popup pattern established in ADR 0030.
- Open buffers are cleaned up automatically — no dangling references to deleted paths.
- Works for both files (`remove_file`) and directories (`remove_dir_all`).
- Status bar feedback confirms the completed deletion.

**Negative / trade-offs**
- Deletion is permanent.  There is no undo path; `std::fs::remove_dir_all` is not
  reversible through the editor's snapshot history.
- A confirmation popup adds one modal state (`Mode::DeleteFile`) to the mode graph.
  The total number of modes is now 13, up from 12.
