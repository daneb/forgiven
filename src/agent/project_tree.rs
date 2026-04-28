use super::tools::extract_symbols;

/// Build a compact, indented file tree of `root` to `max_depth` levels.
/// Hidden files and noisy directories (target, node_modules, …) are skipped.
pub fn build_project_tree(root: &std::path::Path, max_depth: usize) -> String {
    let mut out = String::new();
    tree_recursive(root, root, 0, max_depth, &mut out);
    out
}

#[allow(clippy::only_used_in_recursion)]
fn tree_recursive(
    root: &std::path::Path,
    path: &std::path::Path,
    depth: usize,
    max_depth: usize,
    out: &mut String,
) {
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|e| {
        // dirs first, then files, both alphabetical
        let is_file = e.path().is_file();
        (is_file, e.file_name().to_string_lossy().to_lowercase())
    });
    // Pre-build indent once per level (depth ≤ max_depth, typically ≤ 2).
    let indent = "  ".repeat(depth);
    for entry in items {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name.starts_with('.') {
            continue; // skip hidden
        }
        if matches!(name.as_ref(), "target" | "node_modules" | "dist" | "build" | ".git") {
            continue;
        }
        if entry.path().is_dir() {
            // Push directly onto `out` — avoids the intermediate String that
            // `format!("{indent}{name}/\n")` would allocate.
            out.push_str(&indent);
            out.push_str(&name);
            out.push_str("/\n");
            tree_recursive(root, &entry.path(), depth + 1, max_depth, out);
        } else {
            out.push_str(&indent);
            out.push_str(&name);
            out.push('\n');
        }
    }
}

/// Build a compact structural map of `src/` files: one line per file listing
/// up to `MAX_NAMES` top-level symbol names.  Gives the model symbol-level
/// orientation at ≈200–400 tokens instead of 300–500 for the filename tree,
/// saving 1–2 `read_file` discovery round-trips per session.
pub fn build_structural_map(root: &std::path::Path) -> String {
    const MAX_NAMES: usize = 8;

    let src_root = root.join("src");
    let mut lines =
        vec!["Structural map (src/ — call get_file_outline for full details):".to_string()];

    // Collect all .rs files under src/ (depth-unlimited but src/ is small).
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    collect_rs_files_recursive(&src_root, &mut paths);
    paths.sort();

    for path in paths {
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        let symbols = extract_symbols(&src);
        if symbols.is_empty() {
            continue;
        }
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        let shown = &names[..names.len().min(MAX_NAMES)];
        let extra = names.len().saturating_sub(MAX_NAMES);
        let suffix = if extra > 0 { format!(" … +{extra}") } else { String::new() };
        lines.push(format!("  {} — {}{}", rel.display(), shown.join(", "), suffix));
    }
    lines.join("\n")
}

fn collect_rs_files_recursive(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files_recursive(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}
