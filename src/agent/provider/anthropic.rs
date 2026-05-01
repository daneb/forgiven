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
