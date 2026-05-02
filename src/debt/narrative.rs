//! Optional LLM narrative: send a structured metric summary to Ollama and return
//! a 2-3 sentence commentary on each debt type.
//!
//! Cached separately in `debt_narrative.md` with a 24-hour TTL.
//! Returns `None` gracefully when Ollama is unreachable or times out.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::DebtReport;

const NARRATIVE_MAX_AGE_SECS: u64 = 24 * 3600;
const NARRATIVE_TIMEOUT_SECS: u64 = 15;
const NARRATIVE_MAX_TOKENS: u32 = 250;

/// Generate (or load from cache) a qualitative narrative for the debt report.
///
/// Uses the Ollama `/v1/chat/completions` endpoint with the configured model.
/// Returns `None` if Ollama is unavailable, times out, or returns an error.
pub async fn generate_narrative(
    report: &DebtReport,
    ollama_base_url: &str,
    model: &str,
) -> Option<String> {
    // Try the cached narrative first.
    if let Some(cached) = load_narrative_cache() {
        return Some(cached);
    }

    let prompt = build_prompt(report);
    let response = call_ollama(ollama_base_url, model, &prompt).await?;
    save_narrative_cache(&response);
    Some(response)
}

// ─────────────────────────────────────────────────────────────────────────────
// Prompt construction
// ─────────────────────────────────────────────────────────────────────────────

fn build_prompt(r: &DebtReport) -> String {
    let impl_rate = (r.intent.implementation_rate() * 100.0).round() as u32;
    let active_pct = r.cognitive.active_surface_pct.round() as u32;

    format!(
        "You are a senior engineer reviewing codebase health metrics for a Rust TUI editor \
        called 'forgiven'. Provide a concise technical commentary (2 sentences max per section, \
        no bullet points, plain text) on these three debt types:\n\n\
        INTENT DEBT: {impl_rate}% of decided ADRs are implemented. \
        {pending} accepted-but-unbuilt, {stale} stale (>18 months untouched), \
        velocity {velocity} ADRs in last 6 months.\n\n\
        TECHNICAL DEBT: {high_cx} functions with high cognitive complexity \
        ({critical} critical). {todo} todo!/unimplemented! macros, \
        {unwraps} unwrap() outside tests, {dead} dead_code suppressions, \
        {long} files >500 LOC. Test coverage: {test_ratio}% of modules have tests.\n\n\
        COGNITIVE DEBT: {active_pct}% of src/ touched in last 30 days. \
        {stale_mods} stale modules. {reentry} re-entry risk functions. \
        {hotspots}.\n\n\
        For each debt type write one pointed observation naming the specific hotspot \
        and one actionable implication. Be direct, not encouraging.",
        impl_rate = impl_rate,
        pending = r.intent.accepted_pending,
        stale = r.intent.stale_count,
        velocity = r.intent.recent_velocity,
        high_cx = r.technical.high_complexity_fns,
        critical = r.technical.critical_complexity_fns,
        todo = r.technical.todo_macros,
        unwraps = r.technical.unwraps_outside_tests,
        dead = r.technical.dead_code_suppressed,
        long = r.technical.long_files,
        test_ratio = (r.technical.test_module_ratio * 100.0).round() as u32,
        active_pct = active_pct,
        stale_mods = if r.cognitive.stale_modules.is_empty() {
            "none".to_string()
        } else {
            r.cognitive.stale_modules.join(", ")
        },
        reentry = r.cognitive.reentry_risk_count,
        hotspots = if r.cognitive.error_hotspots.is_empty() {
            "no tool error hotspots".to_string()
        } else {
            format!("error hotspots: {}", r.cognitive.error_hotspots.join(", "))
        },
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Ollama call
// ─────────────────────────────────────────────────────────────────────────────

async fn call_ollama(base_url: &str, model: &str, prompt: &str) -> Option<String> {
    let url = format!("{base_url}/v1/chat/completions");

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": false,
        "max_tokens": NARRATIVE_MAX_TOKENS,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(NARRATIVE_TIMEOUT_SECS))
        .build()
        .ok()?;

    let resp = client.post(&url).json(&body).send().await.ok()?;

    if !resp.status().is_success() {
        tracing::debug!("[debt/narrative] Ollama returned {}", resp.status());
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    json.pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ─────────────────────────────────────────────────────────────────────────────
// Narrative cache (24-hour TTL, plain text file)
// ─────────────────────────────────────────────────────────────────────────────

fn narrative_cache_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".local/share")
    };
    Some(base.join("forgiven").join("debt_narrative.txt"))
}

fn load_narrative_cache() -> Option<String> {
    let path = narrative_cache_path()?;
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).unwrap_or(Duration::MAX);
    if age.as_secs() > NARRATIVE_MAX_AGE_SECS {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn save_narrative_cache(text: &str) {
    let Some(path) = narrative_cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, text);
}
