//! File explorer tree sidebar.
//!
//! The explorer shows a collapsible directory tree on the left side of the screen.
//! Directories are expanded/collapsed lazily on first open. Hidden directories and
//! build artefact directories are skipped automatically.

use std::path::{Path, PathBuf};

// ── Skip list ──────────────────────────────────────────────────────────────────

const SKIP_DIRS: &[&str] = &[
    ".git", "target", "node_modules", "dist", "build", ".next",
    "__pycache__", ".cache", ".idea", ".vscode",
];

const SKIP_HIDDEN: bool = true; // skip dotfiles / dot-dirs (except .git already in list)

fn should_skip(name: &str) -> bool {
    if SKIP_DIRS.contains(&name) {
        return true;
    }
    if SKIP_HIDDEN && name.starts_with('.') {
        return true;
    }
    false
}

// ── Data types ─────────────────────────────────────────────────────────────────

/// A single entry in the file tree.
#[derive(Debug, Clone)]
pub struct FileNode {
    /// Absolute path to this entry.
    pub path: PathBuf,
    /// Display name (file/directory name only, not the full path).
    pub name: String,
    pub is_dir: bool,
    /// Whether a directory's children have been loaded.
    pub children_loaded: bool,
    pub is_expanded: bool,
    pub children: Vec<FileNode>,
    /// Indent depth (0 = root level).
    pub depth: usize,
}

impl FileNode {
    fn new_dir(path: PathBuf, name: String, depth: usize) -> Self {
        Self {
            path,
            name,
            is_dir: true,
            children_loaded: false,
            is_expanded: false,
            children: Vec::new(),
            depth,
        }
    }

    fn new_file(path: PathBuf, name: String, depth: usize) -> Self {
        Self {
            path,
            name,
            is_dir: false,
            children_loaded: true,
            is_expanded: false,
            children: Vec::new(),
            depth,
        }
    }
}

// ── FileExplorer ───────────────────────────────────────────────────────────────

pub struct FileExplorer {
    pub visible: bool,
    pub focused: bool,
    /// Root directory being shown.
    pub root_path: PathBuf,
    /// Top-level entries (children of root).
    pub root_nodes: Vec<FileNode>,
    pub root_loaded: bool,
    /// Index into the *flat* visible list produced by `flat_visible()`.
    pub cursor_idx: usize,
}

impl FileExplorer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            visible: false,
            focused: false,
            root_path: root,
            root_nodes: Vec::new(),
            root_loaded: false,
            cursor_idx: 0,
        }
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.focused = true;
            if !self.root_loaded {
                self.load_root();
            }
        } else {
            self.focused = false;
        }
    }

    pub fn focus(&mut self) {
        self.focused = true;
        self.visible = true;
        if !self.root_loaded {
            self.load_root();
        }
    }

    pub fn blur(&mut self) {
        self.focused = false;
    }

    // ── Loading ────────────────────────────────────────────────────────────────

    fn load_root(&mut self) {
        self.root_nodes = load_dir(&self.root_path, 0);
        self.root_loaded = true;
    }

    /// Re-scan the root directory, discarding all cached expand/collapse state.
    /// Call this after files are created or deleted on disk.
    pub fn reload(&mut self) {
        self.root_nodes = load_dir(&self.root_path, 0);
        self.root_loaded = true;
        // Keep cursor in bounds after the reload.
        let len = self.flat_visible().len();
        if len > 0 {
            self.cursor_idx = self.cursor_idx.min(len - 1);
        }
    }

    /// Expand or collapse the node at `flat_idx` in the flat visible list.
    pub fn toggle_node_at(&mut self, flat_idx: usize) {
        // Walk the tree to find the node at position `flat_idx`.
        let path = {
            let flat = self.flat_visible();
            flat.get(flat_idx).map(|n| n.path.clone())
        };
        if let Some(p) = path {
            toggle_in_list(&mut self.root_nodes, &p);
        }
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    pub fn move_up(&mut self) {
        self.cursor_idx = self.cursor_idx.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        let len = self.flat_visible().len();
        if len > 0 {
            self.cursor_idx = (self.cursor_idx + 1).min(len - 1);
        }
    }

    /// Return the path selected by the cursor, if it's a file (not a directory).
    pub fn selected_file(&self) -> Option<PathBuf> {
        let flat = self.flat_visible();
        flat.get(self.cursor_idx).and_then(|n| {
            if n.is_dir { None } else { Some(n.path.clone()) }
        })
    }

    /// Return the path selected by the cursor regardless of type.
    pub fn selected_path(&self) -> Option<PathBuf> {
        let flat = self.flat_visible();
        flat.get(self.cursor_idx).map(|n| n.path.clone())
    }

    // ── Flat rendering list ────────────────────────────────────────────────────

    /// Flatten the visible tree into a single list for rendering and cursor tracking.
    pub fn flat_visible(&self) -> Vec<&FileNode> {
        let mut out = Vec::new();
        flatten_nodes(&self.root_nodes, &mut out);
        out
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Load one level of directory entries, sorted (dirs first, then files).
fn load_dir(path: &Path, depth: usize) -> Vec<FileNode> {
    let mut dirs: Vec<FileNode> = Vec::new();
    let mut files: Vec<FileNode> = Vec::new();

    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if should_skip(&name) {
            continue;
        }

        if p.is_dir() {
            dirs.push(FileNode::new_dir(p, name, depth));
        } else {
            files.push(FileNode::new_file(p, name, depth));
        }
    }

    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));
    dirs.extend(files);
    dirs
}

/// Recursively flatten visible nodes into `out`.
fn flatten_nodes<'a>(nodes: &'a [FileNode], out: &mut Vec<&'a FileNode>) {
    for node in nodes {
        out.push(node);
        if node.is_dir && node.is_expanded {
            flatten_nodes(&node.children, out);
        }
    }
}

/// Walk the tree and toggle the node whose path matches `target`.
fn toggle_in_list(nodes: &mut Vec<FileNode>, target: &Path) -> bool {
    for node in nodes.iter_mut() {
        if node.path == target {
            if node.is_dir {
                node.is_expanded = !node.is_expanded;
                if node.is_expanded && !node.children_loaded {
                    node.children = load_dir(&node.path, node.depth + 1);
                    node.children_loaded = true;
                }
            }
            return true;
        }
        if node.is_dir && node.is_expanded {
            if toggle_in_list(&mut node.children, target) {
                return true;
            }
        }
    }
    false
}
