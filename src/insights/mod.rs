//! Collaboration analytics for Forgiven.
//!
//! Phase 1 reads `forgiven.log` and produces an [`InsightSummary`] containing
//! session counts, model usage, activity heatmaps, and error tallies.
//!
//! Phase 2 adds structured JSONL instrumentation: `session_start`, per-turn
//! tool telemetry, and `tool_error` records.
//!
//! Phase 3 adds [`aggregator`] (joins sessions.jsonl with log metrics) and
//! [`panel`] (the Ratatui overlay, Mode::InsightsDashboard, SPC a I).
//!
//! Phase 4 adds an LLM-generated qualitative narrative via `:insights summarize`.

pub mod aggregator;
pub mod log_parser;
pub mod panel;

pub use aggregator::build_insights;
pub use log_parser::parse_log_file;
