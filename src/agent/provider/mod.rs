//! Provider abstraction — Copilot and Ollama only.
//!
//! The active provider is set once at startup from the `[provider]` section of
//! `~/.config/forgiven/config.toml` and is never changed at runtime.
#![allow(dead_code)]

mod copilot;
mod ollama;

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
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider config
// ─────────────────────────────────────────────────────────────────────────────

/// Static per-provider configuration built once from `config.toml`.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Empty for Copilot (uses OAuth) and Ollama (no auth).
    pub api_key: String,
    pub ollama_base_url: String,
    pub ollama_context_length: Option<u32>,
    pub ollama_tool_calls: bool,
    pub ollama_planning_tools: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            api_key: String::new(),
            ollama_base_url: "http://localhost:11434".to_string(),
            ollama_context_length: None,
            ollama_tool_calls: false,
            ollama_planning_tools: false,
        }
    }
}

impl ProviderKind {
    /// Parse the `active` string from config into a `ProviderKind`.
    /// Unrecognised values fall back to `Copilot`.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "ollama" => Self::Ollama,
            _ => Self::Copilot,
        }
    }

    /// Short human-readable name shown in diagnostics.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Copilot => "Copilot",
            Self::Ollama => "Ollama",
        }
    }

    /// Fallback model ID used when not otherwise configured.
    pub fn default_model_id(&self) -> &'static str {
        match self {
            Self::Copilot => "claude-sonnet-4",
            Self::Ollama => "qwen2.5-coder:14b",
        }
    }

    /// Whether this provider requires a `Bearer` token in API requests.
    pub fn requires_auth(&self) -> bool {
        !matches!(self, Self::Ollama)
    }

    /// Whether this provider uses OAuth token exchange for authentication.
    pub fn is_oauth(&self) -> bool {
        matches!(self, Self::Copilot)
    }

    /// HTTP connect timeout in seconds.
    pub fn connect_timeout_secs(&self) -> u64 {
        match self {
            Self::Ollama => 60,
            _ => 15,
        }
    }

    /// Per-chunk stream timeout in seconds.
    pub fn chunk_timeout_secs(&self) -> u64 {
        match self {
            Self::Ollama => 20,
            _ => 60,
        }
    }

    /// The `/chat/completions` endpoint URL for this provider.
    pub fn chat_endpoint(&self, config: &ProviderConfig, copilot_api_base: &str) -> String {
        match self {
            Self::Copilot => format!("{copilot_api_base}/chat/completions"),
            Self::Ollama => format!("{}/v1/chat/completions", config.ollama_base_url),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ollama warmup
// ─────────────────────────────────────────────────────────────────────────────

/// Fire a no-op request to Ollama to preload `model` into RAM before the user
/// sends their first message.
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
        .timeout(std::time::Duration::from_secs(120))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_ollama() {
        assert_eq!(ProviderKind::from_str("ollama"), ProviderKind::Ollama);
    }

    #[test]
    fn from_str_unknown_falls_back_to_copilot() {
        assert_eq!(ProviderKind::from_str("unknown"), ProviderKind::Copilot);
        assert_eq!(ProviderKind::from_str(""), ProviderKind::Copilot);
        assert_eq!(ProviderKind::from_str("anthropic"), ProviderKind::Copilot);
    }

    #[test]
    fn from_str_is_case_insensitive() {
        assert_eq!(ProviderKind::from_str("Ollama"), ProviderKind::Ollama);
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
                > ProviderKind::Copilot.connect_timeout_secs()
        );
    }
}
