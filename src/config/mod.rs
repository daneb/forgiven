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
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpServerConfig {
    /// Human-readable name shown in the UI.
    pub name: String,
    /// Executable to spawn (e.g. "npx", "uvx", "/usr/local/bin/my-mcp-server").
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables to set for the server process.
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
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LspConfig {
    #[serde(default)]
    pub servers: Vec<LspServerConfig>,
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
    /// Preferred Copilot model ID (e.g., "gpt-4o", "claude-3.5-sonnet").
    /// Falls back to "gpt-4o" if not set or if the model is no longer available.
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
    "gpt-4o".to_string()
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
