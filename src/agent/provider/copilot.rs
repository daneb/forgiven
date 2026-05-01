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
