# ADR 0015 — File Creation and Explorer Enhancements

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

The editor had no way to create new files from within the UI. The only path to
working on a new file was to create it in a separate shell and then open it with
the fuzzy finder (`SPC f f`) or by restarting the editor with the path as an
argument.

This broke the self-contained editing workflow, particularly when using the
agent to scaffold new modules — the agent could create a file on disk, but the
user had no in-editor way to then create sibling files alongside it.

Additional gaps in the file explorer:
- No way to add a new file relative to the currently selected directory.
- No way to refresh the tree after files were created on disk (e.g. by the agent
  or an external tool).

---

## Decision

### 1. `:e <path>` command (vim-compatible)

The `execute_command()` function gained a new arm for commands starting with
`"e "` or `"edit "`:

```rust
_ if cmd.starts_with("e ") || cmd.starts_with("edit ") => {
    let path_str = cmd.splitn(2, ' ').nth(1).unwrap_or("").trim();
    let path = if Path::new(path_str).is_absolute() {
        PathBuf::from(path_str)
    } else {
        current_dir().join(path_str)   // relative to project root
    };
    self.open_file(&path)?;
    if self.file_explorer.visible {
        self.file_explorer.reload();   // show new entry immediately
    }
}
```

`open_file()` already handles non-existent paths — it creates an empty named
buffer with `file_path` set. The file is not written to disk until `:w`.

This mirrors vim's `:e` / `:edit` command, so it is immediately familiar. Both
relative (from project root) and absolute paths are supported.

### 2. `SPC f n` — new file leader binding

A `FileNew` action was added to the `Action` enum and wired to `SPC f n` in the
leader key tree:

```rust
file_node.children.insert('n', KeyNode::leaf("new file", Action::FileNew));
```

`execute_action(FileNew)` enters Command mode pre-filled with `"e "`:

```rust
Action::FileNew => {
    self.command_buffer = "e ".to_string();
    self.mode = Mode::Command;
}
```

The user sees `:e ` in the command bar and types the path.

### 3. `n` in Explorer mode — context-aware new file

When the file explorer is focused, pressing `n` switches to Command mode
pre-filled with the directory currently under the cursor:

```
:e src/
```

The user only needs to type the filename and press Enter:

```
:e src/my_new_module.rs
```

**Directory resolution logic:**
- If the cursor is on a directory node → use that directory.
- If the cursor is on a file node → use its parent directory.
- If nothing is selected → use the explorer root.

The prefix is expressed as a project-relative path for readability (no absolute
paths in the command bar).

```rust
KeyCode::Char('n') => {
    let target_dir = explorer.selected_path()
        .map(|p| if p.is_dir() { p } else { p.parent()... })
        .unwrap_or(explorer.root_path.clone());

    let rel = target_dir
        .strip_prefix(&explorer.root_path)
        .to_string_lossy();

    let prefill = if rel.is_empty() { "e ".into() }
                  else { format!("e {rel}/") };

    self.command_buffer = prefill;
    self.mode = Mode::Command;
}
```

### 4. `r` in Explorer mode — manual refresh

Pressing `r` while the explorer is focused calls `FileExplorer::reload()` and
shows a status message.

This is a simple escape hatch for cases where the tree is stale: agent creates
files, external tools create files, or `git checkout` changes the working tree.

### 5. `FileExplorer::reload()`

A new public method on `FileExplorer`:

```rust
pub fn reload(&mut self) {
    self.root_nodes = load_dir(&self.root_path, 0);
    self.root_loaded = true;
    // Clamp cursor so it doesn't point past the end of the new list.
    let len = self.flat_visible().len();
    if len > 0 { self.cursor_idx = self.cursor_idx.min(len - 1); }
}
```

This discards all expanded/collapsed state and re-scans from the root.
It is also called automatically by `:e` when the explorer is visible, so that
the new file's parent directory appears in the tree immediately after the buffer
is created (the file itself won't appear until `:w` writes it to disk).

---

## Workflow Examples

**Create a new Rust module from scratch:**
```
SPC f n          → command bar shows ":e "
src/utils.rs     → type filename
Enter            → empty buffer for src/utils.rs opens
i                → enter Insert mode
...              → write the code
:w               → saves src/utils.rs to disk
SPC e e, r       → refresh explorer (or wait for next 'r' press)
```

**Create a file next to an existing one (explorer workflow):**
```
SPC e e          → open explorer
j                → navigate to src/ directory
n                → command bar pre-fills ":e src/"
helpers.rs       → type filename
Enter            → empty buffer opens
```

**Vim-style `:edit`:**
```
:edit tests/integration_test.rs    → works with full or relative path
```

---

## Consequences

- **Parent directories must exist**: `buf.save()` calls `std::fs::write()`, which
  requires the parent directory to exist. If the user types `:e new_dir/file.rs`
  and `new_dir/` does not exist, `:w` will fail with an OS error. A future
  improvement could create missing parent directories automatically (`mkdir -p`).
- **No inline rename**: the same mechanism could support `:e` as a rename/move
  by opening the new path and saving, but the old file would remain. Rename is
  not yet implemented.
- **Explorer expand state lost on reload**: `FileExplorer::reload()` discards
  expanded/collapsed state. For large projects this means the user has to
  re-expand directories after refreshing. A smarter reload that preserves
  expanded paths (by storing them in a `HashSet<PathBuf>`) is tracked as a
  future improvement.
- **No auto-refresh**: the explorer does not watch the filesystem; it only
  refreshes on explicit `r` or after `:e`. ADR 0017 tracks adding a filesystem
  watcher (`notify` crate) for automatic refresh.
