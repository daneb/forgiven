//! Persistent JSON cache for debt metrics.
//!
//! Cache file: `~/.local/share/forgiven/debt_cache.json` (XDG_DATA_HOME-aware).
//! Invalidated when ADR or source file mtimes change, or when older than 1 hour.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::DebtReport;

const CACHE_VERSION: u32 = 1;
const MAX_AGE_SECS: u64 = 3600;

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    version: u32,
    computed_at: u64,
    adr_mtime_sum: u64,
    src_mtime_sum: u64,
    report: DebtReport,
}

/// Load a cached `DebtReport` if it is still fresh for `project_root`.
///
/// Returns `None` when the cache is absent, stale, version-mismatched, or corrupt.
pub fn load_if_fresh(project_root: &Path) -> Option<DebtReport> {
    let path = cache_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&data).ok()?;

    if entry.version != CACHE_VERSION {
        return None;
    }

    let now = now_secs();
    if now.saturating_sub(entry.computed_at) > MAX_AGE_SECS {
        return None;
    }

    let adr_dir = project_root.join("docs/adr");
    let src_dir = project_root.join("src");
    if mtime_sum(&adr_dir) != entry.adr_mtime_sum || mtime_sum(&src_dir) != entry.src_mtime_sum {
        return None;
    }

    Some(entry.report)
}

/// Persist a `DebtReport` to the cache file.
///
/// Silently swallows I/O errors so a permissions problem never disrupts startup.
pub fn save(project_root: &Path, report: &DebtReport) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let adr_dir = project_root.join("docs/adr");
    let src_dir = project_root.join("src");

    let entry = CacheEntry {
        version: CACHE_VERSION,
        computed_at: now_secs(),
        adr_mtime_sum: mtime_sum(&adr_dir),
        src_mtime_sum: mtime_sum(&src_dir),
        report: report.clone(),
    };

    if let Ok(json) = serde_json::to_string(&entry) {
        let _ = std::fs::write(&path, json);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".local/share")
    };
    Some(base.join("forgiven").join("debt_cache.json"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Sum of modification times (in seconds) of all files under `dir`.
///
/// Used as a lightweight fingerprint: if any file is added, removed, or
/// modified, the sum changes and the cache is invalidated.
fn mtime_sum(dir: &Path) -> u64 {
    let mut sum: u64 = 0;
    mtime_sum_recursive(dir, &mut sum);
    sum
}

fn mtime_sum_recursive(dir: &Path, acc: &mut u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            mtime_sum_recursive(&path, acc);
        } else if let Ok(meta) = std::fs::metadata(&path) {
            if let Ok(modified) = meta.modified() {
                *acc = acc.wrapping_add(
                    modified.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                );
            }
        }
    }
}
