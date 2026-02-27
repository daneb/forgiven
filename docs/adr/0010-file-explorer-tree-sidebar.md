# ADR 0010 — File Explorer Tree Sidebar

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

The existing file-open workflow required `SPC f f` to open a flat fuzzy-picker that
scanned the entire working directory recursively. This had two shortcomings:

1. **No project overview.** Developers rely on a persistent tree to understand
   project structure — flat lists do not convey directory hierarchy.
2. **No lazy navigation.** The flat scan always read all files upfront, becoming
   slow in large repositories, and provided no way to incrementally explore
   subdirectories.

Two designs were considered:

| Design | Pros | Cons |
|--------|------|------|
| **Enhanced flat picker** (fuzzy search + grouping) | Familiar to VS Code / Telescope users; requires no layout change | Still no persistent tree view; harder to understand project layout at a glance |
| **Tree sidebar** (VS Code / NeoTree style) | Persistent project view; lazy loading; keyboard-first navigation | Requires a new layout column; more implementation surface |

The tree sidebar was chosen as it matches the expected mental model for an
IDE-style editor and pairs naturally with the already-present agent panel.

## Decision

### Data model (`src/explorer/mod.rs`)

```rust
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,          // display name only
    pub is_dir: bool,
    pub children_loaded: bool, // false until first expand
    pub is_expanded: bool,
    pub children: Vec<FileNode>,
    pub depth: usize,          // 0 = root level
}

pub struct FileExplorer {
    pub visible: bool,
    pub focused: bool,
    pub root_path: PathBuf,
    pub root_nodes: Vec<FileNode>,
    pub root_loaded: bool,
    pub cursor_idx: usize,     // index into flat_visible()
}
```

### Lazy directory loading

Children are only read from disk when a directory node is first expanded:

```rust
fn toggle_in_list(nodes: &mut Vec<FileNode>, target: &Path) -> bool {
    // find the node, then:
    node.is_expanded = !node.is_expanded;
    if node.is_expanded && !node.children_loaded {
        node.children = load_dir(&node.path, node.depth + 1);
        node.children_loaded = true;
    }
}
```

`load_dir()` reads one level of a directory, sorts entries (directories first,
then files, both alphabetically), and skips:
- Hidden files and directories (names starting with `.`)
- Common build artefact directories: `target`, `node_modules`, `dist`, `build`,
  `.next`, `__pycache__`, `.cache`, `.idea`, `.vscode`

### Flat visible list

The tree is collapsed into a linear list for cursor-indexed rendering:

```rust
pub fn flat_visible(&self) -> Vec<&FileNode> { ... }

fn flatten_nodes<'a>(nodes: &'a [FileNode], out: &mut Vec<&'a FileNode>) {
    for node in nodes {
        out.push(node);
        if node.is_dir && node.is_expanded {
            flatten_nodes(&node.children, out);
        }
    }
}
```

`cursor_idx` is an index into this flat list. All navigation operations
(`move_up`, `move_down`, `toggle_node_at`) are expressed in terms of this index.

### Layout

When the explorer is visible a 25-column left panel is added to the horizontal
split. The three-way layout logic in `UI::render()`:

```
explorer visible + agent visible  →  [25 cols] | [Min(1)] | [35%]
explorer visible only             →  [25 cols] | [Min(1)]
agent visible only                →  [60%]     | [40%]
neither                           →  [full width]
```

### Rendering (`render_file_explorer`)

Each visible `FileNode` is rendered as a single `Line`:

```
"  ▼ src/"          depth=0 expanded directory
"    ▶ editor/"     depth=1 collapsed directory
"      mod.rs"      depth=2 file
```

- **Directories** are coloured `Color::Cyan`
- **Files** are coloured `Color::White`
- **Selected row** uses `bg(Color::Blue).fg(Color::White).BOLD`
- The panel scrolls to keep `cursor_idx` in view at all times
- The block title shows the root directory name; the border is `LightGreen` when
  focused and `DarkGray` when not

### Mode and keybindings

A new `Mode::Explorer` is added to the mode enum. Focus is managed by two new
`Action` variants wired to leader keys:

```
SPC e e  →  ExplorerToggle  (open/close; focuses on open)
SPC e f  →  ExplorerFocus   (open if hidden, then focus)
```

In `Mode::Explorer` a dedicated `handle_explorer_mode()` handler processes:

| Key | Action |
|-----|--------|
| `k` / `↑` | Move cursor up |
| `j` / `↓` | Move cursor down |
| `Enter` / `l` | Expand/collapse directory; open file and return to Normal mode |
| `Esc` / `Tab` | Return focus to editor; panel stays visible |

### Integration with `Editor`

`FileExplorer` is a field of `Editor` initialised from `std::env::current_dir()`.
When a file is opened via the explorer the existing `open_file()` path is reused,
ensuring the LSP `textDocument/didOpen` notification is sent automatically.

## Consequences

- The explorer and the flat file picker (`SPC f f`) coexist; they serve different
  workflows (browse vs search)
- Lazy loading means opening the editor on a large monorepo is fast — the tree
  only reads directories that the user explicitly expands
- The flat visible list is rebuilt on every `flat_visible()` call. For typical
  project sizes (hundreds of expanded nodes) this is negligible; for very large
  expanded trees a cached dirty-flag approach could be added
- The 25-column width is hard-coded; making it resizable (drag or user config) is
  a future enhancement
- There is no file-system watch: new files created outside the editor do not
  appear in the tree until the user closes and re-expands the parent directory.
  A `notify`-based inotify/FSEvents watcher is a planned future addition
- `Mode::Explorer` is excluded from the normal `KeyHandler` path — explorer key
  handling lives entirely in `handle_explorer_mode()`, keeping navigation logic
  self-contained and avoiding accidental leader-sequence conflicts
