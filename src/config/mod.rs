// Configuration module

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

/// A single MCP server entry in the config file.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpServerConfig {
    /// Human-readable name shown in the UI.
    pub name: String,
    /// HTTP URL for an externally-managed MCP server (e.g. "http://localhost:8080").
    /// When set, `command`/`args`/`env` are ignored — no process is spawned.
    #[serde(default)]
    pub url: Option<String>,
    /// Executable to spawn for stdio transport.
    #[serde(default)]
    pub command: String,
    /// Arguments passed to the executable (stdio transport only).
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables to set for the server process (stdio only).
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// A single language server entry in the config file.
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
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Optional initialization options forwarded verbatim to the LSP server's
    /// `initialize` request.
    #[serde(default)]
    pub initialization_options: Option<toml::Value>,
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
    /// Visually wrap long lines at the viewport edge instead of scrolling horizontally.
    /// The buffer is unchanged — no newlines are inserted.  Defaults to `false`.
    #[serde(default)]
    pub soft_wrap: bool,
}

fn default_tab_width() -> usize {
    4
}
fn default_use_spaces() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tab_width: default_tab_width(),
            use_spaces: default_use_spaces(),
            lsp: LspConfig::default(),
            mcp: McpConfig::default(),
            soft_wrap: false,
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
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_serialises_cleanly() {
        let original = Config::default();
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.tab_width, 4);
        assert!(parsed.use_spaces);
        assert!(!parsed.soft_wrap);
    }

    #[test]
    fn lsp_server_config_parse() {
        let toml_str = r#"
[[lsp.servers]]
language = "rust"
command  = "rust-analyzer"
args     = ["--extra-arg"]
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.lsp.servers.len(), 1);
        let server = &cfg.lsp.servers[0];
        assert_eq!(server.language, "rust");
        assert_eq!(server.command, "rust-analyzer");
        assert_eq!(server.args, &["--extra-arg"]);
    }

    #[test]
    fn mcp_server_stdio_parse() {
        let toml_str = r#"
[[mcp.servers]]
name    = "filesystem"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.mcp.servers.len(), 1);
        let server = &cfg.mcp.servers[0];
        assert_eq!(server.name, "filesystem");
        assert_eq!(server.command, "npx");
        assert_eq!(server.args.len(), 3);
        assert!(server.url.is_none());
    }

    #[test]
    fn mcp_server_sse_parse() {
        let toml_str = r#"
[[mcp.servers]]
name = "searxng"
url  = "http://localhost:8080"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let server = &cfg.mcp.servers[0];
        assert_eq!(server.name, "searxng");
        assert_eq!(server.url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(server.command, "");
    }
}
