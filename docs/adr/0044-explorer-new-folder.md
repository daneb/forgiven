# ADR 0044 — Explorer New Folder

**Date:** 2026-03-06
**Status:** Accepted

---

## Context

The file explorer supported creating new files (`n` key → Command mode
pre-filled with `"e <dir>/"`) but had no direct way to create a new directory.
Users had to open a terminal or use the command palette to create directories,
then reload the explorer with `R`. This was a common enough operation that a
first-class in-editor shortcut was justified.

---

## Decision

### Trigger

`m` (mnemonic: **m**kdir) in Explorer mode opens `Mode::NewFolder`. The target
parent directory is whichever entry is currently selected in the explorer tree:
if the cursor is on a file, its parent directory is used; if it is on a
directory, that directory is used directly.

### State

Two fields are added to `Editor`:

```rust
new_folder_buffer: String,              // folder name being typed
new_folder_parent: Option<PathBuf>,     // resolved parent directory
```

Both are reset to empty / `None` on confirmation or cancellation.

### Key handling (`handle_new_folder_mode`)

| Key | Behaviour |
|-----|-----------|
| `Enter` | Call `do_create_folder()` |
| `Esc` | Clear buffer, return to `Mode::Explorer` |
| `Backspace` | Delete last character |
| Any char except `/` `\` | Append to buffer |

Path separators (`/` and `\`) are blocked to prevent accidental
sub-directory names — the user creates one level at a time.

### Creation logic (`do_create_folder`)

1. Trim the buffer; if empty, cancel silently and return to Explorer.
2. Join the name onto `new_folder_parent` to form the target path.
3. If the path already exists, show an error in the status bar and keep the
   popup open so the user can edit the name.
4. Otherwise call `std::fs::create_dir_all(&new_dir)` — this handles the case
   where nested paths are needed (e.g. if `new_folder_parent` itself was
   freshly created and not yet flushed).
5. Call `file_explorer.reload()` so the new directory appears immediately.
6. Show `"Created folder '<name>'"` in the status bar and return to Explorer.

### UI (`render_new_folder_popup`)

A centred floating popup overlays the explorer panel. It uses a `LightGreen`
border and title to distinguish it from the `RenameFile` (yellow) and
`DeleteFile` (red) popups. The status-bar mode indicator shows `MKDIR` in
`LightGreen`.

---

## Alternatives considered

**Re-use the Command mode `:mkdir` convention**
Adding a `:mkdir` command would be consistent with `:e` for files, but
requires the user to know the full path rather than inferring it from the
cursor position.  The dedicated Explorer shortcut is context-aware and faster.

**Allow `/` in the name to create nested folders in one step**
Permitting path separators would require splitting the input and creating each
component individually.  `create_dir_all` would handle this, but the UX of
typing `foo/bar/baz` in a single-line popup is confusing.  A single-level
shortcut is sufficient for the common case.

---

## Consequences

**Positive**
- Folders can be created without leaving the editor.
- The parent directory is resolved from context — no need to type a full path.
- `create_dir_all` makes the operation robust even if intermediate directories
  are somehow absent.
- Duplicate-name detection keeps the popup open for immediate correction.
- The explorer reloads automatically; no manual `R` is needed.

**Negative / trade-offs**
- `Mode::NewFolder` adds another mode to the mode graph (now 15 modes).
- Only one directory level can be created per invocation.

---

## Mode graph addition

```
Explorer  ── m ──► NewFolder
NewFolder ── Enter ──► Explorer  (folder created)
          ── Esc   ──► Explorer  (cancelled)
```
