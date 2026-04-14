use super::{AgentPanel, ChatMessage, Role};

// ─────────────────────────────────────────────────────────────────────────────
// Message importance scoring
// ─────────────────────────────────────────────────────────────────────────────

/// Score a message's importance for history retention (higher = keep longer).
///
/// Scores are additive weights used by the importance-scored truncation in
/// `send_message()` to prefer dropping large low-value messages before small
/// high-value ones when the context budget is tight.
pub fn message_importance(msg: &ChatMessage) -> u32 {
    let mut score: u32 = match msg.role {
        Role::User => 3,      // user instructions define the task
        Role::Assistant => 2, // model replies carry context
        Role::System => 0,    // display-only dividers, never sent to API
    };
    let c = &msg.content;
    // Messages containing errors or failures are highly valuable to retain.
    if c.contains("error") || c.contains("Error") || c.contains("failed") || c.contains("panic") {
        score += 3;
    }
    // Large messages that look like raw file reads (line-numbered output) or batch
    // results are low-value once the model has already acted on them.
    if c.len() > 2000 && (c.contains(" | ") || c.starts_with("=== ")) {
        score = score.saturating_sub(2);
    }
    score
}

// ─────────────────────────────────────────────────────────────────────────────
// Token-budget types
// ─────────────────────────────────────────────────────────────────────────────

/// Per-segment token breakdown captured at `submit()` time.
/// Shown in the `SPC d` diagnostics overlay and the status-bar fuel gauge.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextBreakdown {
    /// Tokens used by the system-prompt rules + preamble (without the open file).
    pub sys_rules_t: u32,
    /// Tokens used by the open-file snippet injected into the system prompt.
    pub ctx_file_t: u32,
    /// Tokens used by the chat history sent this round (after truncation).
    pub history_t: u32,
    /// Tokens used by the new user message.
    pub user_msg_t: u32,
    /// Model context window size in tokens.
    pub ctx_window: u32,
}

impl ContextBreakdown {
    pub fn total(&self) -> u32 {
        self.sys_rules_t + self.ctx_file_t + self.history_t + self.user_msg_t
    }

    /// Percentage of the context window consumed (0–100).
    pub fn used_pct(&self) -> u32 {
        self.total() * 100 / self.ctx_window.max(1)
    }
}

/// Context-budget snapshot captured at `submit()` time, correlated with the
/// `StreamEvent::Usage` that arrives after the round completes.
/// Used to write per-invocation metrics to `~/.local/share/forgiven/sessions.jsonl`.
#[derive(Debug, Clone, Copy)]
pub struct SubmitCtx {
    /// Model context window in tokens (from the /models API, or 128k fallback).
    pub ctx_window: u32,
    /// Estimated system-prompt tokens (system.len() / 4).
    pub sys_tokens: u32,
    /// Tokens remaining for history after system-prompt deduction (80% of window − sys).
    pub budget_for_history: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// History compression
// ─────────────────────────────────────────────────────────────────────────────

impl AgentPanel {
    /// Serialises the current non-separator conversation into the user input field
    /// as a single summarisation prompt, clears the message history so the outgoing
    /// API call carries no prior context, and sets `janitor_compressing = true` so
    /// `poll_stream()` knows to replace history with the returned summary.
    ///
    /// The caller is responsible for immediately calling `submit()` after this.
    pub fn compress_history(&mut self) {
        // Collect all non-separator messages (skip Role::System separators like
        // "── New conversation · …" and "── Context compressed · …").
        let history_text: String = self
            .messages
            .iter()
            .filter(|m| {
                // Keep user/assistant messages; drop system separator lines.
                !matches!(m.role, Role::System)
            })
            .map(|m| {
                let label = match m.role {
                    Role::User => "User",
                    Role::Assistant => "Assistant",
                    Role::System => "System",
                };
                format!("**{}:** {}", label, m.content)
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        if history_text.is_empty() {
            // Nothing to compress.
            return;
        }

        let prompt = format!(
            "Summarise the conversation below into the following fixed sections.\n\
             Use EXACTLY these headers — no others.\n\n\
             ## Files changed\n\
             One line per file: `path/to/file — what changed` (omit section if none).\n\n\
             ## Key decisions\n\
             Bullet each architecture or design decision made. Include ADR references if mentioned.\n\n\
             ## Open questions\n\
             Verbatim copy of any question posed to the user that has not yet been answered.\n\n\
             ## Next step\n\
             One sentence: the immediate next action when the session resumes.\n\n\
             ## Context notes\n\
             Any non-obvious facts (gotchas, key file locations, invariants) that would be \
             expensive to re-discover.\n\n\
             Discard completed throwaway tasks, status narration, and tool-call transcripts. \
             Be maximally concise in every section.\n\n\
             <conversation>\n{history_text}\n</conversation>"
        );

        // ── Disk persistence: write full history before archiving ────────────
        // Appends to ~/.local/share/forgiven/history/<session_start_secs>.jsonl
        // so the conversation is recoverable after compression.
        if let Some(path) = super::history_file_path(self.session_start_secs) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                for msg in &self.messages {
                    let line = serde_json::json!({
                        "role": msg.role.as_str(),
                        "content": msg.content,
                        "ts": now_secs,
                    });
                    let _ = writeln!(f, "{}", line);
                }
            }
        }

        // Preserve original conversation in archive so the user can scroll back
        // and see what was there before compression.
        self.archived_messages.extend(std::mem::take(&mut self.messages));
        // Cap the archive so repeated janitor runs across a long session don't
        // accumulate unbounded memory.  Drop the oldest messages first.
        const MAX_ARCHIVED: usize = 400;
        if self.archived_messages.len() > MAX_ARCHIVED {
            let drop = self.archived_messages.len() - MAX_ARCHIVED;
            self.archived_messages.drain(..drop);
        }
        self.tasks.clear();
        self.input = prompt;
        self.janitor_compressing = true;
        // Reset so the warnings can fire again if the next session also approaches the limit.
        self.context_near_limit_warned = false;
        self.session_total_100k_warned = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{ChatMessage, Role};

    fn msg(role: Role, content: &str) -> ChatMessage {
        ChatMessage { role, content: content.to_string(), images: vec![] }
    }

    #[test]
    fn test_message_importance_user_error() {
        // user (3) + error keyword (+3) = 6
        let m = msg(Role::User, "command failed with error: exit code 1");
        assert_eq!(message_importance(&m), 6);
    }

    #[test]
    fn test_message_importance_plain_assistant() {
        // assistant with no special content = 2
        let m = msg(Role::Assistant, "Here is the result.");
        assert_eq!(message_importance(&m), 2);
    }

    #[test]
    fn test_context_breakdown_total() {
        let b = ContextBreakdown {
            sys_rules_t: 100,
            ctx_file_t: 200,
            history_t: 300,
            user_msg_t: 400,
            ctx_window: 10_000,
        };
        assert_eq!(b.total(), 1000);
    }

    #[test]
    fn test_context_breakdown_used_pct() {
        let b = ContextBreakdown {
            sys_rules_t: 1000,
            ctx_file_t: 0,
            history_t: 0,
            user_msg_t: 0,
            ctx_window: 10_000,
        };
        // 1000 / 10000 * 100 = 10%
        assert_eq!(b.used_pct(), 10);
    }
}
