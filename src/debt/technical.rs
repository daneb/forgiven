//! Static source analysis → [`TechnicalDebt`].
//!
//! Scans every `.rs` file under `src/` for explicit debt markers and computes
//! an approximate cognitive-complexity score per function.

use std::path::{Path, PathBuf};

use super::TechnicalDebt;

const LONG_FILE_LOC_THRESHOLD: usize = 500;
const HIGH_COMPLEXITY_THRESHOLD: u32 = 15;
const CRITICAL_COMPLEXITY_THRESHOLD: u32 = 25;
const WORST_SITES_SHOWN: usize = 3;

pub fn analyse(src_dir: &Path) -> TechnicalDebt {
    let files = collect_rs_files(src_dir);
    let total = files.len();

    let mut debt = TechnicalDebt::default();
    let mut modules_with_tests: usize = 0;

    // (score, label) pairs collected for sorting
    let mut complexity_sites: Vec<(u32, String)> = Vec::new();

    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };

        let rel = path.strip_prefix(src_dir).unwrap_or(path);
        let rel_str = rel.to_string_lossy();

        // ── Per-file counts ───────────────────────────────────────────────────
        let line_count = source.lines().count();
        if line_count > LONG_FILE_LOC_THRESHOLD {
            debt.long_files += 1;
        }

        let mut in_test_block = false;
        let mut unwrap_outside_test = 0usize;

        for line in source.lines() {
            let t = line.trim();

            if t.starts_with("#[cfg(test)]") {
                in_test_block = true;
                modules_with_tests += 1;
                continue;
            }
            // Test block ends at a top-level `}` (heuristic, good enough).
            if in_test_block && t == "}" && !line.starts_with(' ') {
                in_test_block = false;
            }

            // Explicit debt markers
            if t.contains("todo!()") || t.contains("todo!(") || t.contains("unimplemented!()") {
                debt.todo_macros += 1;
            }
            if !in_test_block && t.contains(".unwrap()") {
                unwrap_outside_test += t.matches(".unwrap()").count();
            }
            if t.starts_with("//")
                && (t.to_uppercase().contains("FIXME")
                    || t.to_uppercase().contains("HACK")
                    || t.to_uppercase().contains("XXX"))
            {
                debt.fixme_comments += 1;
            }
            if t.contains("#[allow(dead_code)]") {
                debt.dead_code_suppressed += 1;
            }
            if t.starts_with("//") && is_phase_comment(t) {
                debt.phase_comments += 1;
            }
        }
        debt.unwraps_outside_tests += unwrap_outside_test;

        // ── Cognitive complexity per function ─────────────────────────────────
        for (fn_name, body) in extract_function_bodies(&source) {
            let score = cognitive_score(&body);
            if score >= HIGH_COMPLEXITY_THRESHOLD {
                debt.high_complexity_fns += 1;
            }
            if score >= CRITICAL_COMPLEXITY_THRESHOLD {
                debt.critical_complexity_fns += 1;
            }
            if score >= HIGH_COMPLEXITY_THRESHOLD {
                complexity_sites.push((score, format!("src/{rel_str}::{fn_name} ({score})")));
            }
        }
    }

    // Worst-N sites sorted by score descending
    complexity_sites.sort_by_key(|b| std::cmp::Reverse(b.0));
    debt.worst_complexity_sites =
        complexity_sites.into_iter().take(WORST_SITES_SHOWN).map(|(_, label)| label).collect();

    if total > 0 {
        debt.test_module_ratio = modules_with_tests as f32 / total as f32;
    }

    debt
}

// ─────────────────────────────────────────────────────────────────────────────
// Cognitive complexity approximation
// ─────────────────────────────────────────────────────────────────────────────

/// Approximate Sonar cognitive complexity for a function body.
///
/// Each control-flow statement scores 1 + current nesting level.
/// Boolean short-circuit operators score 1 (flat).
fn cognitive_score(body: &str) -> u32 {
    let mut score: u32 = 0;
    let mut brace_depth: i32 = 0;

    for line in body.lines() {
        let trimmed = line.trim();

        // Track brace depth for nesting penalty (minimum 0).
        let opens = trimmed.chars().filter(|&c| c == '{').count() as i32;
        let closes = trimmed.chars().filter(|&c| c == '}').count() as i32;

        // Determine nesting *at the point of the control flow keyword* (before
        // the opening brace on this same line).
        let depth_here = brace_depth.max(0) as u32;

        let is_control = trimmed.starts_with("if ")
            || trimmed.starts_with("} else if ")
            || trimmed.starts_with("else if ")
            || trimmed.starts_with("} else")
            || trimmed.starts_with("else {")
            || trimmed.starts_with("while ")
            || trimmed.starts_with("for ")
            || trimmed.starts_with("loop {")
            || trimmed.starts_with("loop{")
            || trimmed.starts_with("match ");

        if is_control {
            score = score.saturating_add(1 + depth_here);
        }

        // Boolean operators — flat penalty
        score = score.saturating_add(trimmed.matches("&&").count() as u32);
        score = score.saturating_add(trimmed.matches("||").count() as u32);

        brace_depth += opens - closes;
    }

    score
}

// ─────────────────────────────────────────────────────────────────────────────
// Function body extraction
// ─────────────────────────────────────────────────────────────────────────────

/// Extract (function_name, body_text) pairs from Rust source.
///
/// Uses brace-balance counting rather than a full parse — fast and sufficient
/// for complexity scoring.
fn extract_function_bodies(source: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut results: Vec<(String, String)> = Vec::new();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Detect function definitions at top level or one impl level in.
        if let Some(name) = fn_name_from_line(trimmed) {
            let indent = leading_spaces(line);
            // Collect body by brace balance.
            let body_start = i;
            let mut depth: i32 = 0;
            let mut found_open = false;
            let mut j = i;
            while j < lines.len() {
                let bl = lines[j];
                for ch in bl.chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            found_open = true;
                        },
                        '}' => {
                            depth -= 1;
                        },
                        _ => {},
                    }
                }
                if found_open && depth <= 0 {
                    let body: Vec<&str> = lines[body_start..=j]
                        .iter()
                        .map(|l| {
                            // Strip common indent so nesting is relative.
                            if l.len() > indent {
                                &l[indent..]
                            } else {
                                l
                            }
                        })
                        .collect();
                    results.push((name, body.join("\n")));
                    i = j + 1;
                    break;
                }
                j += 1;
            }
            if j >= lines.len() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    results
}

fn fn_name_from_line(trimmed: &str) -> Option<String> {
    if !trimmed.contains(" fn ") && !trimmed.starts_with("fn ") {
        return None;
    }
    let fn_pos = trimmed.find("fn ")?;
    let after_fn = trimmed[fn_pos + 3..].trim_start();
    let end = after_fn.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(after_fn.len());
    if end == 0 {
        None
    } else {
        Some(after_fn[..end].to_string())
    }
}

fn leading_spaces(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

// ─────────────────────────────────────────────────────────────────────────────
// File collection
// ─────────────────────────────────────────────────────────────────────────────

fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rs_recursive(dir, &mut files);
    files
}

fn collect_rs_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(name, "target" | ".git" | "node_modules") {
                collect_rs_recursive(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn is_phase_comment(comment_line: &str) -> bool {
    // Matches "// Phase N" or "// Phase N:" patterns
    let after_slash = comment_line.trim_start_matches('/').trim();
    let lower = after_slash.to_lowercase();
    if !lower.starts_with("phase ") {
        return false;
    }
    let after_phase = &lower["phase ".len()..];
    after_phase.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cognitive_score_flat_if() {
        // A simple if at depth 0 scores 1
        let body = "fn foo() {\n    if x {\n        bar();\n    }\n}";
        let score = cognitive_score(body);
        assert!(score >= 1, "expected >= 1, got {score}");
    }

    #[test]
    fn cognitive_score_nested_higher() {
        let flat = "fn a() {\n    if x { }\n}";
        let nested = "fn b() {\n    if x {\n        if y {\n        }\n    }\n}";
        assert!(cognitive_score(nested) > cognitive_score(flat));
    }

    #[test]
    fn fn_name_extracted() {
        assert_eq!(
            fn_name_from_line("pub fn analyse(dir: &Path) -> Result"),
            Some("analyse".to_string())
        );
        assert_eq!(fn_name_from_line("async fn compute() -> u32"), Some("compute".to_string()));
        assert_eq!(fn_name_from_line("struct Foo"), None);
    }

    #[test]
    fn phase_comment_detection() {
        assert!(is_phase_comment("// Phase 1"));
        assert!(is_phase_comment("// Phase 3: Nexus sidecar"));
        assert!(!is_phase_comment("// This is a normal comment"));
    }
}
