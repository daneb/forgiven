// Configuration module
// Phase 1: Basic config + LSP server registration via TOML
// Phase 6: Full Lua-based configuration system

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

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
}

fn default_tab_width() -> usize { 4 }
fn default_use_spaces() -> bool { true }

impl Default for Config {
    fn default() -> Self {
        Self {
            tab_width: default_tab_width(),
            use_spaces: default_use_spaces(),
            lsp: LspConfig::default(),
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
            }
        }
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
