use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{debug, info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Token types
// ─────────────────────────────────────────────────────────────────────────────

/// Sentinel error returned by `start_chat_stream_with_tools` when the API responds with
/// 401 Unauthorized so that `agentic_loop` can refresh the token and retry the round.
#[derive(Debug)]
pub(super) struct TokenExpiredError;

impl std::fmt::Display for TokenExpiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Copilot API token expired")
    }
}

impl std::error::Error for TokenExpiredError {}

#[derive(Debug, Clone)]
pub(super) struct CopilotApiToken {
    pub token: String,
    pub expires_at: u64,
}

impl CopilotApiToken {
    pub(super) fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now + 60 >= self.expires_at
    }
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    token: String,
    expires_at: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Auth
// ─────────────────────────────────────────────────────────────────────────────

pub(super) fn load_oauth_token() -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = format!("{home}/.config/github-copilot/apps.json");
    let raw = std::fs::read_to_string(&path).with_context(|| format!("Cannot read {path}"))?;
    let val: serde_json::Value =
        serde_json::from_str(&raw).context("apps.json is not valid JSON")?;
    val.as_object()
        .and_then(|m| m.values().next())
        .and_then(|e| e.get("oauth_token"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .context("oauth_token not found in apps.json")
}

/// Load the OAuth token and exchange it for a Copilot API token.
/// Convenience wrapper for callers that don't have access to an `AgentPanel`.
pub async fn acquire_copilot_token() -> Result<String> {
    let oauth = load_oauth_token()?;
    let api_token = exchange_token(&oauth).await?;
    Ok(api_token.token)
}

/// Single non-streaming Copilot completion — for short one-shot tasks such as
/// generating a commit message. Returns the assistant reply as a plain `String`.
pub async fn one_shot_complete(
    api_token: &str,
    model_id: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model_id,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user",   "content": user   }
        ],
        "stream": false,
        "temperature": 0.3,
        "max_tokens": max_tokens
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_default();
    let resp = client
        .post("https://api.githubcopilot.com/chat/completions")
        .header("Authorization", format!("Bearer {api_token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Copilot-Integration-Id", "vscode-chat")
        .header("editor-version", "forgiven/0.1.0")
        .header("editor-plugin-version", "forgiven-copilot/0.1.0")
        .header("openai-intent", "conversation-panel")
        .header("User-Agent", "forgiven/0.1.0")
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("one_shot_complete: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Copilot API error ({status}): {text}"));
    }

    let val: serde_json::Value =
        resp.json().await.context("one_shot_complete: response not JSON")?;
    let content = val["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
    Ok(content)
}

pub(super) async fn exchange_token(oauth_token: &str) -> Result<CopilotApiToken> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    let mut retry_attempts = 0;
    let max_retries = 3;
    let mut delay = tokio::time::Duration::from_secs(1);

    let (status, body_text) = loop {
        match client
            .get("https://api.github.com/copilot_internal/v2/token")
            .header("Authorization", format!("token {oauth_token}"))
            .header("User-Agent", "forgiven/0.1.0")
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(resp) => {
                let s = resp.status();
                let b = resp.text().await.unwrap_or_default();
                debug!("Token exchange response ({s}): {b}");
                // Only retry on server errors or rate limits; fail immediately on 4xx auth errors.
                if s.is_success() || (s.is_client_error() && s.as_u16() != 429) {
                    break (s, b);
                }
                warn!("Token exchange retrying due to server error ({s})");
            },
            Err(e) => {
                warn!("Token exchange retrying due to network error: {e}");
            },
        }
        retry_attempts += 1;
        if retry_attempts >= max_retries {
            return Err(anyhow::anyhow!("Token exchange failed after {max_retries} attempts"));
        }
        tokio::time::sleep(delay).await;
        delay *= 2;
    };

    if !status.is_success() {
        return Err(anyhow::anyhow!("Token exchange failed ({status}): {body_text}"));
    }

    let val: serde_json::Value = serde_json::from_str(&body_text)
        .with_context(|| format!("Token response is not JSON: {body_text}"))?;
    info!("Token response keys: {:?}", val.as_object().map(|o| o.keys().collect::<Vec<_>>()));

    let token_str = val
        .get("token")
        .and_then(|v| v.as_str())
        .with_context(|| format!("No 'token' field in response: {body_text}"))?
        .to_string();
    let expires_at_str = val.get("expires_at").and_then(|v| v.as_str()).map(|s| s.to_string());
    debug!("Copilot API token acquired (expires_at={:?})", expires_at_str);

    let tr = TokenResponse { token: token_str, expires_at: expires_at_str };
    let expires_at = tr.expires_at.as_deref().and_then(chrono_unix_from_iso).unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() + 1800)
            .unwrap_or(1800)
    });

    Ok(CopilotApiToken { token: tr.token, expires_at })
}

fn chrono_unix_from_iso(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let s = if let Some(pos) = s.find('+') { &s[..pos] } else { s };
    let s = if let Some(pos) = s.rfind('-') {
        if pos > 10 {
            &s[..pos]
        } else {
            s
        }
    } else {
        s
    };
    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time: Vec<u64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();
    if date.len() < 3 || time.len() < 3 {
        return None;
    }
    let y = date[0].saturating_sub(1970);
    let days = y * 365 + y / 4 + days_before_month(date[1], date[0]) + date[2] - 1;
    Some(days * 86400 + time[0] * 3600 + time[1] * 60 + time[2])
}

fn days_before_month(month: u64, year: u64) -> u64 {
    let dim = [0u64, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    {
        1
    } else {
        0
    };
    let mut total = 0;
    for m in 1..month.min(13) {
        total += dim[m as usize];
        if m == 2 {
            total += leap;
        }
    }
    total
}
