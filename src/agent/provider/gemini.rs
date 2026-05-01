#![allow(dead_code)]

use super::ChatProvider;

pub struct GeminiProvider {
    pub api_key: String,
}

impl ChatProvider for GeminiProvider {
    fn endpoint(&self) -> String {
        "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions".to_string()
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
        "Gemini"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> GeminiProvider {
        GeminiProvider { api_key: "AIza-test".to_string() }
    }

    #[test]
    fn endpoint_is_google_openai_compat() {
        assert!(provider().endpoint().contains("generativelanguage.googleapis.com"));
        assert!(provider().endpoint().ends_with("/chat/completions"));
    }

    #[test]
    fn display_name_contains_gemini() {
        assert!(provider().display_name().to_lowercase().contains("gemini"));
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
    fn num_ctx_is_none() {
        assert!(provider().num_ctx().is_none());
    }
}
