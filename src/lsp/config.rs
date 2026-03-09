use std::path::PathBuf;

/// Configuration for a language server
#[derive(Debug, Clone)]
pub struct LanguageServerConfig {
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
    pub file_extensions: Vec<String>,
}

impl LanguageServerConfig {
    /// Get default language server configurations
    pub fn defaults() -> Vec<Self> {
        vec![
            // JavaScript/TypeScript
            Self {
                language: "typescript".to_string(),
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
                file_extensions: vec!["ts".to_string(), "tsx".to_string(), "js".to_string(), "jsx".to_string()],
            },
            // Rust (kept for testing/reference)
            Self {
                language: "rust".to_string(),
                command: "rust-analyzer".to_string(),
                args: vec![],
                file_extensions: vec!["rs".to_string()],
            },
            // Python (kept for testing/reference)
            Self {
                language: "python".to_string(),
                command: "pyright-langserver".to_string(),
                args: vec!["--stdio".to_string()],
                file_extensions: vec!["py".to_string()],
            },
            // Go (kept for testing/reference)
            Self {
                language: "go".to_string(),
                command: "gopls".to_string(),
                args: vec![],
                file_extensions: vec!["go".to_string()],
            },
        ]
    }

    /// Get language server config for a file path
    pub fn for_path(path: &PathBuf) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        
        Self::defaults()
            .into_iter()
            .find(|config| config.file_extensions.iter().any(|ext| ext == extension))
    }
}
