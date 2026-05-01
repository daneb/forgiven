#![allow(dead_code)]

use super::ChatProvider;

pub struct OpenRouterProvider {
    pub api_key: String,
    pub site_url: String,
    pub app_name: String,
}

impl ChatProvider for OpenRouterProvider {
    fn endpoint(&self) -> String {
        "https://openrouter.ai/api/v1/chat/completions".to_string()
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        if !self.site_url.is_empty() {
            headers.push(("HTTP-Referer".to_string(), self.site_url.clone()));
        }
        if !self.app_name.is_empty() {
            headers.push(("X-Title".to_string(), self.app_name.clone()));
        }
        headers
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
        "OpenRouter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenRouterProvider {
        OpenRouterProvider {
            api_key: "sk-or-test".to_string(),
            site_url: "https://myapp.example.com".to_string(),
            app_name: "MyApp".to_string(),
        }
    }

    #[test]
    fn endpoint_is_openrouter() {
        assert!(provider().endpoint().contains("openrouter.ai"));
        assert!(provider().endpoint().ends_with("/chat/completions"));
    }

    #[test]
    fn extra_headers_includes_referer() {
        let headers = provider().extra_headers();
        assert!(headers
            .iter()
            .any(|(k, v)| k == "HTTP-Referer" && v == "https://myapp.example.com"));
    }

    #[test]
    fn extra_headers_includes_title() {
        let headers = provider().extra_headers();
        assert!(headers.iter().any(|(k, v)| k == "X-Title" && v == "MyApp"));
    }

    #[test]
    fn extra_headers_empty_when_no_site_url_or_app_name() {
        let p = OpenRouterProvider {
            api_key: "k".to_string(),
            site_url: String::new(),
            app_name: String::new(),
        };
        assert!(p.extra_headers().is_empty());
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
    fn api_key_returned() {
        assert_eq!(provider().api_key(), "sk-or-test");
    }
}
