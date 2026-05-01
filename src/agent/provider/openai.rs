#![allow(dead_code)]

use super::ChatProvider;

pub struct OpenAiProvider {
    pub api_key: String,
    pub base_url: String,
}

impl ChatProvider for OpenAiProvider {
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn format_system_message(&self, system: &str, _context: Option<&str>) -> serde_json::Value {
        serde_json::json!({ "role": "system", "content": system })
    }

    fn requires_auth(&self) -> bool {
        true
    }

    fn is_oauth(&self) -> bool {
        false
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
        &self.api_key
    }

    fn display_name(&self) -> &str {
        "OpenAI"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenAiProvider {
        OpenAiProvider {
            api_key: "sk-openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }

    #[test]
    fn endpoint_appends_path() {
        assert_eq!(provider().endpoint(), "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn base_url_override_reflected_in_endpoint() {
        let p = OpenAiProvider {
            api_key: "k".to_string(),
            base_url: "https://MY.openai.azure.com/openai/deployments/gpt-4o".to_string(),
        };
        assert!(p.endpoint().ends_with("/chat/completions"));
        assert!(p.endpoint().contains("azure.com"));
    }

    #[test]
    fn requires_auth_true() {
        assert!(provider().requires_auth());
    }

    #[test]
    fn is_oauth_false() {
        assert!(!provider().is_oauth());
    }

    #[test]
    fn extra_headers_empty() {
        assert!(provider().extra_headers().is_empty());
    }

    #[test]
    fn api_key_returned() {
        assert_eq!(provider().api_key(), "sk-openai");
    }
}
