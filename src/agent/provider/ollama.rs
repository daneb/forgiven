#![allow(dead_code)]

use super::ChatProvider;

pub struct OllamaProvider {
    pub base_url: String,
    pub context_length: Option<u32>,
    pub tool_calls: bool,
    pub planning_tools: bool,
}

impl ChatProvider for OllamaProvider {
    fn endpoint(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn format_system_message(&self, system: &str, _context: Option<&str>) -> serde_json::Value {
        serde_json::json!({ "role": "system", "content": system })
    }

    fn requires_auth(&self) -> bool {
        false
    }

    fn is_oauth(&self) -> bool {
        false
    }

    fn supports_tool_calls(&self) -> bool {
        self.tool_calls
    }

    fn supports_stream_usage(&self) -> bool {
        false
    }

    fn supports_planning_tools(&self) -> bool {
        self.planning_tools
    }

    fn connect_timeout_secs(&self) -> u64 {
        60
    }

    fn chunk_timeout_secs(&self) -> u64 {
        20
    }

    fn max_retries(&self) -> usize {
        2
    }

    fn num_ctx(&self) -> Option<u32> {
        self.context_length
    }

    fn api_key(&self) -> &str {
        ""
    }

    fn display_name(&self) -> &str {
        "Ollama"
    }
}
