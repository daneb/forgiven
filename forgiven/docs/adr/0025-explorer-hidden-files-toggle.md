# ADR 0025 — Explorer Hidden Files Toggle

**Date:** 2026-02-24
**Status:** Accepted

---

## Context

The file explorer implemented in ADR 0010 automatically skips hidden files and
directories (those starting with `.`) during directory scanning. This behaviour
was hard-coded in the `should_skip()` filter function — there was no way for
users to view configuration files like `.gitignore`, `.env`, or `.github/`
workflows without opening them via `:e` or the fuzzy finder.

While hiding dot-files by default reduces clutter, many workflows require
frequent access to these files:
- Editing `.env` for local configuration
- Reviewing `.github/workflows/` CI definitions
- Modifying `.prettierrc`, `.eslintrc`, or other tool configs
- Inspecting `.git/hooks/` or other version-control metadata

Two designs were considered:

| Design | Pros | Cons |
|--------|------|------|
| **Always show hidden files** | Simple — no toggle needed | Clutters the tree; many users never need to see them |
| **Toggle visibility** (default hidden) | Preserves clean default view; opt-in when needed | Requires new action, keybinding, and state management |

The toggle approach was chosen to maintain the uncluttered default experience
while providing an escape hatch for users who need to work with hidden files.

---

## Decision

### 1. State tracking in `FileExplorer`

A new `show_hidden: bool` field was added to the `FileExplorer` struct
(default: `false`):

```rust
pub struct FileExplorer {
    pub visible: bool,
    pub focused: bool,
    pub root_path: PathBuf,
    pub root_nodes: Vec<FileNode>,
    pub root_loaded: bool,
    pub cursor_idx: usize,
    pub show_hidden: bool,  // ← new field
}
```

The existing `should_skip()` filter now respects this flag:

```rust
fn should_skip(name: &str, is_dir: bool, show_hidden: bool) -> bool {
    if !show_hidden && name.starts_with('.') { return true; }
    if is_dir { SKIP_DIRS.contains(&name) } else { false }
}
```

### 2. `toggle_hidden()` method

A new public method on `FileExplorer` flips the flag and reloads the tree:

```rust
pub fn toggle_hidden(&mut self) {
    self.show_hidden = !self.show_hidden;
    self.reload();  // Re-scan directories with the new filter
}
```

This ensures that hidden files appear/disappear immediately when the toggle is
triggered. The `reload()` call (from ADR 0015) discards all expanded state and
re-scans from the root with the new filter applied.

### 3. Action and key binding

A new `Action::ExplorerToggleHidden` was added to the `Action` enum and wired
to two different contexts:

#### a) `SPC e h` — Leader key (Normal mode)

Added to the explorer submenu of the leader key tree:

```rust
let mut explorer_node = KeyNode::branch("explorer");
explorer_node.children.insert('e', ...);
explorer_node.children.insert('f', ...);
explorer_node.children.insert('h', KeyNode::leaf(
    "toggle hidden files",
    Action::ExplorerToggleHidden
));
```

This allows toggling from anywhere in the editor via the which-key menu.

#### b) `h` — Direct binding in Explorer mode

Added to `handle_explorer_mode()`:

```rust
Mode::Explorer => match key.code {
    KeyCode::Char('h') => {
        self.file_explorer.toggle_hidden();
        let status = if self.file_explorer.show_hidden {
            "Showing hidden files"
        } else {
            "Hiding hidden files"
        };
        self.set_status(status.to_string());
    }
    // ... existing j/k/Enter/Esc handlers
}
```

This provides immediate access when the explorer is focused, without needing
to navigate the leader key menu.

### 4. Action handler in `execute_action()`

The `execute_action()` dispatch includes a handler for the action:

```rust
Action::ExplorerToggleHidden => {
    self.file_explorer.toggle_hidden();
    let status = if self.file_explorer.show_hidden {
        "Explorer: showing hidden files"
    } else {
        "Explorer: hiding hidden files"
    };
    self.set_status(status.to_string());
}
```

This handler is invoked by the `SPC e h` leader binding.

---

## Workflow Examples

**Toggle hidden files via leader key:**
```
SPC e h              → status bar shows "Explorer: showing hidden files"
                      tree now includes .github/, .env, etc.
SPC e h              → status bar shows "Explorer: hiding hidden files"
                      dot-files disappear
```

**Toggle from within explorer:**
```
SPC e e              → open and focus explorer
j j k                → navigate around
h                    → status bar shows "Showing hidden files"
                      tree refreshes; .gitignore now visible
```

---

## Consequences

- **Expanded state lost on toggle**: because `toggle_hidden()` calls `reload()`,
  all expanded/collapsed state is discarded when toggling. For large projects
  this means the user must re-expand directories after toggling. A future
  enhancement (tracked in the backlog) could preserve expanded paths across
  reload by storing them in a `HashSet<PathBuf>` and re-applying them after
  the tree is re-scanned.
- **Hidden state not persisted**: the `show_hidden` flag is reset to `false`
  every time the editor is restarted. A config file option (e.g.
  `explorer.show_hidden_by_default`) could be added if users request a way to
  default to showing hidden files.
- **No granular filter**: the toggle is binary — either all dot-files are shown
  or none are. A more granular approach (e.g. skip `.git/` but show `.github/`,
  or show only dot-files matching a pattern) is not currently supported.
- **Consistent with conventions**: the `h` key was chosen because it is
  mnemonic ("h" for "hidden") and does not conflict with existing explorer
  bindings (`j`/`k` for navigation, `l`/`Enter` for open, `n` for new file,
  `r` for refresh). It also aligns with common file manager conventions (e.g.
  Ranger uses `zh` to toggle hidden files).
- **Status feedback**: both the `h` key and `SPC e h` provide immediate status
  bar feedback, so users always know the current state of the toggle.
