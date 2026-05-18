use super::tools::extract_symbols;

/// Source file extensions included in the structural map and repo-map.
const SUPPORTED_EXTENSIONS: &[&str] =
    &["rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt", "rb", "cs", "cpp", "c", "h"];

/// Directories always excluded from scanning (noisy, not source).
const SKIP_DIRS: &[&str] =
    &["target", "node_modules", "dist", "build", ".git", "__pycache__", ".venv", "venv"];

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

/// Recursively collect all source files under `dir` whose extension is in
/// `SUPPORTED_EXTENSIONS`.  Hidden files and `SKIP_DIRS` are excluded.
pub(crate) fn collect_source_files_recursive(
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            collect_source_files_recursive(&path, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if SUPPORTED_EXTENSIONS.contains(&ext) {
                out.push(path);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// P2-S2/S3: Reference graph + PageRank
// ─────────────────────────────────────────────────────────────────────────────

/// Build a directed reference graph over `paths`.
///
/// For each file we extract its exported symbol names, then scan every *other*
/// file's raw source for whole-word occurrences of those names.  A match
/// produces a directed edge: referencing_file → defining_file.
///
/// Returns an adjacency list indexed by position in `paths`:
/// `graph[i]` = set of file indices that file `i` references.
fn build_reference_graph(paths: &[std::path::PathBuf], sources: &[String]) -> Vec<Vec<usize>> {
    let n = paths.len();
    // symbol_name → defining file index (first definition wins)
    let mut sym_to_file: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, src) in sources.iter().enumerate() {
        for sym in extract_symbols(src) {
            sym_to_file.entry(sym.name).or_insert(i);
        }
    }

    let mut graph: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, src) in sources.iter().enumerate() {
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (sym_name, &def_file) in &sym_to_file {
            if def_file == i {
                continue; // skip self-references
            }
            // Whole-word scan: require non-alphanumeric/underscore boundary.
            if word_occurs(src, sym_name) && seen.insert(def_file) {
                graph[i].push(def_file);
            }
        }
    }
    graph
}

/// True if `needle` appears in `haystack` as a whole identifier (surrounded
/// by non-word characters or string boundaries).
fn word_occurs(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let nbytes = needle.as_bytes();
    let nlen = nbytes.len();
    if nlen == 0 || nlen > bytes.len() {
        return false;
    }
    let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0usize;
    while let Some(pos) = bytes[start..].windows(nlen).position(|w| w == nbytes) {
        let abs = start + pos;
        let pre_ok = abs == 0 || !is_word_char(bytes[abs - 1]);
        let post_ok = abs + nlen >= bytes.len() || !is_word_char(bytes[abs + nlen]);
        if pre_ok && post_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Run PageRank over the reference graph.
///
/// - `graph[i]` = outgoing edges from node i
/// - damping = 0.85, convergence threshold = 1e-6, max 100 iterations
/// - Returns a score per node (higher = more referenced).
fn pagerank(graph: &[Vec<usize>]) -> Vec<f64> {
    let n = graph.len();
    if n == 0 {
        return Vec::new();
    }
    const DAMPING: f64 = 0.85;
    const THRESHOLD: f64 = 1e-6;
    const MAX_ITER: usize = 100;

    // Build transpose (incoming edges) for efficient iteration.
    let mut in_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut out_deg: Vec<usize> = vec![0; n];
    for (i, outs) in graph.iter().enumerate() {
        out_deg[i] = outs.len();
        for &j in outs {
            in_edges[j].push(i);
        }
    }

    let mut rank = vec![1.0f64 / n as f64; n];
    let base = (1.0 - DAMPING) / n as f64;

    for _ in 0..MAX_ITER {
        let mut next = vec![base; n];
        for j in 0..n {
            for &i in &in_edges[j] {
                if out_deg[i] > 0 {
                    next[j] += DAMPING * rank[i] / out_deg[i] as f64;
                }
            }
        }
        // Handle dangling nodes: distribute their rank evenly.
        let dangling_sum: f64 = (0..n).filter(|&i| out_deg[i] == 0).map(|i| rank[i]).sum();
        if dangling_sum > 0.0 {
            let spread = DAMPING * dangling_sum / n as f64;
            for v in &mut next {
                *v += spread;
            }
        }
        let delta: f64 = rank.iter().zip(&next).map(|(a, b)| (a - b).abs()).sum();
        rank = next;
        if delta < THRESHOLD {
            break;
        }
    }
    rank
}

/// Build a PageRank-ordered repo map capped at `max_tokens` (estimated as
/// `chars / 4`).  Returns a string ready to inject into the system prompt.
///
/// Format mirrors `build_structural_map` but files appear highest-rank first.
pub fn build_ranked_repo_map(root: &std::path::Path, max_tokens: usize) -> String {
    const MAX_NAMES: usize = 8;
    // chars budget: 1 token ≈ 4 chars
    let char_budget = max_tokens * 4;

    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    collect_source_files_recursive(root, &mut paths);
    paths.sort();

    if paths.is_empty() {
        return "Repo map: (no source files found)".to_string();
    }

    // Read sources once; skip unreadable files.
    let sources: Vec<String> =
        paths.iter().map(|p| std::fs::read_to_string(p).unwrap_or_default()).collect();

    let graph = build_reference_graph(&paths, &sources);
    let scores = pagerank(&graph);

    // Sort indices by descending PageRank score.
    let mut order: Vec<usize> = (0..paths.len()).collect();
    order.sort_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal));

    let header = "Ranked repo map (highest PageRank first — use get_file_outline for detail):";
    let mut lines = vec![header.to_string()];
    let mut used_chars = header.len() + 1;

    for idx in order {
        let path = &paths[idx];
        let rel = path.strip_prefix(root).unwrap_or(path);
        let syms = extract_symbols(&sources[idx]);
        if syms.is_empty() {
            continue;
        }
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        let shown = &names[..names.len().min(MAX_NAMES)];
        let extra = names.len().saturating_sub(MAX_NAMES);
        let suffix = if extra > 0 { format!(" … +{extra}") } else { String::new() };
        let line = format!("  {} — {}{}", rel.display(), shown.join(", "), suffix);
        if used_chars + line.len() + 1 > char_budget {
            break;
        }
        used_chars += line.len() + 1;
        lines.push(line);
    }
    lines.join("\n")
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tree_tests {
    use super::*;
    use std::fs;

    fn make_tree() -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(format!(
            "forgiven_tree_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let r = &tmp;
        fs::create_dir_all(r.join("src")).unwrap();
        fs::create_dir_all(r.join("scripts")).unwrap();
        fs::create_dir_all(r.join("target/debug")).unwrap();
        fs::write(r.join("src/main.rs"), "pub fn main() {}").unwrap();
        fs::write(r.join("src/lib.rs"), "pub fn helper() {}").unwrap();
        fs::write(r.join("scripts/build.py"), "def build(): pass").unwrap();
        fs::write(r.join("index.ts"), "export function init() {}").unwrap();
        fs::write(r.join("target/debug/binary"), "binary").unwrap();
        tmp
    }

    #[test]
    fn collects_multi_language_files() {
        let tmp = make_tree();
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        collect_source_files_recursive(&tmp, &mut paths);

        let names: Vec<&str> =
            paths.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();

        assert!(names.contains(&"main.rs"), "should include .rs");
        assert!(names.contains(&"build.py"), "should include .py");
        assert!(names.contains(&"index.ts"), "should include .ts");
        assert!(!names.contains(&"binary"), "target/ must be excluded");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ranked_map_includes_all_languages() {
        let tmp = make_tree();
        let map = build_ranked_repo_map(&tmp, 4096);

        assert!(map.contains("main.rs"), "map must list main.rs");
        assert!(map.contains("build.py"), "map must list build.py");
        assert!(map.contains("index.ts"), "map must list index.ts");
        assert!(map.contains("main"), "main.rs symbol present");
        assert!(map.contains("build"), "build.py symbol present");
        assert!(map.contains("init"), "index.ts symbol present");

        let _ = fs::remove_dir_all(&tmp);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// P2-S6: PageRank + reference graph tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod pagerank_tests {
    use super::*;

    /// PageRank on an acyclic graph must converge and sum to ≈1.0.
    #[test]
    fn pagerank_stable_on_acyclic_graph() {
        // A → B → C  (no cycles)
        let graph = vec![
            vec![1usize], // 0 → 1
            vec![2usize], // 1 → 2
            vec![],       // 2 (sink)
        ];
        let scores = pagerank(&graph);
        assert_eq!(scores.len(), 3);
        let sum: f64 = scores.iter().sum();
        // PageRank with dangling-node redistribution sums to ≈1.0.
        assert!((sum - 1.0).abs() < 1e-4, "scores should sum to ~1.0, got {sum}");
        // C is the sink — it accumulates the most rank.
        assert!(scores[2] > scores[0], "sink node should rank higher than source: {:?}", scores);
    }

    /// A node with more incoming edges should rank higher than one with fewer.
    #[test]
    fn pagerank_ranking_matches_reference_counts() {
        // Both 0 and 1 point to 2; nobody points to 3.
        let graph = vec![
            vec![2usize], // 0 → 2
            vec![2usize], // 1 → 2
            vec![],       // 2 (two incoming)
            vec![],       // 3 (zero incoming)
        ];
        let scores = pagerank(&graph);
        assert!(
            scores[2] > scores[3],
            "node with 2 inbound refs should outrank node with 0: {:?}",
            scores
        );
    }

    /// `word_occurs` must respect word boundaries.
    #[test]
    fn word_occurs_boundary() {
        assert!(word_occurs("foo bar baz", "bar"));
        assert!(word_occurs("foo.bar()", "bar"), "dot is not a word char");
        assert!(word_occurs("(bar)", "bar"), "paren boundary");
        assert!(!word_occurs("call_bar()", "bar"), "underscore is a word char — no boundary");
        assert!(!word_occurs("foobar", "bar"), "no boundary before");
        assert!(!word_occurs("bar_baz", "bar"), "no boundary after");
        assert!(word_occurs("bar", "bar"), "exact match");
    }

    /// build_reference_graph should produce an edge from the caller to the
    /// definer when the definer's symbol name appears in the caller's source.
    #[test]
    fn reference_graph_detects_cross_file_reference() {
        // File 0 defines `helper`; file 1 calls `helper`.
        let paths = vec![std::path::PathBuf::from("lib.rs"), std::path::PathBuf::from("main.rs")];
        let sources = vec!["pub fn helper() {}".to_string(), "fn main() { helper(); }".to_string()];
        let graph = build_reference_graph(&paths, &sources);
        // main.rs (index 1) should have an edge to lib.rs (index 0).
        assert!(graph[1].contains(&0), "main.rs should reference lib.rs: graph={graph:?}");
        // lib.rs should not reference main.rs.
        assert!(!graph[0].contains(&1), "lib.rs must not reference main.rs: graph={graph:?}");
    }

    /// build_ranked_repo_map must respect the token cap.
    #[test]
    fn ranked_repo_map_respects_token_budget() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!(
            "forgiven_pr_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        // Write 10 files each with a distinct symbol.
        for i in 0..10u8 {
            fs::write(tmp.join(format!("f{i}.rs")), format!("pub fn sym{i}() {{}}")).unwrap();
        }
        // A very small budget (16 tokens ≈ 64 chars) should truncate output.
        let map = build_ranked_repo_map(&tmp, 16);
        // Should still include the header.
        assert!(map.contains("Ranked repo map"), "header present");
        // With only 64 chars budget total, not all 10 files can appear.
        let file_count = (0..10u8).filter(|i| map.contains(&format!("f{i}.rs"))).count();
        assert!(file_count < 10, "token cap should exclude some files, got {file_count}");
        let _ = fs::remove_dir_all(&tmp);
    }
}
