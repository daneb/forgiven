//! Slim agent module — Copilot + Ollama only.
//!
//! Auth flow:
//!   1. Read the GitHub OAuth token from ~/.config/github-copilot/apps.json
//!   2. Exchange it for a short-lived Copilot API token via the GitHub API
//!   3. Stream chat completions from api.githubcopilot.com (OpenAI-compatible SSE)

mod auth;
pub mod provider;
mod submit;

pub use auth::{acquire_copilot_token, CopilotApiToken};
pub use provider::{ProviderConfig, ProviderKind};
pub use submit::start_inline_assist;

/// Events produced by the streaming inline assist call.
///
/// Only the variants needed by inline_assist.rs are present.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Token(String),
    Done,
    Error(String),
}
