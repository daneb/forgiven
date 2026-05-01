//! Provider abstraction — selects between GitHub Copilot, Ollama, Anthropic,
//! OpenAI, Google Gemini, and OpenRouter.
//!
//! The active provider is set once at startup from the `[provider]` section of
//! `~/.config/forgiven/config.toml` and is never changed at runtime.  All agent
//! interactions route through the chosen provider's endpoint.  Copilot and all
//! four new providers share the same OpenAI-compatible SSE wire format, so the
//! streaming parser and tool-calling loop are unchanged across all providers.

mod anthropic;
mod copilot;
mod gemini;
mod ollama;
mod openai;
mod openrouter;

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

// ─────────────────────────────────────────────────────────────────────────────
// Provider config
// ─────────────────────────────────────────────────────────────────────────────

/// Static per-provider configuration built once from `config.toml`.
///
/// Consolidates all provider-specific fields that were previously scattered as
/// individual fields on `AgentPanel`.  Token/quota state (which changes at
/// runtime) lives separately on the panel.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Resolved API key for Anthropic/OpenAI/Gemini/OpenRouter.
    /// Empty for Copilot (uses OAuth) and Ollama (no auth).
    pub api_key: String,
    pub ollama_base_url: String,
    pub ollama_context_length: Option<u32>,
    pub ollama_tool_calls: bool,
    pub ollama_planning_tools: bool,
    /// Base URL for OpenAI (allows Azure overrides).
    pub openai_base_url: String,
    pub openrouter_site_url: String,
    pub openrouter_app_name: String,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            api_key: String::new(),
            ollama_base_url: "http://localhost:11434".to_string(),
            ollama_context_length: None,
            ollama_tool_calls: false,
            ollama_planning_tools: false,
            openai_base_url: "https://api.openai.com/v1".to_string(),
            openrouter_site_url: String::new(),
            openrouter_app_name: String::new(),
        }
    }
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

    /// Whether this provider uses OAuth token exchange for authentication.
    /// Only Copilot requires OAuth; all others use a static API key or no auth.
    pub fn is_oauth(&self) -> bool {
        matches!(self, Self::Copilot)
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

    /// The `/chat/completions` endpoint URL for this provider.
    ///
    /// `copilot_api_base` is the business-API base URL updated on each token
    /// refresh; it is ignored for all non-Copilot providers.  Pass an empty
    /// string or the standard endpoint when the dynamic base is unavailable
    /// (e.g. one-shot calls outside the panel).
    pub fn chat_endpoint(&self, config: &ProviderConfig, copilot_api_base: &str) -> String {
        match self {
            Self::Copilot => format!("{copilot_api_base}/chat/completions"),
            Self::Ollama => format!("{}/v1/chat/completions", config.ollama_base_url),
            Self::Anthropic => "https://api.anthropic.com/v1/chat/completions".to_string(),
            Self::OpenAi => format!("{}/chat/completions", config.openai_base_url),
            Self::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
                    .to_string()
            },
            Self::OpenRouter => "https://openrouter.ai/api/v1/chat/completions".to_string(),
        }
    }

    /// Build the `ProviderSettings` passed to the agentic loop.
    ///
    /// `api_token` must already be resolved (OAuth token for Copilot,
    /// env-expanded key for others, empty string for Ollama).
    /// Override `supports_tool_calls` / `planning_tools` on the returned value
    /// when the caller needs non-default behaviour (e.g. inline assist).
    pub fn build_settings(
        &self,
        config: &ProviderConfig,
        api_token: String,
        copilot_api_base: &str,
    ) -> ProviderSettings {
        ProviderSettings {
            kind: self.clone(),
            api_token,
            chat_endpoint: self.chat_endpoint(config, copilot_api_base),
            num_ctx: if *self == Self::Ollama { config.ollama_context_length } else { None },
            supports_tool_calls: *self != Self::Ollama || config.ollama_tool_calls,
            planning_tools: *self != Self::Ollama || config.ollama_planning_tools,
            openrouter_site_url: config.openrouter_site_url.clone(),
            openrouter_app_name: config.openrouter_app_name.clone(),
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

// ─────────────────────────────────────────────────────────────────────────────
// ChatProvider trait
// ─────────────────────────────────────────────────────────────────────────────

/// Stateless per-provider dispatch. Object-safe (no async, no generics).
///
/// Implemented by both per-provider config structs (panel level, see
/// `make_provider`) and by `ProviderSettings` (runtime snapshot passed into
/// the agentic loop).  The latter allows `start_chat_stream_with_tools` to
/// call `provider.extra_headers()` and `provider.display_name()` without
/// knowing the concrete type.
///
/// Several trait methods (`endpoint`, `requires_auth`, `num_ctx`, etc.) are
/// wired into `ProviderSettings` via identical inherent methods; those call
/// sites do not yet route through the trait.  The `#[allow(dead_code)]` below
/// suppresses the lint until `AgentPanel.provider` is migrated to
/// `BoxedProvider` in the next step.
#[allow(dead_code)]
pub trait ChatProvider: Send + Sync {
    /// Full URL for the `/chat/completions` endpoint.
    fn endpoint(&self) -> String;
    /// Provider-specific extra HTTP headers injected on every chat request.
    /// Copilot adds four routing headers; OpenRouter adds Referer/Title metadata.
    fn extra_headers(&self) -> Vec<(String, String)>;
    /// Format the system prompt for this provider's wire format.
    /// Anthropic splits into cached + volatile content blocks; all others return
    /// a plain `{"role":"system","content":"..."}` JSON object.
    fn format_system_message(&self, system: &str, context: Option<&str>) -> serde_json::Value;
    /// Whether to include `Authorization: Bearer <token>` in API requests.
    fn requires_auth(&self) -> bool;
    /// Whether this provider uses OAuth token exchange rather than a static API key.
    /// Only Copilot returns `true`.
    fn is_oauth(&self) -> bool;
    fn supports_tool_calls(&self) -> bool;
    fn supports_stream_usage(&self) -> bool;
    fn supports_planning_tools(&self) -> bool;
    fn connect_timeout_secs(&self) -> u64;
    fn chunk_timeout_secs(&self) -> u64;
    fn max_retries(&self) -> usize;
    /// Ollama KV-cache size override injected into `"options".num_ctx`.
    /// `None` for all other providers.
    fn num_ctx(&self) -> Option<u32>;
    /// Static API key used in the `Authorization` header.
    /// Empty for Copilot (uses OAuth-derived token) and Ollama (no auth).
    fn api_key(&self) -> &str;
    /// Short human-readable name for logging and UI labels.
    fn display_name(&self) -> &str;
}

/// A heap-allocated, type-erased provider implementation.
#[allow(dead_code)]
pub type BoxedProvider = Box<dyn ChatProvider>;

/// Build a `BoxedProvider` from config.  Called once at startup and again if
/// the Copilot base URL changes after a token refresh.
#[allow(dead_code)]
pub fn make_provider(
    kind: ProviderKind,
    config: &ProviderConfig,
    copilot_api_base: &str,
) -> BoxedProvider {
    match kind {
        ProviderKind::Copilot => {
            Box::new(copilot::CopilotProvider { api_base: copilot_api_base.to_owned() })
        },
        ProviderKind::Ollama => Box::new(ollama::OllamaProvider {
            base_url: config.ollama_base_url.clone(),
            context_length: config.ollama_context_length,
            tool_calls: config.ollama_tool_calls,
            planning_tools: config.ollama_planning_tools,
        }),
        ProviderKind::Anthropic => {
            Box::new(anthropic::AnthropicProvider { api_key: config.api_key.clone() })
        },
        ProviderKind::OpenAi => Box::new(openai::OpenAiProvider {
            api_key: config.api_key.clone(),
            base_url: config.openai_base_url.clone(),
        }),
        ProviderKind::Gemini => {
            Box::new(gemini::GeminiProvider { api_key: config.api_key.clone() })
        },
        ProviderKind::OpenRouter => Box::new(openrouter::OpenRouterProvider {
            api_key: config.api_key.clone(),
            site_url: config.openrouter_site_url.clone(),
            app_name: config.openrouter_app_name.clone(),
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// impl ChatProvider for ProviderSettings
// ─────────────────────────────────────────────────────────────────────────────

impl ChatProvider for ProviderSettings {
    fn endpoint(&self) -> String {
        self.chat_endpoint.clone()
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        match self.kind {
            ProviderKind::Copilot => vec![
                ("Copilot-Integration-Id".to_string(), "vscode-chat".to_string()),
                ("editor-version".to_string(), "forgiven/0.1.0".to_string()),
                ("editor-plugin-version".to_string(), "forgiven-copilot/0.1.0".to_string()),
                ("openai-intent".to_string(), "conversation-panel".to_string()),
            ],
            ProviderKind::OpenRouter => {
                let mut headers = Vec::new();
                if !self.openrouter_site_url.is_empty() {
                    headers.push(("HTTP-Referer".to_string(), self.openrouter_site_url.clone()));
                }
                if !self.openrouter_app_name.is_empty() {
                    headers.push(("X-Title".to_string(), self.openrouter_app_name.clone()));
                }
                headers
            },
            _ => Vec::new(),
        }
    }

    fn format_system_message(&self, system: &str, context: Option<&str>) -> serde_json::Value {
        if self.kind == ProviderKind::Anthropic {
            anthropic::format_anthropic_system_message(system, context)
        } else {
            serde_json::json!({ "role": "system", "content": system })
        }
    }

    fn requires_auth(&self) -> bool {
        self.kind.requires_auth()
    }

    fn is_oauth(&self) -> bool {
        self.kind.is_oauth()
    }

    fn supports_tool_calls(&self) -> bool {
        self.supports_tool_calls
    }

    fn supports_stream_usage(&self) -> bool {
        self.kind.supports_stream_usage()
    }

    fn supports_planning_tools(&self) -> bool {
        self.planning_tools
    }

    fn connect_timeout_secs(&self) -> u64 {
        self.kind.connect_timeout_secs()
    }

    fn chunk_timeout_secs(&self) -> u64 {
        self.kind.chunk_timeout_secs()
    }

    fn max_retries(&self) -> usize {
        self.kind.max_retries()
    }

    fn num_ctx(&self) -> Option<u32> {
        self.num_ctx
    }

    fn api_key(&self) -> &str {
        &self.api_token
    }

    fn display_name(&self) -> &str {
        self.kind.display_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ProviderKind ─────────────────────────────────────────────────────────

    #[test]
    fn from_str_recognises_all_variants() {
        assert_eq!(ProviderKind::from_str("ollama"), ProviderKind::Ollama);
        assert_eq!(ProviderKind::from_str("anthropic"), ProviderKind::Anthropic);
        assert_eq!(ProviderKind::from_str("openai"), ProviderKind::OpenAi);
        assert_eq!(ProviderKind::from_str("gemini"), ProviderKind::Gemini);
        assert_eq!(ProviderKind::from_str("openrouter"), ProviderKind::OpenRouter);
        assert_eq!(ProviderKind::from_str("copilot"), ProviderKind::Copilot);
    }

    #[test]
    fn from_str_unknown_falls_back_to_copilot() {
        assert_eq!(ProviderKind::from_str("unknown"), ProviderKind::Copilot);
        assert_eq!(ProviderKind::from_str(""), ProviderKind::Copilot);
    }

    #[test]
    fn from_str_is_case_insensitive() {
        assert_eq!(ProviderKind::from_str("ANTHROPIC"), ProviderKind::Anthropic);
        assert_eq!(ProviderKind::from_str("Ollama"), ProviderKind::Ollama);
    }

    #[test]
    fn all_variants_have_non_empty_display_name() {
        let variants = [
            ProviderKind::Copilot,
            ProviderKind::Ollama,
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::Gemini,
            ProviderKind::OpenRouter,
        ];
        for v in &variants {
            assert!(!v.display_name().is_empty(), "{v:?} has empty display_name");
        }
    }

    #[test]
    fn ollama_is_not_oauth_and_not_auth() {
        assert!(!ProviderKind::Ollama.is_oauth());
        assert!(!ProviderKind::Ollama.requires_auth());
    }

    #[test]
    fn copilot_is_oauth() {
        assert!(ProviderKind::Copilot.is_oauth());
    }

    #[test]
    fn ollama_has_longer_connect_timeout() {
        assert!(
            ProviderKind::Ollama.connect_timeout_secs()
                > ProviderKind::Anthropic.connect_timeout_secs()
        );
    }

    #[test]
    fn ollama_has_fewer_max_retries() {
        assert!(ProviderKind::Ollama.max_retries() < ProviderKind::Anthropic.max_retries());
    }

    // ── resolve_api_key ──────────────────────────────────────────────────────

    #[test]
    fn resolve_api_key_literal() {
        assert_eq!(resolve_api_key("sk-literal-key"), "sk-literal-key");
    }

    #[test]
    fn resolve_api_key_env_var_unset_returns_empty() {
        // Use a name that is extremely unlikely to be set in CI
        let result = resolve_api_key("$FORGIVEN_TEST_UNSET_VAR_XYZ_12345");
        assert_eq!(result, "");
    }

    // ── ProviderSettings / ChatProvider trait ────────────────────────────────

    fn test_settings(kind: ProviderKind) -> ProviderSettings {
        let config = ProviderConfig::default();
        kind.build_settings(&config, "test-token".to_string(), "https://api.githubcopilot.com")
    }

    #[test]
    fn provider_settings_copilot_endpoint_uses_api_base() {
        let s = test_settings(ProviderKind::Copilot);
        assert!(s.chat_endpoint.contains("githubcopilot.com"));
        assert!(s.chat_endpoint.ends_with("/chat/completions"));
    }

    #[test]
    fn provider_settings_anthropic_endpoint_correct() {
        let s = test_settings(ProviderKind::Anthropic);
        assert!(s.chat_endpoint.contains("anthropic.com"));
    }

    #[test]
    fn provider_settings_ollama_num_ctx_forwarded() {
        let config =
            ProviderConfig { ollama_context_length: Some(65536), ..ProviderConfig::default() };
        let s = ProviderKind::Ollama.build_settings(&config, String::new(), "");
        assert_eq!(s.num_ctx, Some(65536));
    }

    #[test]
    fn provider_settings_non_ollama_num_ctx_none() {
        let s = test_settings(ProviderKind::Anthropic);
        assert!(s.num_ctx.is_none());
    }

    #[test]
    fn provider_settings_ollama_tool_calls_off_by_default() {
        let s = test_settings(ProviderKind::Ollama);
        assert!(!s.supports_tool_calls);
    }

    #[test]
    fn provider_settings_implements_chat_provider() {
        // Compile-time check: ProviderSettings satisfies ChatProvider.
        fn accepts_provider(_: &dyn ChatProvider) {}
        let s = test_settings(ProviderKind::Copilot);
        accepts_provider(&s);
    }
}
