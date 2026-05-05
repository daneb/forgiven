//! Cognitive debt — developer domain-awareness signals.
//!
//! Three signals:
//! 1. Active surface: what fraction of `src/` was touched in the last 30 days?
//! 2. Re-entry risk: functions that are highly complex AND in untouched files.
//! 3. Tool-error hotspots: path prefixes with elevated error rates in sessions.jsonl.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::CognitiveDebt;

const ACTIVE_DAYS: u64 = 30;
const REENTRY_RISK_SHOWN: usize = 3;
const STALE_MODULE_SHOWN: usize = 4;
const ERROR_HOTSPOT_SHOWN: usize = 3;
const COMPLEXITY_REENTRY_THRESHOLD: u32 = 15;

pub async fn analyse(project_root: &Path) -> CognitiveDebt {
    let src_dir = project_root.join("src");
    let total_src_files = count_rs_files(&src_dir);

    let (recently_touched_paths, has_git) = recent_git_files(project_root).await;
    let recently_touched = recently_touched_paths.len();

    let active_surface_pct = if total_src_files > 0 {
        (recently_touched as f32 / total_src_files as f32) * 100.0
    } else {
        0.0
    };

    let stale_modules = stale_module_names(&src_dir, &recently_touched_paths);
    let (reentry_risk_count, reentry_risk_sites) = reentry_risk(&src_dir, &recently_touched_paths);

    let (error_hotspots, has_session) = tool_error_hotspots();

    CognitiveDebt {
        total_src_files,
        recently_touched,
        active_surface_pct,
        stale_modules,
        reentry_risk_count,
        reentry_risk_sites,
        error_hotspots,
        has_git_data: has_git,
        has_session_data: has_session,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Git: recently-touched files
// ─────────────────────────────────────────────────────────────────────────────

async fn recent_git_files(project_root: &Path) -> (HashSet<PathBuf>, bool) {
    let since = format!("{ACTIVE_DAYS}.days.ago");
    let output = tokio::process::Command::new("git")
        .args(["log", &format!("--since={since}"), "--name-only", "--pretty=format:"])
        .current_dir(project_root)
        .output()
        .await;

    let Ok(out) = output else {
        return (HashSet::new(), false);
    };
    if !out.status.success() {
        return (HashSet::new(), false);
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let paths: HashSet<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.starts_with("src/"))
        .map(PathBuf::from)
        .collect();

    (paths, true)
}

// ─────────────────────────────────────────────────────────────────────────────
// Stale module detection
// ─────────────────────────────────────────────────────────────────────────────

fn stale_module_names(src_dir: &Path, recently_touched: &HashSet<PathBuf>) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(src_dir) else {
        return Vec::new();
    };

    let touched_dirs: HashSet<String> = recently_touched
        .iter()
        .filter_map(|p| {
            // "src/foo/bar.rs" -> "foo"
            p.components().nth(1).and_then(|c| c.as_os_str().to_str()).map(String::from)
        })
        .collect();

    let mut stale: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !touched_dirs.contains(name))
        .collect();

    stale.sort();
    stale.truncate(STALE_MODULE_SHOWN);
    stale
}

// ─────────────────────────────────────────────────────────────────────────────
// Re-entry risk: complex functions in untouched files
// ─────────────────────────────────────────────────────────────────────────────

fn reentry_risk(src_dir: &Path, recently_touched: &HashSet<PathBuf>) -> (usize, Vec<String>) {
    let mut sites: Vec<(u32, String)> = Vec::new();

    let src_parent = src_dir.parent().unwrap_or(src_dir);

    for path in collect_rs_files(src_dir) {
        let rel = path.strip_prefix(src_parent).unwrap_or(&path);
        // Only examine files NOT recently touched.
        if recently_touched.contains(rel) {
            continue;
        }

        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };

        let rel_display = rel.to_string_lossy();
        for (fn_name, body) in extract_fn_bodies(&source) {
            let score = approx_cognitive_score(&body);
            if score >= COMPLEXITY_REENTRY_THRESHOLD {
                sites.push((score, format!("{rel_display}::{fn_name} ({score})")));
            }
        }
    }

    sites.sort_by_key(|b| std::cmp::Reverse(b.0));
    let count = sites.len();
    let top: Vec<String> = sites.into_iter().take(REENTRY_RISK_SHOWN).map(|(_, l)| l).collect();
    (count, top)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tool-error hotspots from sessions.jsonl
// ─────────────────────────────────────────────────────────────────────────────

fn tool_error_hotspots() -> (Vec<String>, bool) {
    let Some(path) = sessions_jsonl_path() else {
        return (Vec::new(), false);
    };

    let Ok(content) = std::fs::read_to_string(&path) else {
        return (Vec::new(), false);
    };

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut found_any = false;

    for line in content.lines() {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if val.get("type").and_then(|t| t.as_str()) != Some("tool_error") {
            continue;
        }
        found_any = true;
        let tool = val.get("tool").and_then(|t| t.as_str()).unwrap_or("unknown").to_string();
        *counts.entry(tool).or_default() += 1;
    }

    if !found_any {
        return (Vec::new(), false);
    }

    let mut ranked: Vec<(usize, String)> = counts.into_iter().map(|(k, v)| (v, k)).collect();
    ranked.sort_by_key(|b| std::cmp::Reverse(b.0));
    let hotspots: Vec<String> = ranked
        .into_iter()
        .take(ERROR_HOTSPOT_SHOWN)
        .map(|(count, tool)| format!("{tool} \u{d7}{count}"))
        .collect();

    (hotspots, true)
}

fn sessions_jsonl_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".local/share")
    };
    Some(base.join("forgiven").join("sessions.jsonl"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Cognitive complexity (local copy — avoids cross-sibling coupling)
// ─────────────────────────────────────────────────────────────────────────────

fn approx_cognitive_score(body: &str) -> u32 {
    let mut score: u32 = 0;
    let mut brace_depth: i32 = 0;

    for line in body.lines() {
        let trimmed = line.trim();
        let opens = trimmed.chars().filter(|&c| c == '{').count() as i32;
        let closes = trimmed.chars().filter(|&c| c == '}').count() as i32;
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
        score = score.saturating_add(trimmed.matches("&&").count() as u32);
        score = score.saturating_add(trimmed.matches("||").count() as u32);

        brace_depth += opens - closes;
    }

    score
}

// ─────────────────────────────────────────────────────────────────────────────
// File helpers
// ─────────────────────────────────────────────────────────────────────────────

fn count_rs_files(dir: &Path) -> usize {
    let mut files = Vec::new();
    collect_rs_into(dir, &mut files);
    files.len()
}

fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rs_into(dir, &mut files);
    files
}

fn collect_rs_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(name, "target" | ".git" | "node_modules") {
                collect_rs_into(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn extract_fn_bodies(source: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut results = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if let Some(name) = fn_name(trimmed) {
            let mut depth: i32 = 0;
            let mut found_open = false;
            let body_start = i;
            let mut j = i;
            while j < lines.len() {
                for ch in lines[j].chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            found_open = true;
                        },
                        '}' => depth -= 1,
                        _ => {},
                    }
                }
                if found_open && depth <= 0 {
                    let body = lines[body_start..=j].join("\n");
                    results.push((name, body));
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

fn fn_name(trimmed: &str) -> Option<String> {
    if !trimmed.contains(" fn ") && !trimmed.starts_with("fn ") {
        return None;
    }
    let fn_pos = trimmed.find("fn ")?;
    let after = trimmed[fn_pos + 3..].trim_start();
    let end = after.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(after.len());
    if end == 0 {
        None
    } else {
        Some(after[..end].to_string())
    }
}
