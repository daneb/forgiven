# ADR 0013 — Multi-Project Support: Project Folder Argument

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

The editor previously had no way to open a different project. `FileExplorer`,
the LSP workspace root, agent `project_root`, `scan_files`, and
`build_project_tree` all anchored themselves to `std::env::current_dir()` at
the time they were called. This meant the editor always started in whichever
directory the shell was in when it was launched.

Typical workflows like:

```sh
cd ~/work/project-a
forgiven                         # opens project-a ✓
forgiven ~/work/project-b        # expected: open project-b — but always opened project-a ✗
```

were impossible without first `cd`-ing to the target directory.

---

## Decision

### Single `set_current_dir()` call in `main()`

All internal state that depends on the project root uses `current_dir()` lazily
at the moment it is needed. Therefore, calling `std::env::set_current_dir()`
**once**, before `Editor::new()` is called, is sufficient to make every
downstream consumer point at the new root — with zero changes to any of them.

### CLI changes (`src/main.rs`)

```rust
struct Cli {
    /// Project folder to open (overrides the current directory).
    #[arg(short = 'C', long = "dir", value_name = "DIR")]
    dir: Option<PathBuf>,

    /// File(s) or directory to open on startup.
    /// If the first positional argument is a directory it is used as
    /// the project root (equivalent to -C).
    files: Vec<PathBuf>,
}
```

Two invocation styles are supported:

| Style | Example |
|-------|---------|
| Explicit flag (mirrors `git -C`, `make -C`) | `forgiven -C ~/work/myapp` |
| Positional directory | `forgiven ~/work/myapp` |
| Positional directory + files | `forgiven ~/work/myapp src/main.rs` |
| Files only (no change) | `forgiven src/main.rs` |
| No arguments (no change) | `forgiven` |

```rust
// Separate directory positional args from file args
let mut project_dir: Option<PathBuf> = cli.dir;
let mut files_to_open: Vec<PathBuf> = Vec::new();

for path in cli.files {
    if path.is_dir() {
        if project_dir.is_none() { project_dir = Some(path); }
    } else {
        files_to_open.push(path);
    }
}

// Canonicalize and chdir before Editor::new()
if let Some(ref dir) = project_dir {
    let canonical = dir.canonicalize()?;
    std::env::set_current_dir(&canonical)?;
}
```

### What `set_current_dir` fixes automatically

| Component | Where `current_dir()` is called |
|-----------|--------------------------------|
| `FileExplorer` | `Editor::new()` → `FileExplorer::new(current_dir()...)` |
| LSP workspace root | `setup_lsp()` → `LspManager::add_server(workspace_root)` |
| Agent `project_root` | `handle_agent_mode()` → `panel.submit(..., current_dir())` |
| `build_project_tree` | inside `AgentPanel::submit()` |
| `scan_files` (fuzzy find) | `Action::FileFind` handler |

All five consumers are correct after the single `chdir` with no further changes.

### Error handling

The path is canonicalized before `chdir`. If the directory does not exist or is
not accessible, the editor exits with a clear message before the TUI starts:

```
Error: Cannot open directory: /nonexistent
Caused by: No such file or directory (os error 2)
```

---

## Consequences

- **Process-wide side effect**: `set_current_dir()` affects the entire process.
  Any future code that creates relative paths will be relative to the project
  root, which is the desired behaviour.
- **Shell working directory unchanged**: the calling shell's cwd is unaffected;
  only the editor process changes its own cwd.
- **Multiple project roots not supported**: only the first directory positional
  argument is treated as the root; subsequent ones are silently ignored. Opening
  two projects simultaneously requires two editor instances.
- **Symlinks resolved**: `canonicalize()` resolves symlinks, so
  `forgiven ~/link-to-project` opens the real path. This ensures consistency
  with `FileExplorer` and path-matching logic elsewhere.
