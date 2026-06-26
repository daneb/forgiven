//! Inline assist — single-turn SSE streaming call (no agentic loop).
//!
//! `start_inline_assist` builds a 2-message conversation, streams the response
//! via the provider's `/chat/completions` endpoint, and emits `StreamEvent`s
//! over an mpsc channel.

use anyhow::Result;
use futures_util::StreamExt as _;
use tokio::sync::{mpsc, oneshot};

use super::{ProviderConfig, ProviderKind, StreamEvent};

/// Launch a single-round, no-tool LLM request for the inline assistant.
///
/// Returns `(receiver, abort_sender)`. Dropping or firing `abort_sender` cancels
/// the in-flight HTTP request.
#[allow(clippy::too_many_arguments)]
pub async fn start_inline_assist(
    provider: &ProviderKind,
    config: &ProviderConfig,
    api_token: Option<&str>,
    copilot_api_base: &str,
    model_id: &str,
    selection_text: String,
    prompt: String,
    language: Option<String>,
) -> Result<(mpsc::Receiver<StreamEvent>, oneshot::Sender<()>)> {
    let lang_str = language.as_deref().unwrap_or("code");
    let system_prompt = format!(
        "You are a {lang_str} code transformation engine. \
        You receive a CODE block and a DIRECTIVE. \
        You output ONLY the transformed {lang_str} code — \
        no conversation, no explanation, no markdown fences, no preamble. \
        If the code is empty, output only what was asked for. \
        Preserve the original indentation."
    );

    let user_content = if selection_text.is_empty() {
        format!("CODE:\n(none)\n\nDIRECTIVE: {prompt}")
    } else {
        format!("CODE:\n{selection_text}\n\nDIRECTIVE: {prompt}")
    };

    let body = serde_json::json!({
        "model": model_id,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_content  }
        ],
        "stream": true,
        "temperature": 0.3
    });

    let endpoint = provider.chat_endpoint(config, copilot_api_base);
    let token = api_token.unwrap_or("").to_string();
    let is_copilot = *provider == ProviderKind::Copilot;

    let (tx, rx) = mpsc::channel::<StreamEvent>(128);
    let (abort_tx, mut abort_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(if is_copilot { 15 } else { 60 }))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                return;
            },
        };

        let mut req = client
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("User-Agent", "forgiven/0.1.0");

        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if is_copilot {
            req = req
                .header("Copilot-Integration-Id", "vscode-chat")
                .header("editor-version", "forgiven/0.1.0")
                .header("editor-plugin-version", "forgiven-copilot/0.1.0")
                .header("openai-intent", "conversation-panel");
        }

        let response = tokio::select! {
            res = req.json(&body).send() => match res {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                    return;
                }
            },
            _ = &mut abort_rx => return,
        };

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            let _ = tx.send(StreamEvent::Error(format!("HTTP {status}: {body_text}"))).await;
            return;
        }

        // Buffer for partial SSE lines across chunks.
        let mut line_buf = String::new();
        let mut stream = response.bytes_stream();

        loop {
            let chunk = tokio::select! {
                chunk = stream.next() => chunk,
                _ = &mut abort_rx => break,
            };
            let bytes = match chunk {
                Some(Ok(b)) => b,
                Some(Err(e)) => {
                    let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                    break;
                },
                None => break,
            };

            let text = String::from_utf8_lossy(&bytes);
            for ch in text.chars() {
                if ch == '\n' {
                    let line = std::mem::take(&mut line_buf);
                    let line = line.trim_end_matches('\r');
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data.trim() == "[DONE]" {
                            let _ = tx.send(StreamEvent::Done).await;
                            return;
                        }
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(content) = val["choices"][0]["delta"]["content"].as_str() {
                                if !content.is_empty() {
                                    let _ = tx.send(StreamEvent::Token(content.to_string())).await;
                                }
                            }
                            // Detect finish_reason == "stop" as implicit Done.
                            if val["choices"][0]["finish_reason"]
                                .as_str()
                                .map(|r| r == "stop")
                                .unwrap_or(false)
                            {
                                let _ = tx.send(StreamEvent::Done).await;
                                return;
                            }
                        }
                    }
                } else {
                    line_buf.push(ch);
                }
            }
        }

        // Stream ended without explicit [DONE]; treat as Done.
        let _ = tx.send(StreamEvent::Done).await;
    });

    Ok((rx, abort_tx))
}
