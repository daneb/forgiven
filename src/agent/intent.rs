//! Intent Translator — Option D of the AI-IDE architecture exploration.
//!
//! Preprocesses user messages into structured task specs before the main agent
//! loop begins. A small, fast LLM call rewrites ambiguous prompts into crisp,
//! scoped instructions, reducing exploratory tool-calling rounds.
//!
//! See `docs/intent-translator.md` for the full specification.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::provider::ProviderKind;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TranslationContext<'a> {
    /// Path or display name of the currently open file (not content).
    pub open_file: Option<&'a str>,
    /// Recently opened file paths (last N).
    pub recent_files: &'a [String],
    /// Reserved for future scope-resolution use; currently unused in the prompt.
    #[allow(dead_code)]
    pub project_root: &'a std::path::Path,
    /// Primary language of the project (e.g. `"Rust"`, `"TypeScript"`).
    pub language_hint: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct Intent {
    /// One-sentence imperative outcome, e.g. "Add error handling to X::foo".
    pub goal: String,
    pub scope: IntentScope,
    pub expected_output: OutputType,
    /// Clarifying questions the IDE should present before dispatching.
    /// Empty when the intent is unambiguous.
    pub ambiguities: Vec<String>,
    /// Rewritten prompt for the main agent. Empty when `ambiguities` is non-empty.
    pub structured_prompt: String,
}

#[derive(Debug, Clone)]
pub enum IntentScope {
    SingleFile(PathBuf),
    MultiFile(Vec<PathBuf>),
    Symbol { file: PathBuf, symbol: String },
    ProjectWide,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OutputType {
    Code,
    Diff,
    Explanation,
    Question,
    Mixed,
}

impl std::fmt::Display for OutputType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputType::Code => write!(f, "Code"),
            OutputType::Diff => write!(f, "Diff"),
            OutputType::Explanation => write!(f, "Explanation"),
            OutputType::Question => write!(f, "Question"),
            OutputType::Mixed => write!(f, "Mixed"),
        }
    }
}

impl IntentScope {
    fn label(&self) -> String {
        match self {
            IntentScope::SingleFile(p) => p.display().to_string(),
            IntentScope::MultiFile(files) => {
                if files.is_empty() {
                    "multiple files".to_string()
                } else {
                    format!("{} files", files.len())
                }
            },
            IntentScope::Symbol { file, symbol } => {
                format!("{} ({})", file.display(), symbol)
            },
            IntentScope::ProjectWide => "project-wide".to_string(),
            IntentScope::Unknown => "unknown scope".to_string(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Call settings resolved at submit time
// ─────────────────────────────────────────────────────────────────────────────

/// Resolved settings passed to [`translate_intent`] at submit time.
/// Built from config + panel state so the function is pure.
pub struct IntentCallSettings<'a> {
    pub endpoint: &'a str,
    pub api_token: &'a str,
    /// Model ID for the translator call (e.g. `"claude-haiku-4-5-20251001"`).
    pub model: &'a str,
    pub provider_kind: &'a ProviderKind,
    pub timeout_ms: u64,
    pub min_chars_to_translate: usize,
    /// Literal string prefixes — messages starting with any of these are skipped.
    pub skip_patterns: &'a [String],
    pub openrouter_site_url: &'a str,
    pub openrouter_app_name: &'a str,
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal JSON shape returned by the model
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct IntentJson {
    goal: Option<String>,
    scope: Option<String>,
    scope_file: Option<String>,
    scope_files: Option<Vec<String>>,
    scope_symbol: Option<String>,
    expected_output: Option<OutputType>,
    ambiguities: Option<Vec<String>>,
    structured_prompt: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

fn build_translator_prompt(message: &str, ctx: &TranslationContext<'_>) -> String {
    let open_file = ctx.open_file.unwrap_or("none");
    let recent =
        if ctx.recent_files.is_empty() { "none".to_string() } else { ctx.recent_files.join(", ") };
    let lang = ctx.language_hint.unwrap_or("Rust");

    format!(
        "You are an intent translator for a {lang} IDE. You do NOT answer the user's question.\n\
         You REWRITE it into a structured spec the main agent will execute.\n\n\
         Given:\n\
         - User message: {message}\n\
         - Open file: {open_file}\n\
         - Recent files: {recent}\n\
         - Project language: {lang}\n\n\
         Produce JSON with these exact fields:\n\
           goal: one-sentence outcome (imperative, e.g. \"Add error handling to X::foo\")\n\
           scope: \"SingleFile\" | \"MultiFile\" | \"Symbol\" | \"ProjectWide\" | \"Unknown\"\n\
           scope_file: file path string if scope is SingleFile or Symbol (omit otherwise)\n\
           scope_files: array of path strings if scope is MultiFile (omit otherwise)\n\
           scope_symbol: symbol name string if scope is Symbol (omit otherwise)\n\
           expected_output: \"Code\" | \"Diff\" | \"Explanation\" | \"Question\" | \"Mixed\"\n\
           ambiguities: array of clarifying questions (empty array if intent is clear)\n\
           structured_prompt: rewritten prompt in 1-3 sentences for the main agent\n\n\
         Rules:\n\
         - If the message has 2+ genuine ambiguities, populate ambiguities and set structured_prompt to \"\".\n\
         - If the intent is already crisp, set structured_prompt identical to the input message.\n\
         - Output ONLY the JSON object, no markdown fences, no preamble."
    )
}

fn parse_intent_json(raw: &str) -> Option<Intent> {
    let cleaned = raw.trim();
    // Strip markdown code fences if the model adds them despite instructions.
    let cleaned = if let Some(body) = cleaned.strip_prefix("```") {
        let inner = body.strip_prefix("json\n").or_else(|| body.strip_prefix('\n')).unwrap_or(body);
        inner.trim_end_matches("```").trim()
    } else {
        cleaned
    };

    let j: IntentJson = serde_json::from_str(cleaned).ok()?;

    let scope = match j.scope.as_deref() {
        Some("SingleFile") => {
            IntentScope::SingleFile(j.scope_file.map(PathBuf::from).unwrap_or_default())
        },
        Some("MultiFile") => IntentScope::MultiFile(
            j.scope_files.unwrap_or_default().into_iter().map(PathBuf::from).collect(),
        ),
        Some("Symbol") => IntentScope::Symbol {
            file: j.scope_file.map(PathBuf::from).unwrap_or_default(),
            symbol: j.scope_symbol.unwrap_or_default(),
        },
        Some("ProjectWide") => IntentScope::ProjectWide,
        _ => IntentScope::Unknown,
    };

    Some(Intent {
        goal: j.goal.unwrap_or_default(),
        scope,
        expected_output: j.expected_output.unwrap_or(OutputType::Mixed),
        ambiguities: j.ambiguities.unwrap_or_default(),
        structured_prompt: j.structured_prompt.unwrap_or_default(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Format an [`Intent`] as a one-line dim preamble for the chat panel.
pub fn format_preamble(intent: &Intent) -> String {
    format!(
        "Goal: {}  ·  Scope: {}  ·  Output: {}",
        intent.goal,
        intent.scope.label(),
        intent.expected_output,
    )
}

/// Attempt to translate `message` into a structured [`Intent`].
///
/// Returns `None` — and logs a `warn!` — if translation should be skipped or
/// fails for any reason (message too short, skip pattern match, HTTP error,
/// timeout, or malformed JSON). The caller must fall through to the raw message.
pub async fn translate_intent(
    message: &str,
    ctx: &TranslationContext<'_>,
    settings: &IntentCallSettings<'_>,
) -> Option<Intent> {
    // Slash commands are handled by spec-kit before this runs; skip them.
    if message.trim_start().starts_with('/') {
        return None;
    }
    for pat in settings.skip_patterns {
        if message.trim_start().starts_with(pat.as_str()) {
            return None;
        }
    }
    if message.len() < settings.min_chars_to_translate {
        return None;
    }

    let prompt = build_translator_prompt(message, ctx);

    let body = serde_json::json!({
        "model": settings.model,
        "messages": [{ "role": "user", "content": prompt }],
        "stream": false,
        "temperature": 0.1,
        "max_tokens": 512
    });

    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_millis(settings.timeout_ms))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("[intent] Failed to build HTTP client: {e}");
            return None;
        },
    };

    let mut req = client
        .post(settings.endpoint)
        .header("Content-Type", "application/json")
        .header("User-Agent", "forgiven/0.1.0");

    if settings.provider_kind.requires_auth() && !settings.api_token.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", settings.api_token));
    }
    if *settings.provider_kind == ProviderKind::Copilot {
        req = req
            .header("Copilot-Integration-Id", "vscode-chat")
            .header("editor-version", "forgiven/0.1.0")
            .header("editor-plugin-version", "forgiven-copilot/0.1.0")
            .header("openai-intent", "conversation-panel");
    }
    if *settings.provider_kind == ProviderKind::OpenRouter {
        if !settings.openrouter_site_url.is_empty() {
            req = req.header("HTTP-Referer", settings.openrouter_site_url);
        }
        if !settings.openrouter_app_name.is_empty() {
            req = req.header("X-Title", settings.openrouter_app_name);
        }
    }

    let response = match req.json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("[intent] HTTP call failed: {e}");
            return None;
        },
    };

    if !response.status().is_success() {
        warn!("[intent] Non-2xx status: {}", response.status());
        return None;
    }

    let json: serde_json::Value = match response.json().await {
        Ok(j) => j,
        Err(e) => {
            warn!("[intent] Failed to parse response JSON: {e}");
            return None;
        },
    };

    let content = json["choices"][0]["message"]["content"].as_str()?;

    match parse_intent_json(content) {
        Some(intent) => {
            info!(
                "[intent] Translated: goal={:?} scope={} output={}",
                intent.goal,
                intent.scope.label(),
                intent.expected_output,
            );
            Some(intent)
        },
        None => {
            warn!("[intent] Malformed JSON from model: {content}");
            None
        },
    }
}
