// Configuration module
// Phase 1: Basic config + LSP server registration via TOML
// Phase 6: Full Lua-based configuration system

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

/// A single MCP server entry in the config file.
///
/// Example (`~/.config/forgiven/config.toml`):
/// ```toml
/// [[mcp.servers]]
/// name    = "filesystem"
/// command = "npx"
/// args    = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
///
/// [[mcp.servers]]
/// name    = "git"
/// command = "uvx"
/// args    = ["mcp-server-git"]
/// ```
///
/// For isolation, wrap the command in a container via a shell wrapper script
/// rather than embedding container logic in the editor config.  See ADR 0053.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpServerConfig {
    /// Human-readable name shown in the UI.
    pub name: String,
    /// Executable to spawn (e.g. "npx", "uvx", "/usr/local/bin/my-mcp-server").
    /// For containerised servers, set this to "docker" and put the `run` args in
    /// `args`, or point to a wrapper script that handles the container invocation.
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables to set for the server process.
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
    /// Preferred Copilot model ID (e.g., "claude-sonnet-4", "gpt-5.1", "gemini-2.5-pro").
    /// Falls back to "claude-sonnet-4" if not set or if the model is no longer available.
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
            default_copilot_model: default_copilot_model(),
            max_agent_rounds: default_max_agent_rounds(),
            agent_warning_threshold: default_agent_warning_threshold(),
        }
    }
}

impl Config {
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
}
