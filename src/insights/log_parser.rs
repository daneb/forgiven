//! Phase 1 log parser: mine `forgiven.log` for collaboration metrics.
//!
//! The log is written with ANSI colour codes by the tracing subscriber. Each
//! line, after stripping escape sequences, has the form:
//!
//! ```text
//! 2026-04-02T22:25:29.957142Z  INFO forgiven::agent::agentic_loop: Sending completion request model="qwen2.5-coder:7b" provider=Ollama
//! ```
//!
//! Key log signals and what they map to:
//!
//! | Signal | Metric |
//! |--------|--------|
//! | `"Starting forgiven"` | session start |
//! | `"Sending completion request"` | agentic LLM round |
//! | `"tool calling disabled"` | chat-only round |
//! | `"one_shot request sending"` | inline one-shot request |
//! | `"Saved buffer"` | buffer save (proxy for files touched) |
//! | log level `WARN` / `ERROR` | warnings / errors |

use std::collections::BTreeMap;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// InsightSummary
// ─────────────────────────────────────────────────────────────────────────────

/// Aggregated metrics derived from `forgiven.log`.
#[derive(Debug, Default)]
pub struct InsightSummary {
    /// Earliest date seen in the log (`YYYY-MM-DD`).
    pub first_date: Option<String>,
    /// Latest date seen in the log (`YYYY-MM-DD`).
    pub last_date: Option<String>,
    /// Number of distinct calendar days with any log activity.
    pub active_days: usize,
    /// Number of times the editor was started (`"Starting forgiven"`).
    pub session_count: usize,
    /// Total agentic LLM rounds (`"Sending completion request"`).
    pub llm_request_count: usize,
    /// Rounds where tool-calling was disabled (`"tool calling disabled"`).
    pub chat_only_count: usize,
    /// Inline one-shot AI requests (`"one_shot request sending"`).
    pub one_shot_count: usize,
    /// Buffer saves (`"Saved buffer"`).
    pub buffer_save_count: usize,
    /// Request count per model name, sorted alphabetically.
    pub models: BTreeMap<String, usize>,
    /// Request count per provider name, sorted alphabetically.
    pub providers: BTreeMap<String, usize>,
    /// LLM requests (agentic + one-shot) bucketed by UTC hour `[0..24]`.
    pub requests_by_hour: [usize; 24],
    /// Session starts per calendar day (`YYYY-MM-DD` → count).
    pub sessions_by_date: BTreeMap<String, usize>,
    /// Number of `WARN`-level log lines.
    pub warn_count: usize,
    /// Number of `ERROR`-level log lines.
    pub error_count: usize,
}

impl InsightSummary {
    /// Render a markdown report suitable for display in the agent panel.
    pub fn format_report(&self) -> String {
        let mut out = String::with_capacity(1024);

        // ── Header ────────────────────────────────────────────────────────
        let date_range = match (&self.first_date, &self.last_date) {
            (Some(f), Some(l)) if f == l => f.clone(),
            (Some(f), Some(l)) => format!("{f} → {l}"),
            _ => "no data".to_string(),
        };
        let total_requests = self.llm_request_count + self.one_shot_count;
        let msgs_per_day = if self.active_days > 0 {
            total_requests as f64 / self.active_days as f64
        } else {
            0.0
        };

        out.push_str("## Forgiven Insights\n\n");
        out.push_str(&format!(
            "**{date_range} · {} sessions · {} active days**\n\n",
            self.session_count, self.active_days
        ));

        // ── Activity ──────────────────────────────────────────────────────
        out.push_str("### Activity\n");
        out.push_str(&format!(
            "- LLM requests: **{total_requests}** ({:.1} / day)\n",
            msgs_per_day
        ));
        if self.llm_request_count > 0 {
            out.push_str(&format!(
                "  - Agentic rounds: {}\n",
                self.llm_request_count - self.chat_only_count.min(self.llm_request_count)
            ));
            out.push_str(&format!("  - Chat-only rounds: {}\n", self.chat_only_count));
        }
        if self.one_shot_count > 0 {
            out.push_str(&format!("  - One-shot (inline): {}\n", self.one_shot_count));
        }
        out.push_str(&format!("- Buffer saves: {}\n\n", self.buffer_save_count));

        // ── Models ────────────────────────────────────────────────────────
        if !self.models.is_empty() {
            out.push_str("### Models\n");
            // Sort by count descending for display.
            let mut by_count: Vec<_> = self.models.iter().collect();
            by_count.sort_by(|a, b| b.1.cmp(a.1));
            for (model, count) in &by_count {
                out.push_str(&format!("- `{model}`: {count} requests\n"));
            }
            out.push('\n');
        }

        // ── Providers ─────────────────────────────────────────────────────
        if !self.providers.is_empty() {
            out.push_str("### Providers\n");
            let mut by_count: Vec<_> = self.providers.iter().collect();
            by_count.sort_by(|a, b| b.1.cmp(a.1));
            for (provider, count) in &by_count {
                out.push_str(&format!("- {provider}: {count} requests\n"));
            }
            out.push('\n');
        }

        // ── Time of day ───────────────────────────────────────────────────
        let time_bands = [
            ("Night   (00–06)", 0usize..6),
            ("Morning (06–12)", 6..12),
            ("Afternoon(12–18)", 12..18),
            ("Evening (18–24)", 18..24),
        ];
        let band_totals: Vec<usize> = time_bands
            .iter()
            .map(|(_, r)| r.clone().map(|h| self.requests_by_hour[h]).sum())
            .collect();
        let max_band = *band_totals.iter().max().unwrap_or(&1).max(&1);

        out.push_str("### By time of day (UTC)\n");
        for ((label, _), total) in time_bands.iter().zip(&band_totals) {
            let filled = (total * 12 / max_band).min(12);
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(12 - filled));
            out.push_str(&format!("- {label}: {bar}  {total}\n"));
        }
        out.push('\n');

        // ── Warnings & errors ─────────────────────────────────────────────
        if self.warn_count > 0 || self.error_count > 0 {
            out.push_str("### Warnings & errors\n");
            if self.warn_count > 0 {
                out.push_str(&format!("- Warnings: {}\n", self.warn_count));
            }
            if self.error_count > 0 {
                out.push_str(&format!("- Errors: {}\n", self.error_count));
            }
            out.push('\n');
        }

        // ── Recent sessions ───────────────────────────────────────────────
        if !self.sessions_by_date.is_empty() {
            out.push_str("### Sessions per day (recent)\n");
            let recent: Vec<_> = self.sessions_by_date.iter().rev().take(7).collect();
            for (date, count) in recent.iter().rev() {
                out.push_str(&format!("- {date}: {count}\n"));
            }
            out.push('\n');
        }

        out.push_str("_Phase 1 data source: `forgiven.log`. Run `:insights` again after more sessions to see trends._\n");
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ANSI stripping
// ─────────────────────────────────────────────────────────────────────────────

/// Remove ANSI/VT100 escape sequences (`ESC[...m`) from a log line.
///
/// The tracing subscriber writes colour codes unconditionally to the log file.
/// This strips them so text matching works on the bare content.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // ESC (0x1b) followed by '[' starts a CSI sequence; skip until 'm'.
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'm' {
                i += 1;
            }
            i += 1; // consume the 'm'
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Key-value extraction helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the value from `key="value"` in `s`. Returns `None` if not found.
fn extract_quoted<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=\"");
    let start = s.find(needle.as_str())? + needle.len();
    let end = start + s[start..].find('"')?;
    Some(&s[start..end])
}

/// Extract the value from `key=value` (unquoted, terminated by space or end)
/// in `s`. Returns `None` if not found.
fn extract_unquoted<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let start = s.find(needle.as_str())? + needle.len();
    // Value ends at the next space, or at the end of the string.
    let len = s[start..].find(' ').unwrap_or(s.len() - start);
    Some(&s[start..start + len])
}

// ─────────────────────────────────────────────────────────────────────────────
// Timestamp helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract `YYYY-MM-DD` from an RFC3339 timestamp (first 10 chars).
fn ts_date(ts: &str) -> Option<&str> {
    if ts.len() >= 10 {
        Some(&ts[..10])
    } else {
        None
    }
}

/// Extract the UTC hour (0–23) from an RFC3339 timestamp (`T` at index 10,
/// hours at 11–12).
fn ts_hour(ts: &str) -> Option<u8> {
    if ts.len() < 13 {
        return None;
    }
    ts[11..13].parse::<u8>().ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Line parser
// ─────────────────────────────────────────────────────────────────────────────

/// Fields extracted from a single stripped log line.
struct ParsedLine<'a> {
    ts: &'a str,
    level: &'a str,
    /// Module path (e.g. `forgiven::agent::agentic_loop`).
    _module: &'a str,
    /// Everything after `module: `.
    message: &'a str,
}

/// Parse a stripped log line into its structural components.
///
/// Expected format (after ANSI stripping):
/// ```text
/// 2026-04-02T22:25:29.957142Z  INFO forgiven::agent::agentic_loop: message text
/// ```
///
/// There are two spaces between the timestamp and the level because the
/// tracing formatter wraps the timestamp in dim-mode ANSI codes (`ESC[2m…ESC[0m`)
/// followed by a literal space, then another space before the level.  After
/// stripping we get `"<ts>  <level> <module>: <message>"`.
fn parse_line(stripped: &str) -> Option<ParsedLine<'_>> {
    // Timestamp: first whitespace-delimited token.
    let ts_end = stripped.find(' ')?;
    let ts = &stripped[..ts_end];
    // Sanity-check: timestamp must start with a digit (year).
    if !ts.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    // Level: next non-empty token after trimming the leading whitespace.
    let after_ts = stripped[ts_end..].trim_start();
    let level_end = after_ts.find(|c: char| c.is_whitespace()).unwrap_or(after_ts.len());
    let level = &after_ts[..level_end];

    // Everything after the level — `"module: message text"`.
    let after_level = after_ts[level_end..].trim_start();

    // Split on the first `": "` (colon-space) to separate module path from
    // message.  Module paths use `"::"` (double colon) which does not match.
    let sep = after_level.find(": ")?;
    let module = &after_level[..sep];
    let message = &after_level[sep + 2..];

    Some(ParsedLine { ts, level, _module: module, message })
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Parse `forgiven.log` at `path` and return an [`InsightSummary`].
///
/// Returns `None` when the file cannot be read (missing, permission error).
/// An empty-but-valid summary is returned when the file exists but has no
/// recognised events.
pub fn parse_log_file(path: &Path) -> Option<InsightSummary> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(parse_log_content(&content))
}

/// Parse log content from a string slice (used directly in tests).
pub fn parse_log_content(content: &str) -> InsightSummary {
    let mut summary = InsightSummary::default();
    let mut active_day_set: std::collections::HashSet<String> = Default::default();

    for raw_line in content.lines() {
        let stripped = strip_ansi(raw_line);
        let Some(line) = parse_line(&stripped) else { continue };

        // Track active days and date range from every line's timestamp.
        if let Some(date) = ts_date(line.ts) {
            active_day_set.insert(date.to_owned());
            match &summary.first_date {
                None => summary.first_date = Some(date.to_owned()),
                Some(f) if date < f.as_str() => summary.first_date = Some(date.to_owned()),
                _ => {},
            }
            match &summary.last_date {
                None => summary.last_date = Some(date.to_owned()),
                Some(l) if date > l.as_str() => summary.last_date = Some(date.to_owned()),
                _ => {},
            }
        }

        // Dispatch on message content.
        let msg = line.message;

        if msg.contains("Starting forgiven") {
            summary.session_count += 1;
            if let Some(date) = ts_date(line.ts) {
                *summary.sessions_by_date.entry(date.to_owned()).or_insert(0) += 1;
            }
            continue;
        }

        if msg.contains("Sending completion request") {
            summary.llm_request_count += 1;
            // Record model and provider.
            if let Some(model) = extract_quoted(msg, "model") {
                *summary.models.entry(model.to_owned()).or_insert(0) += 1;
            }
            if let Some(provider) = extract_unquoted(msg, "provider") {
                *summary.providers.entry(provider.to_owned()).or_insert(0) += 1;
            }
            if let Some(h) = ts_hour(line.ts) {
                summary.requests_by_hour[h as usize] += 1;
            }
            continue;
        }

        if msg.contains("tool calling disabled") {
            summary.chat_only_count += 1;
            continue;
        }

        if msg.contains("one_shot request sending") {
            summary.one_shot_count += 1;
            if let Some(model) = extract_quoted(msg, "model") {
                *summary.models.entry(model.to_owned()).or_insert(0) += 1;
            }
            if let Some(provider) = extract_unquoted(msg, "provider") {
                *summary.providers.entry(provider.to_owned()).or_insert(0) += 1;
            }
            if let Some(h) = ts_hour(line.ts) {
                summary.requests_by_hour[h as usize] += 1;
            }
            continue;
        }

        if msg.contains("Saved buffer") {
            summary.buffer_save_count += 1;
            continue;
        }

        // Warning / error tallies.
        match line.level {
            "WARN" => summary.warn_count += 1,
            "ERROR" => summary.error_count += 1,
            _ => {},
        }
    }

    summary.active_days = active_day_set.len();
    summary
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Sample log lines representative of what forgiven.log contains.
    // The ANSI codes are embedded as raw escape sequences.
    const SAMPLE_LOG: &str = "\
\x1b[2m2026-04-02T22:25:29.957270Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mforgiven\x1b[0m\x1b[2m:\x1b[0m Starting forgiven
\x1b[2m2026-04-03T09:09:58.963336Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mforgiven::agent::agentic_loop\x1b[0m\x1b[2m:\x1b[0m Sending completion request model=\"qwen2.5-coder:7b\" provider=Ollama
\x1b[2m2026-04-03T09:18:17.669910Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mforgiven::agent::agentic_loop\x1b[0m\x1b[2m:\x1b[0m Agentic loop: tool calling disabled for Ollama — running in chat-only mode
\x1b[2m2026-04-03T09:18:33.890273Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mforgiven::agent::agentic_loop\x1b[0m\x1b[2m:\x1b[0m Sending completion request model=\"qwen2.5-coder:7b\" provider=Ollama
\x1b[2m2026-04-04T14:26:37.710554Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mforgiven::editor::ai\x1b[0m\x1b[2m:\x1b[0m one_shot request sending provider=Ollama model=\"gemma4:e4b\"
\x1b[2m2026-04-04T14:49:44.320418Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mforgiven::buffer::buffer\x1b[0m\x1b[2m:\x1b[0m Saved buffer 'main.rs' to \"/path/main.rs\"
\x1b[2m2026-04-04T14:49:30.449592Z\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2mforgiven::lsp\x1b[0m\x1b[2m:\x1b[0m [lsp stderr] rust-analyzer: file not found
\x1b[2m2026-04-05T08:00:00.000000Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mforgiven\x1b[0m\x1b[2m:\x1b[0m Starting forgiven
";

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[2m2026-04-02T22:25:29Z\x1b[0m \x1b[32m INFO\x1b[0m msg";
        let stripped = strip_ansi(input);
        assert_eq!(stripped, "2026-04-02T22:25:29Z  INFO msg");
    }

    #[test]
    fn test_extract_quoted() {
        let line = "Sending completion request model=\"qwen2.5-coder:7b\" provider=Ollama";
        assert_eq!(extract_quoted(line, "model"), Some("qwen2.5-coder:7b"));
    }

    #[test]
    fn test_extract_unquoted() {
        let line = "Sending completion request model=\"qwen2.5-coder:7b\" provider=Ollama";
        assert_eq!(extract_unquoted(line, "provider"), Some("Ollama"));
    }

    #[test]
    fn test_ts_date() {
        assert_eq!(ts_date("2026-04-02T22:25:29.957142Z"), Some("2026-04-02"));
        assert_eq!(ts_date("short"), None);
    }

    #[test]
    fn test_ts_hour() {
        assert_eq!(ts_hour("2026-04-02T22:25:29.957142Z"), Some(22));
        assert_eq!(ts_hour("2026-04-02T09:00:00Z"), Some(9));
    }

    #[test]
    fn test_session_count() {
        let s = parse_log_content(SAMPLE_LOG);
        assert_eq!(s.session_count, 2);
    }

    #[test]
    fn test_llm_request_count() {
        let s = parse_log_content(SAMPLE_LOG);
        // Two "Sending completion request" lines.
        assert_eq!(s.llm_request_count, 2);
    }

    #[test]
    fn test_chat_only_count() {
        let s = parse_log_content(SAMPLE_LOG);
        assert_eq!(s.chat_only_count, 1);
    }

    #[test]
    fn test_one_shot_count() {
        let s = parse_log_content(SAMPLE_LOG);
        assert_eq!(s.one_shot_count, 1);
    }

    #[test]
    fn test_buffer_save_count() {
        let s = parse_log_content(SAMPLE_LOG);
        assert_eq!(s.buffer_save_count, 1);
    }

    #[test]
    fn test_warn_count() {
        let s = parse_log_content(SAMPLE_LOG);
        assert_eq!(s.warn_count, 1);
    }

    #[test]
    fn test_models() {
        let s = parse_log_content(SAMPLE_LOG);
        // qwen2.5-coder:7b appears in two "Sending completion request" lines,
        // gemma4:e4b appears in the one-shot line.
        assert_eq!(s.models.get("qwen2.5-coder:7b"), Some(&2));
        assert_eq!(s.models.get("gemma4:e4b"), Some(&1));
    }

    #[test]
    fn test_active_days() {
        let s = parse_log_content(SAMPLE_LOG);
        // Lines span 2026-04-02, 04-03, 04-04, 04-05 → 4 unique days.
        assert_eq!(s.active_days, 4);
    }

    #[test]
    fn test_date_range() {
        let s = parse_log_content(SAMPLE_LOG);
        assert_eq!(s.first_date.as_deref(), Some("2026-04-02"));
        assert_eq!(s.last_date.as_deref(), Some("2026-04-05"));
    }

    #[test]
    fn test_requests_by_hour() {
        let s = parse_log_content(SAMPLE_LOG);
        // Hour 9: one "Sending completion request" (09:09) + one chat-only (09:18) + one
        // "Sending completion request" (09:18) → 2 requests_by_hour entries at hour 9.
        assert_eq!(s.requests_by_hour[9], 2);
        // Hour 14: one one-shot request.
        assert_eq!(s.requests_by_hour[14], 1);
    }

    #[test]
    fn test_format_report_non_empty() {
        let s = parse_log_content(SAMPLE_LOG);
        let report = s.format_report();
        assert!(report.contains("## Forgiven Insights"));
        assert!(report.contains("sessions"));
        assert!(report.contains("qwen2.5-coder:7b"));
    }
}
