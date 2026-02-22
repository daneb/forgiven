// Configuration module
// Phase 1: Basic placeholder
// Phase 6: Full Lua-based configuration system

/// Editor configuration
#[derive(Debug, Clone)]
pub struct Config {
    pub tab_width: usize,
    pub use_spaces: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tab_width: 4,
            use_spaces: true,
        }
    }
}
