// Configuration module
// Phase 1: Basic config + LSP server registration via TOML
// Phase 6: Full Lua-based configuration system

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

/// A single MCP server entry in the config file.
///
/// Two transport modes are supported:
///
/// **stdio** — the editor spawns the process and communicates over stdin/stdout:
/// ```toml
/// [[mcp.servers]]
/// name    = "filesystem"
/// command = "npx"
/// args    = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
/// ```
///
/// **HTTP** — connect to an externally-managed server (e.g. a Docker container
/// the user started themselves).  The editor owns no process lifecycle:
/// ```toml
/// [[mcp.servers]]
/// name = "searxng"
/// url  = "http://localhost:8080"
/// ```
/// Start the container once with:
/// ```sh
/// docker run -d --rm -p 8080:8080 isokoliuk/mcp-searxng
/// ```
/// The editor will connect on startup and disconnect cleanly on exit without
/// touching the container.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpServerConfig {
    /// Human-readable name shown in the UI.
    pub name: String,
    /// HTTP URL for an externally-managed MCP server (e.g. "http://localhost:8080").
    /// When set, `command`/`args`/`env` are ignored — no process is spawned.
    #[serde(default)]
    pub url: Option<String>,
    /// Executable to spawn for stdio transport.
    /// Ignored when `url` is set.
    #[serde(default)]
    pub command: String,
    /// Arguments passed to the executable (stdio transport only).
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables to set for the server process (stdio only).
    /// Values beginning with `$` are resolved from the shell environment at
    /// startup (e.g. `GITHUB_TOKEN = "$GITHUB_TOKEN"`).
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// A single language server entry in the config file.
///
/// Example (`~/.config/forgiven/config.toml`):
/// ```toml
/// [[lsp.servers]]
/// language = "rust"
/// command  = "rust-analyzer"
/// args     = []
///
/// # Optional: pass custom initialization_options to the LSP server.
/// # Values are merged with forgiven's built-in defaults (user values win).
/// # Example for OmniSharp — override the analysis timeout:
/// [lsp.servers.initialization_options.RoslynExtensionsOptions]
/// documentAnalysisTimeoutMs = 60000
/// enableImportCompletion    = true
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LspServerConfig {
    /// Language ID (must match the extension mapping in LspManager::language_from_path).
    pub language: String,
    /// Executable name or full path.
    pub command: String,
    /// Optional arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables injected into the server process.
    ///
    /// Values prefixed with `$` are resolved from the host environment at
    /// startup (e.g. `RUSTUP_TOOLCHAIN = "$RUSTUP_TOOLCHAIN"`).
    /// Useful for disambiguating toolchains when multiple Rust installations
    /// coexist (Homebrew + rustup) by setting `RUSTUP_TOOLCHAIN = "stable"`.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Optional initialization options forwarded verbatim to the LSP server's
    /// `initialize` request. Merged with forgiven's built-in defaults; user
    /// values take precedence.
    #[serde(default)]
    pub initialization_options: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LspConfig {
    #[serde(default)]
    pub servers: Vec<LspServerConfig>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Per-provider settings for GitHub Copilot.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CopilotProviderConfig {
    /// Preferred model ID (e.g. `"claude-sonnet-4"`, `"gpt-5.1"`).
    /// Falls back to `"claude-sonnet-4"` if not set or no longer available.
    #[serde(default = "default_copilot_model")]
    pub default_model: String,
}

impl Default for CopilotProviderConfig {
    fn default() -> Self {
        Self { default_model: default_copilot_model() }
    }
}

/// Per-provider settings for a local Ollama server.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OllamaProviderConfig {
    /// Base URL of the Ollama server.
    ///
    /// Default: `"http://localhost:11434"`.
    /// Override to reach a remote Ollama instance (e.g. `"http://192.168.1.10:11434"`).
    #[serde(default = "default_ollama_base_url")]
    pub base_url: String,
    /// Preferred Ollama model tag (e.g. `"qwen2.5-coder:14b"`, `"llama3.3:latest"`).
    /// Must match a tag returned by `ollama list` on the server.
    #[serde(default = "default_ollama_model")]
    pub default_model: String,
    /// Active context-window size in tokens sent to Ollama as `options.num_ctx`.
    ///
    /// Without this, Ollama may use a server default as low as 4 096 tokens.
    /// Recommended values:
    ///
    /// | RAM   | Model | `context_length` |
    /// |-------|-------|-----------------|
    /// | 16 GB | 14 B  | 32768           |
    /// | 24 GB | 14 B  | 65536           |
    ///
    /// Omit to let Ollama choose (uses `OLLAMA_CONTEXT_LENGTH` env var or its
    /// own default, which may be very small for older versions).
    #[serde(default)]
    pub context_length: Option<u32>,
    /// Enable the agentic tool-calling loop for Ollama.
    ///
    /// Defaults to `false`.  Tool-calling behaviour varies widely across Ollama
    /// model versions — many models emit calls as raw JSON text instead of the
    /// structured OpenAI `tool_calls` delta format, which breaks the loop and
    /// shows garbled JSON in the panel.
    ///
    /// Enable only for models you have verified support it:
    /// ```toml
    /// [provider.ollama]
    /// tool_calls = true   # requires qwen2.5-coder:14b + Ollama ≥ 0.5
    /// ```
    #[serde(default)]
    pub tool_calls: bool,
}

impl Default for OllamaProviderConfig {
    fn default() -> Self {
        Self {
            base_url: default_ollama_base_url(),
            default_model: default_ollama_model(),
            context_length: None,
            tool_calls: false,
        }
    }
}

/// Top-level provider selection block (`[provider]` in `config.toml`).
///
/// Example:
/// ```toml
/// [provider]
/// active = "ollama"
///
/// [provider.ollama]
/// base_url       = "http://localhost:11434"
/// default_model  = "qwen2.5-coder:14b"
/// context_length = 32768
///
/// [provider.copilot]
/// default_model = "claude-sonnet-4"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderConfig {
    /// Which provider to use: `"copilot"` (default) or `"ollama"`.
    /// Only one provider is active at a time; switching requires a restart.
    #[serde(default = "default_provider_active")]
    pub active: String,
    /// Copilot-specific settings.
    #[serde(default)]
    pub copilot: CopilotProviderConfig,
    /// Ollama-specific settings.
    #[serde(default)]
    pub ollama: OllamaProviderConfig,
}

fn default_provider_active() -> String {
    "copilot".to_string()
}

fn default_ollama_base_url() -> String {
    "http://localhost:11434".to_string()
}

fn default_ollama_model() -> String {
    "qwen2.5-coder:14b".to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent config
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for the agent panel.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct AgentConfig {
    /// Which prompt framework to enable for slash commands in the agent panel.
    ///
    /// | value            | effect                                               |
    /// |------------------|------------------------------------------------------|
    /// | `"none"` / `""`  | disabled — no slash-command interception (default)  |
    /// | `"spec-kit"`     | built-in Spec-Driven Development workflow           |
    /// | `/path/to/dir`   | custom framework loaded from a directory of `.md`   |
    #[serde(default)]
    pub spec_framework: String,
    /// Automatically compress eligible tool results using LLMLingua before
    /// they are appended to the conversation history.
    ///
    /// Requires a connected MCP server named `"llmlingua"` that exposes a
    /// `compress_text` tool (see `mcp_servers/llmlingua_server.py`).
    ///
    /// Code-reading tools (`read_file`, `get_file_outline`, `get_symbol_context`)
    /// are always excluded — compressing source code corrupts identifiers and
    /// operators.  Only tool results longer than 2 000 characters are compressed;
    /// shorter results are returned unchanged.
    ///
    /// Adds ~100 ms–2 s latency per eligible tool call (CPU BERT inference).
    /// Recommended for heavy sessions where context pressure is the bottleneck.
    #[serde(default)]
    pub auto_compress_tool_results: bool,
}

/// Top-level editor configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_tab_width")]
    pub tab_width: usize,
    #[serde(default = "default_use_spaces")]
    pub use_spaces: bool,
    #[serde(default)]
    pub lsp: LspConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    /// Active provider and per-provider settings.
    /// See [`ProviderConfig`] for the full TOML schema.
    #[serde(default)]
    pub provider: ProviderConfig,
    /// Preferred Copilot model ID — kept for backwards compatibility with configs
    /// that predate the `[provider]` block.  When `provider.active = "copilot"`,
    /// this value seeds `provider.copilot.default_model` if the latter is absent.
    /// Prefer setting `[provider.copilot] default_model` in new configs.
    #[serde(default = "default_copilot_model")]
    pub default_copilot_model: String,
    /// Maximum number of agentic tool-calling rounds before prompting the user.
    /// Prevents runaway loops while allowing user to continue if needed.
    #[serde(default = "default_max_agent_rounds")]
    pub max_agent_rounds: usize,
    /// Warn the user when this many rounds remain before hitting the limit.
    #[serde(default = "default_agent_warning_threshold")]
    pub agent_warning_threshold: usize,
}

fn default_tab_width() -> usize {
    4
}
fn default_use_spaces() -> bool {
    true
}
fn default_copilot_model() -> String {
    "claude-sonnet-4".to_string()
}
fn default_max_agent_rounds() -> usize {
    20
}
fn default_agent_warning_threshold() -> usize {
    3
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tab_width: default_tab_width(),
            use_spaces: default_use_spaces(),
            lsp: LspConfig::default(),
            mcp: McpConfig::default(),
            agent: AgentConfig::default(),
            provider: ProviderConfig::default(),
            default_copilot_model: default_copilot_model(),
            max_agent_rounds: default_max_agent_rounds(),
            agent_warning_threshold: default_agent_warning_threshold(),
        }
    }
}

impl Config {
    /// Return the preferred model ID for the active provider.
    ///
    /// - For `"copilot"`: returns `provider.copilot.default_model`, falling back to
    ///   the legacy `default_copilot_model` field for backwards-compatible configs.
    /// - For `"ollama"`: returns `provider.ollama.default_model`.
    pub fn active_default_model(&self) -> &str {
        match self.provider.active.as_str() {
            "ollama" => &self.provider.ollama.default_model,
            _ => {
                // Honour the legacy top-level field when the new nested field
                // still holds its default ("claude-sonnet-4"), giving precedence
                // to an explicit `[provider.copilot] default_model` setting.
                let nested = &self.provider.copilot.default_model;
                let legacy = &self.default_copilot_model;
                if nested == "claude-sonnet-4" && legacy != "claude-sonnet-4" {
                    legacy
                } else {
                    nested
                }
            },
        }
    }

    /// Load config from `~/.config/forgiven/config.toml`.
    /// Falls back to defaults silently if the file is missing; logs a warning on parse errors.
    pub fn load() -> Self {
        let path = Self::config_path();

        let Some(path) = path else {
            return Self::default();
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(), // file doesn't exist yet
        };

        match toml::from_str::<Config>(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                warn!("Failed to parse config {:?}: {}", path, e);
                Self::default()
            },
        }
    }

    /// Save the current config to `~/.config/forgiven/config.toml`.
    /// Creates the directory if it doesn't exist.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path().ok_or("HOME environment variable not set")?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let toml_string = toml::to_string_pretty(self)?;
        std::fs::write(&path, toml_string)?;
        Ok(())
    }

    /// Return the path to the config file, or `None` if `$HOME` is not set.
    pub fn config_path() -> Option<PathBuf> {
        let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg)
        } else {
            let home = std::env::var("HOME").ok()?;
            PathBuf::from(home).join(".config")
        };
        Some(base.join("forgiven").join("config.toml"))
    }

    /// Return the path to the persistent log file.
    /// `$XDG_DATA_HOME/forgiven/forgiven.log`, falling back to
    /// `$HOME/.local/share/forgiven/forgiven.log`.
    /// Returns `None` if `$HOME` is not set; callers should fall back to `/tmp/forgiven.log`.
    pub fn log_path() -> Option<PathBuf> {
        let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(xdg)
        } else {
            let home = std::env::var("HOME").ok()?;
            PathBuf::from(home).join(".local/share")
        };
        Some(base.join("forgiven").join("forgiven.log"))
    }
}
