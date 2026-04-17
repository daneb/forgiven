//! Phase 3 aggregator: parse `sessions.jsonl` and join with log metrics.
//!
//! `sessions.jsonl` contains one JSON object per line. Two record types are
//! consumed here:
//!
//! ```text
//! // session_end (written since Phase 1 / ADR 0092)
//! {"type":"session_end","model":"gemma4:e4b","session_rounds":5,
//!  "session_prompt_total":12000,"session_completion_total":3200,
//!  "files_changed":2,"ended_by":"new_conversation","ts":1776341015}
//!
//! // tool_error (written from Phase 2 onwards, ADR 0129)
//! {"type":"tool_error","tool":"read_file","error_type":"NotFound","ts":...}
//! ```
//!
//! Phase 2 also adds `session_start` records — these are parsed but currently
//! used only for future session-duration calculations.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::insights::log_parser::InsightSummary;

// ─────────────────────────────────────────────────────────────────────────────
// Raw record types (deserialized from sessions.jsonl lines)
// ─────────────────────────────────────────────────────────────────────────────

/// A `session_end` line from `sessions.jsonl`.
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields reserved for Phase 4 narrative + future analysis
pub struct SessionEndRecord {
    /// Model name used (e.g. `"gemma4:e4b"`).
    pub model: Option<String>,
    /// Why the session ended: `"new_conversation"`, `"quit"`, etc.
    pub ended_by: Option<String>,
    /// Number of files the agent modified.
    pub files_changed: Option<u32>,
    /// Cumulative prompt tokens sent this session (includes re-sends).
    pub session_prompt_total: Option<u32>,
    /// Cumulative completion tokens received this session.
    pub session_completion_total: Option<u32>,
    /// Number of agentic rounds (LLM invocations) this session.
    pub session_rounds: Option<u32>,
    /// Unix timestamp of the `session_end` event.
    pub ts: Option<i64>,
}

/// A `tool_error` line from `sessions.jsonl` (written from Phase 2 onwards).
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // fields reserved for Phase 4 narrative + future drill-down
pub struct ToolErrorRecord {
    /// The tool that failed (e.g. `"read_file"`).
    pub tool: Option<String>,
    /// Categorised error type (e.g. `"NotFound"`, `"PermissionDenied"`).
    pub error_type: Option<String>,
    /// Unix timestamp.
    pub ts: Option<i64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Aggregated session metrics
// ─────────────────────────────────────────────────────────────────────────────

/// Processed metrics derived from `sessions.jsonl`.
#[derive(Debug, Default)]
pub struct SessionMetrics {
    /// All `session_end` records.
    pub sessions: Vec<SessionEndRecord>,
    /// All `tool_error` records (Phase 2+).
    pub tool_errors: Vec<ToolErrorRecord>,

    /// How many session records carry token data (prompt > 0 || completion > 0).
    pub sessions_with_tokens: usize,
    /// Sum of prompt tokens across all sessions.
    pub total_prompt_tokens: u64,
    /// Sum of completion tokens across all sessions.
    pub total_completion_tokens: u64,

    /// Maximum rounds seen in any single session.
    pub max_rounds: u32,
    /// Distribution of session lengths (rounds). Buckets 1-19 are exact;
    /// bucket 20 catches sessions with ≥ 20 rounds.
    pub rounds_histogram: BTreeMap<u32, usize>,

    /// Total files changed across all sessions.
    pub total_files_changed: u32,

    /// Tool errors grouped by `error_type`.
    pub errors_by_type: BTreeMap<String, usize>,
}

impl SessionMetrics {
    /// Average rounds per session; 0.0 when no sessions have been recorded.
    pub fn avg_rounds(&self) -> f64 {
        if self.sessions.is_empty() {
            return 0.0;
        }
        let total: u64 = self.sessions.iter().filter_map(|s| s.session_rounds).map(u64::from).sum();
        total as f64 / self.sessions.len() as f64
    }

    /// Average files changed per session.
    pub fn avg_files(&self) -> f64 {
        if self.sessions.is_empty() {
            return 0.0;
        }
        self.total_files_changed as f64 / self.sessions.len() as f64
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Combined insights struct
// ─────────────────────────────────────────────────────────────────────────────

/// All available collaboration analytics, combining log-derived and JSONL data.
pub struct AggregatedInsights {
    /// Phase 1 log-derived metrics (always present).
    pub log: InsightSummary,
    /// Phase 2+ sessions.jsonl metrics (empty when no file exists).
    pub sessions: SessionMetrics,
    /// Phase 4 LLM-generated narrative (`insights_narrative.md`), if present.
    pub narrative: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Parser
// ─────────────────────────────────────────────────────────────────────────────

/// Parse `sessions.jsonl` at `path` and return aggregated `SessionMetrics`.
///
/// Missing or unreadable files return an empty `SessionMetrics` rather than
/// an error, so Phase 3 degrades gracefully when Phase 2 data doesn't exist yet.
pub fn parse_sessions_jsonl(path: &Path) -> SessionMetrics {
    let mut m = SessionMetrics::default();

    let Ok(content) = std::fs::read_to_string(path) else { return m };

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // Route by the `"type"` field without fully deserialising — a fast
        // substring check before the heavier serde call.
        if line.contains("\"session_end\"") {
            if let Ok(rec) = serde_json::from_str::<SessionEndRecord>(line) {
                let pt = rec.session_prompt_total.unwrap_or(0);
                let ct = rec.session_completion_total.unwrap_or(0);
                if pt > 0 || ct > 0 {
                    m.sessions_with_tokens += 1;
                    m.total_prompt_tokens += u64::from(pt);
                    m.total_completion_tokens += u64::from(ct);
                }

                if let Some(rounds) = rec.session_rounds {
                    let bucket = rounds.min(20);
                    *m.rounds_histogram.entry(bucket).or_insert(0) += 1;
                    if rounds > m.max_rounds {
                        m.max_rounds = rounds;
                    }
                }

                m.total_files_changed += rec.files_changed.unwrap_or(0);
                m.sessions.push(rec);
            }
        } else if line.contains("\"tool_error\"") {
            if let Ok(rec) = serde_json::from_str::<ToolErrorRecord>(line) {
                let key = rec.error_type.clone().unwrap_or_else(|| "unknown".to_owned());
                *m.errors_by_type.entry(key).or_insert(0) += 1;
                m.tool_errors.push(rec);
            }
        }
        // session_start / other future record types are silently skipped.
    }

    m
}

/// Build an [`AggregatedInsights`] by reading all available data sources under
/// `data_dir` (typically `~/.local/share/forgiven/`).
pub fn build_insights(data_dir: &Path) -> AggregatedInsights {
    let log = crate::insights::log_parser::parse_log_file(&data_dir.join("forgiven.log"))
        .unwrap_or_default();
    let sessions = parse_sessions_jsonl(&data_dir.join("sessions.jsonl"));
    let narrative = std::fs::read_to_string(data_dir.join("insights_narrative.md")).ok();
    AggregatedInsights { log, sessions, narrative }
}
