//! Debt dashboard — three lenses on codebase health, shown on the welcome screen.
//!
//! Intent debt:   ADR decisions not yet translated to code.
//! Technical debt: Static signals — cognitive complexity, todo/unwrap markers, test ratio.
//! Cognitive debt: Developer domain awareness — recently-touched surface, re-entry risk.
//!
//! Computation runs in a background tokio task (spawned from main.rs) and the result
//! is delivered via oneshot channel to the editor, following the established pattern.
//! A JSON cache at `~/.local/share/forgiven/debt_cache.json` avoids recomputation on
//! every launch; invalidated when ADR or source file mtimes change, or after 1 hour.

mod cache;
mod cognitive;
mod intent;
mod narrative;
mod technical;

pub use narrative::generate_narrative;

use serde::{Deserialize, Serialize};
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntentDebt {
    pub total_adrs: usize,
    pub implemented: usize,
    /// `Accepted` ADRs without a corresponding `Implemented` status.
    pub accepted_pending: usize,
    pub proposed: usize,
    pub superseded: usize,
    /// Active ADRs (Accepted/Proposed) not modified in the past 18 months.
    pub stale_count: usize,
    /// ADRs created or modified in the past 6 months.
    pub recent_velocity: usize,
}

impl IntentDebt {
    /// Fraction of decided ADRs (Accepted + Implemented) that are built.
    /// Returns 1.0 when there are no decided ADRs.
    pub fn implementation_rate(&self) -> f32 {
        let decided = self.implemented + self.accepted_pending;
        if decided == 0 {
            return 1.0;
        }
        self.implemented as f32 / decided as f32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TechnicalDebt {
    /// Functions with cognitive complexity score ≥ 15.
    pub high_complexity_fns: usize,
    /// Functions with cognitive complexity score ≥ 25.
    pub critical_complexity_fns: usize,
    /// Top-3 worst sites: "src/foo.rs::fn_name (score)".
    pub worst_complexity_sites: Vec<String>,
    /// `todo!()` / `unimplemented!()` macro calls.
    pub todo_macros: usize,
    /// `.unwrap()` calls outside `#[cfg(test)]` blocks.
    pub unwraps_outside_tests: usize,
    /// `FIXME` / `HACK` / `XXX` comments.
    pub fixme_comments: usize,
    /// `#[allow(dead_code)]` attribute suppressions.
    pub dead_code_suppressed: usize,
    /// `// Phase N` comments (planned-but-unfinished multi-phase work).
    pub phase_comments: usize,
    /// Source files exceeding 500 lines.
    pub long_files: usize,
    /// Fraction of `.rs` modules that contain at least one `#[cfg(test)]` block.
    pub test_module_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CognitiveDebt {
    pub total_src_files: usize,
    /// Files in `src/` touched in the last 30 days (via git log).
    pub recently_touched: usize,
    /// `recently_touched / total_src_files` as a percentage.
    pub active_surface_pct: f32,
    /// `src/` subdirectories with no recent git activity.
    pub stale_modules: Vec<String>,
    /// Functions that are both highly complex AND in recently-untouched files.
    pub reentry_risk_count: usize,
    /// Names of the top-3 re-entry risk functions.
    pub reentry_risk_sites: Vec<String>,
    /// Tool-error hotspot path prefixes from `sessions.jsonl`.
    pub error_hotspots: Vec<String>,
    pub has_git_data: bool,
    pub has_session_data: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DebtReport {
    pub intent: IntentDebt,
    pub technical: TechnicalDebt,
    pub cognitive: CognitiveDebt,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Compute (or load from cache) the three debt metrics for `project_root`.
///
/// Always returns a `DebtReport`; individual fields default to zero/empty when
/// data sources are unavailable. Never panics.
pub async fn compute(project_root: &Path) -> DebtReport {
    if let Some(cached) = cache::load_if_fresh(project_root) {
        tracing::debug!("[debt] cache hit");
        return cached;
    }

    tracing::debug!("[debt] cache miss — recomputing");

    let adr_dir = project_root.join("docs/adr");
    let src_dir = project_root.join("src");

    let intent = intent::analyse(&adr_dir);
    let technical = technical::analyse(&src_dir);
    let cognitive = cognitive::analyse(project_root).await;

    let report = DebtReport { intent, technical, cognitive };
    cache::save(project_root, &report);
    report
}
