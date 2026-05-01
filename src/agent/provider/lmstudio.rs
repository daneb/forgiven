#![allow(dead_code)]

use super::ChatProvider;

pub struct LmStudioProvider {
    pub base_url: String,
    pub tool_calls: bool,
    pub planning_tools: bool,
}

impl ChatProvider for LmStudioProvider {
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
        None
    }

    fn api_key(&self) -> &str {
        ""
    }

    fn display_name(&self) -> &str {
        "LM Studio"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(tool_calls: bool, planning: bool) -> LmStudioProvider {
        LmStudioProvider {
            base_url: "http://localhost:1234/v1".to_string(),
            tool_calls,
            planning_tools: planning,
        }
    }

    #[test]
    fn endpoint_uses_base_url() {
        assert_eq!(provider(false, false).endpoint(), "http://localhost:1234/v1/chat/completions");
    }

    #[test]
    fn requires_auth_false() {
        assert!(!provider(false, false).requires_auth());
    }

    #[test]
    fn is_oauth_false() {
        assert!(!provider(false, false).is_oauth());
    }

    #[test]
    fn api_key_empty() {
        assert_eq!(provider(false, false).api_key(), "");
    }

    #[test]
    fn supports_stream_usage_false() {
        assert!(!provider(false, false).supports_stream_usage());
    }

    #[test]
    fn tool_calls_flag_respected() {
        assert!(!provider(false, false).supports_tool_calls());
        assert!(provider(true, false).supports_tool_calls());
    }

    #[test]
    fn planning_tools_flag_respected() {
        assert!(!provider(false, false).supports_planning_tools());
        assert!(provider(false, true).supports_planning_tools());
    }

    #[test]
    fn connect_timeout_longer_than_cloud() {
        assert!(provider(false, false).connect_timeout_secs() > 15);
    }
}
