#![allow(dead_code)]

use super::ChatProvider;

pub struct AnthropicProvider {
    pub api_key: String,
}

impl ChatProvider for AnthropicProvider {
    fn endpoint(&self) -> String {
        "https://api.anthropic.com/v1/chat/completions".to_string()
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn format_system_message(&self, system: &str, context: Option<&str>) -> serde_json::Value {
        format_anthropic_system_message(system, context)
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
        "Anthropic"
    }
}

/// Format the system prompt for Anthropic's prompt-caching wire format.
///
/// When a context snippet (currently open file) is present, the system prompt
/// is split at the "Currently open file" boundary so the stable preamble
/// (rules, structural map, tool conventions) is eligible for prompt caching
/// via `"cache_control": {"type": "ephemeral"}`.  The volatile snippet
/// (changes every time the buffer is modified) is left uncached so it doesn't
/// evict the stable prefix from the cache.
///
/// When no context snippet is present, the entire prompt is stable and is
/// cached as a single block.
pub(super) fn format_anthropic_system_message(
    system: &str,
    context: Option<&str>,
) -> serde_json::Value {
    if context.is_some() {
        if let Some(split_pos) = system.find("Currently open file") {
            let stable = &system[..split_pos];
            let volatile = &system[split_pos..];
            return serde_json::json!({
                "role": "system",
                "content": [
                    {
                        "type": "text",
                        "text": stable,
                        "cache_control": { "type": "ephemeral" }
                    },
                    { "type": "text", "text": volatile }
                ]
            });
        }
    }
    // No context snippet, or split point not found — cache the whole prompt.
    serde_json::json!({
        "role": "system",
        "content": [
            {
                "type": "text",
                "text": system,
                "cache_control": { "type": "ephemeral" }
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> AnthropicProvider {
        AnthropicProvider { api_key: "sk-test".to_string() }
    }

    #[test]
    fn endpoint_is_anthropic_direct() {
        assert_eq!(provider().endpoint(), "https://api.anthropic.com/v1/chat/completions");
    }

    #[test]
    fn extra_headers_empty() {
        assert!(provider().extra_headers().is_empty());
    }

    #[test]
    fn is_oauth_false() {
        assert!(!provider().is_oauth());
    }

    #[test]
    fn requires_auth_true() {
        assert!(provider().requires_auth());
    }

    #[test]
    fn api_key_returned() {
        assert_eq!(provider().api_key(), "sk-test");
    }

    #[test]
    fn display_name_is_anthropic() {
        assert_eq!(provider().display_name(), "Anthropic");
    }

    #[test]
    fn format_system_message_no_context_single_cached_block() {
        let msg = provider().format_system_message("You are a helpful assistant.", None);
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(content[0]["text"], "You are a helpful assistant.");
    }

    #[test]
    fn format_system_message_with_context_splits_at_boundary() {
        let system = "You are helpful.\nCurrently open file\n```\nfn main() {}\n```".to_string();
        let msg = provider().format_system_message(&system, Some("fn main() {}"));
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        // First block is cached stable prefix
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert!(content[0]["text"].as_str().unwrap().contains("You are helpful."));
        // Second block is volatile (no cache_control)
        assert!(content[1].get("cache_control").is_none());
        assert!(content[1]["text"].as_str().unwrap().contains("Currently open file"));
    }

    #[test]
    fn format_system_message_context_but_no_split_marker_caches_whole() {
        let system = "No file marker here.".to_string();
        let msg = provider().format_system_message(&system, Some("some context"));
        let content = msg["content"].as_array().unwrap();
        // Falls through to single-block path
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
    }
}
