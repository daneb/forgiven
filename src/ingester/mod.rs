use std::sync::Arc;

use crate::mcp::McpManager;

/// Fetch a URL via an MCP `fetch` tool and return the response as a string.
pub async fn fetch_url(mcp: Arc<McpManager>, url: String) -> anyhow::Result<String> {
    let args = build_fetch_args(&url);
    let raw = mcp.call_tool("fetch", &args).await;
    validate_fetch_response(raw, &url)
}

/// Build the JSON arguments string for the MCP `fetch` tool call.
pub(crate) fn build_fetch_args(url: &str) -> String {
    format!(r#"{{"url":"{url}","max_length":50000}}"#)
}

/// Return `Ok(raw)` when non-empty, or an error for blank/whitespace responses.
pub(crate) fn validate_fetch_response(raw: String, url: &str) -> anyhow::Result<String> {
    if raw.trim().is_empty() {
        anyhow::bail!("MCP fetch returned empty response for {url}");
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_fetch_args ──────────────────────────────────────────────────────

    #[test]
    fn args_contains_url() {
        let args = build_fetch_args("https://example.com");
        assert!(args.contains(r#""url":"https://example.com""#));
    }

    #[test]
    fn args_contains_max_length() {
        let args = build_fetch_args("https://example.com");
        assert!(args.contains(r#""max_length":50000"#));
    }

    #[test]
    fn args_is_valid_json() {
        let args = build_fetch_args("https://example.com/path?q=1&r=2");
        let parsed: serde_json::Value =
            serde_json::from_str(&args).expect("build_fetch_args must produce valid JSON");
        assert_eq!(parsed["url"], "https://example.com/path?q=1&r=2");
        assert_eq!(parsed["max_length"], 50000);
    }

    // ── validate_fetch_response ───────────────────────────────────────────────

    #[test]
    fn non_empty_response_passes_through() {
        let result = validate_fetch_response("# Hello\n\nWorld".to_string(), "https://x.com");
        assert_eq!(result.unwrap(), "# Hello\n\nWorld");
    }

    #[test]
    fn empty_string_is_error() {
        let result = validate_fetch_response(String::new(), "https://x.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty response"));
    }

    #[test]
    fn whitespace_only_is_error() {
        let result = validate_fetch_response("   \n\t  ".to_string(), "https://x.com");
        assert!(result.is_err());
    }

    #[test]
    fn error_message_includes_url() {
        let url = "https://missing.example.com";
        let err = validate_fetch_response(String::new(), url).unwrap_err();
        assert!(err.to_string().contains(url), "error should name the failing URL");
    }

    #[test]
    fn single_whitespace_char_is_error() {
        // A response of just "\n" is indistinguishable from empty — reject it.
        let result = validate_fetch_response("\n".to_string(), "https://x.com");
        assert!(result.is_err());
    }

    #[test]
    fn response_with_leading_whitespace_passes() {
        // Content that starts with whitespace but has real text is valid.
        let result = validate_fetch_response("\n\n# Title".to_string(), "https://x.com");
        assert!(result.is_ok());
    }
}
