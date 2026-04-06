//! Provider abstraction — selects between GitHub Copilot, Ollama, Anthropic,
//! OpenAI, Google Gemini, and OpenRouter.
//!
//! The active provider is set once at startup from the `[provider]` section of
//! `~/.config/forgiven/config.toml` and is never changed at runtime.  All agent
//! interactions route through the chosen provider's endpoint.  Copilot and all
//! four new providers share the same OpenAI-compatible SSE wire format, so the
//! streaming parser and tool-calling loop are unchanged across all providers.

// ─────────────────────────────────────────────────────────────────────────────
// Provider kind
// ─────────────────────────────────────────────────────────────────────────────

/// Which AI backend the editor is configured to use.
///
/// Set once at startup from `config.toml`; never changed at runtime.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ProviderKind {
    /// GitHub Copilot Enterprise (default) — OAuth-authenticated, cloud-hosted,
    /// OpenAI-compatible SSE streaming via `api.githubcopilot.com`.
    #[default]
    Copilot,
    /// Local Ollama server — no authentication, runs models on the user's machine.
    /// Uses Ollama's OpenAI-compatible `/v1/chat/completions` endpoint so the
    /// same SSE parser works without modification.
    Ollama,
    /// Anthropic direct API — Bearer `ANTHROPIC_API_KEY`, OpenAI-compatible
    /// endpoint at `api.anthropic.com/v1/chat/completions`.
    Anthropic,
    /// OpenAI direct API — Bearer `OPENAI_API_KEY`, standard
    /// `/v1/chat/completions`.  `base_url` may be overridden for Azure OpenAI.
    OpenAi,
    /// Google Gemini — Bearer `GEMINI_API_KEY`, OpenAI-compatible endpoint at
    /// `generativelanguage.googleapis.com/v1beta/openai/`.
    Gemini,
    /// OpenRouter aggregator — Bearer `OPENROUTER_API_KEY`, single key for
    /// 300+ models.  Optional `HTTP-Referer` / `X-Title` headers identify the client.
    OpenRouter,
}

impl ProviderKind {
    /// Parse the `active` string from config into a `ProviderKind`.
    /// Unrecognised values fall back to `Copilot`.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "ollama" => Self::Ollama,
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAi,
            "gemini" => Self::Gemini,
            "openrouter" => Self::OpenRouter,
            _ => Self::Copilot,
        }
    }

    /// Short human-readable name shown in the agent panel title and diagnostics.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Copilot => "Copilot",
            Self::Ollama => "Ollama",
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Gemini",
            Self::OpenRouter => "OpenRouter",
        }
    }

    /// Emoji placed before "You" in the user's message headers.
    pub fn user_emoji(&self) -> &'static str {
        "🧑"
    }

    /// Emoji placed before the AI's name in assistant message headers.
    pub fn ai_emoji(&self) -> &'static str {
        match self {
            Self::Copilot => "🤖",
            Self::Ollama => "🦙",
            Self::Anthropic => "🟠",
            Self::OpenAi => "🟢",
            Self::Gemini => "🔵",
            Self::OpenRouter => "🌐",
        }
    }

    /// Whether this provider requires a `Bearer` token in API requests.
    pub fn requires_auth(&self) -> bool {
        !matches!(self, Self::Ollama)
    }

    /// Whether to include `"stream_options": { "include_usage": true }` in chat
    /// requests.  Ollama's OpenAI-compat layer does not support this field
    /// reliably; sending it may cause the request to fail on older Ollama builds.
    pub fn supports_stream_usage(&self) -> bool {
        !matches!(self, Self::Ollama)
    }

    /// HTTP connect timeout in seconds.
    ///
    /// Ollama needs a longer connect timeout on a cold start because the local
    /// server may need to load the model into RAM before accepting the first
    /// request.  All cloud providers use a tight timeout.
    pub fn connect_timeout_secs(&self) -> u64 {
        match self {
            Self::Ollama => 60,
            _ => 15,
        }
    }

    /// Per-chunk stream timeout in seconds.
    ///
    /// Once a local model is warm, inference is fast — stalled connections should
    /// surface quickly.  Cloud providers need a larger window to accommodate
    /// network jitter and queuing delays.
    pub fn chunk_timeout_secs(&self) -> u64 {
        match self {
            Self::Ollama => 20,
            _ => 60,
        }
    }

    /// Maximum retry attempts for transient failures.
    ///
    /// Ollama is local — network failures are rarely transient and model errors
    /// won't resolve by retrying.  Fail fast so the user sees the error promptly.
    pub fn max_retries(&self) -> usize {
        match self {
            Self::Ollama => 2,
            _ => 5,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API key resolution
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve an API key from config, expanding `$VAR` references.
///
/// If `raw` starts with `$` followed by an uppercase identifier (e.g.
/// `"$ANTHROPIC_API_KEY"`), the value is read from the environment at call
/// time.  Literal strings are returned as-is.  If the env var is unset or
/// empty, an empty string is returned and the first API call will produce a
/// clear 401 error — no silent failure.
///
/// This matches the `$VAR` expansion pattern used for MCP env vars (ADR 0050).
pub fn resolve_api_key(raw: &str) -> String {
    if let Some(var_name) = raw.strip_prefix('$') {
        std::env::var(var_name).unwrap_or_default()
    } else {
        raw.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ollama warmup
// ─────────────────────────────────────────────────────────────────────────────

/// Fire a no-op request to Ollama to preload `model` into RAM before the user
/// sends their first message.  Eliminates the cold-start delay on the first
/// interaction.
///
/// Uses the native `/api/generate` endpoint with no `prompt` field — Ollama
/// loads the model into RAM and returns immediately without generating tokens.
/// `keep_alive` holds the model in RAM for 30 minutes so subsequent requests
/// stay fast.
///
/// Designed to be called via `tokio::spawn` at startup; silently logs any error
/// (Ollama may not be running yet) so it never blocks or panics.
pub async fn warmup_ollama(base_url: String, model: String) {
    use tracing::{info, warn};

    let url = format!("{base_url}/api/generate");
    let body = serde_json::json!({
        "model": model,
        "keep_alive": "30m"
    });

    match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default()
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(120)) // wait while model loads into RAM
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!("[ollama] warmup complete — {model:?} is loaded and ready");
        },
        Ok(resp) => {
            warn!("[ollama] warmup returned unexpected status {}", resp.status());
        },
        Err(e) => {
            warn!("[ollama] warmup failed (Ollama may not be running): {e}");
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Runtime settings
// ─────────────────────────────────────────────────────────────────────────────

/// Runtime configuration threaded through the agentic loop and HTTP layer.
///
/// Built once per `submit()` call from `ProviderKind` + config values.
/// Carries everything the HTTP layer needs so there is no global state.
#[derive(Debug, Clone)]
pub struct ProviderSettings {
    /// Which provider these settings belong to.
    pub kind: ProviderKind,
    /// Bearer token for Copilot/Anthropic/OpenAI/Gemini/OpenRouter requests.
    /// Empty string for Ollama (no auth).
    pub api_token: String,
    /// Full URL of the `/v1/chat/completions` endpoint.
    pub chat_endpoint: String,
    /// `num_ctx` value injected into Ollama's `"options"` field in each request.
    /// Controls the active KV-cache / context window on the Ollama server side.
    ///
    /// Without this, Ollama may use its server default (as low as 4 096 tokens).
    /// Set to `None` for all non-Ollama providers.
    pub num_ctx: Option<u32>,
    /// Whether to send tool definitions and run the agentic tool-calling loop.
    pub supports_tool_calls: bool,
    /// Whether to include planning tools (`create_task`, `complete_task`,
    /// `ask_user`) in the tool list.
    pub planning_tools: bool,
    /// Value for the `HTTP-Referer` header sent to OpenRouter.
    /// Empty for all other providers.
    pub openrouter_site_url: String,
    /// Value for the `X-Title` header sent to OpenRouter.
    /// Empty for all other providers.
    pub openrouter_app_name: String,
}

impl ProviderSettings {
    /// Delegate to `kind.connect_timeout_secs()`.
    pub fn connect_timeout_secs(&self) -> u64 {
        self.kind.connect_timeout_secs()
    }

    /// Delegate to `kind.chunk_timeout_secs()`.
    pub fn chunk_timeout_secs(&self) -> u64 {
        self.kind.chunk_timeout_secs()
    }

    /// Delegate to `kind.supports_stream_usage()`.
    pub fn supports_stream_usage(&self) -> bool {
        self.kind.supports_stream_usage()
    }

    /// Delegate to `kind.requires_auth()`.
    pub fn requires_auth(&self) -> bool {
        self.kind.requires_auth()
    }

    /// Delegate to `kind.max_retries()`.
    pub fn max_retries(&self) -> usize {
        self.kind.max_retries()
    }
}
