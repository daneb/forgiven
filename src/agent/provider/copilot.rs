#![allow(dead_code)]

use super::ChatProvider;

pub struct CopilotProvider {
    pub api_base: String,
}

impl ChatProvider for CopilotProvider {
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.api_base)
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        vec![
            ("Copilot-Integration-Id".to_string(), "vscode-chat".to_string()),
            ("editor-version".to_string(), "forgiven/0.1.0".to_string()),
            ("editor-plugin-version".to_string(), "forgiven-copilot/0.1.0".to_string()),
            ("openai-intent".to_string(), "conversation-panel".to_string()),
        ]
    }

    fn format_system_message(&self, system: &str, _context: Option<&str>) -> serde_json::Value {
        serde_json::json!({ "role": "system", "content": system })
    }

    fn requires_auth(&self) -> bool {
        true
    }
    fn is_oauth(&self) -> bool {
        true
    }
    fn supports_tool_calls(&self) -> bool {
        true
    }
    fn supports_stream_usage(&self) -> bool {
        true
    }
    fn supports_planning_tools(&self) -> bool {
        true
    }
    fn connect_timeout_secs(&self) -> u64 {
        15
    }
    fn chunk_timeout_secs(&self) -> u64 {
        60
    }
    fn max_retries(&self) -> usize {
        5
    }
    fn num_ctx(&self) -> Option<u32> {
        None
    }
    fn api_key(&self) -> &str {
        ""
    }
    fn display_name(&self) -> &str {
        "Copilot"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> CopilotProvider {
        CopilotProvider { api_base: "https://api.githubcopilot.com".to_string() }
    }

    #[test]
    fn endpoint_appends_path() {
        assert_eq!(provider().endpoint(), "https://api.githubcopilot.com/chat/completions");
    }

    #[test]
    fn extra_headers_has_four_entries() {
        let headers = provider().extra_headers();
        assert_eq!(headers.len(), 4);
    }

    #[test]
    fn extra_headers_contains_integration_id() {
        let headers = provider().extra_headers();
        assert!(headers.iter().any(|(k, v)| k == "Copilot-Integration-Id" && v == "vscode-chat"));
    }

    #[test]
    fn is_oauth_true() {
        assert!(provider().is_oauth());
    }

    #[test]
    fn requires_auth_true() {
        assert!(provider().requires_auth());
    }

    #[test]
    fn api_key_is_empty() {
        assert_eq!(provider().api_key(), "");
    }

    #[test]
    fn supports_tool_calls() {
        assert!(provider().supports_tool_calls());
    }

    #[test]
    fn num_ctx_is_none() {
        assert!(provider().num_ctx().is_none());
    }
}
