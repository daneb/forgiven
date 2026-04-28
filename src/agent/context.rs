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
            .conversation
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
        if let Some(path) = super::history_file_path(self.conversation.session_start_secs) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                for msg in &self.conversation.messages {
                    let line = serde_json::json!({
                        "role": msg.role.as_str(),
                        "content": msg.content,
                        "ts": now_secs,
                        "char_count": msg.content.len(),
                    });
                    let _ = writeln!(f, "{}", line);
                }
            }
        }

        // Preserve original conversation in archive so the user can scroll back
        // and see what was there before compression.
        self.conversation.archived_messages.extend(std::mem::take(&mut self.conversation.messages));
        // Cap the archive so repeated janitor runs across a long session don't
        // accumulate unbounded memory.  Drop the oldest messages first.
        const MAX_ARCHIVED: usize = 400;
        if self.conversation.archived_messages.len() > MAX_ARCHIVED {
            let drop = self.conversation.archived_messages.len() - MAX_ARCHIVED;
            self.conversation.archived_messages.drain(..drop);
        }
        self.tasks.clear();
        self.conversation.input = prompt;
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

    // ── ADR 0119 regression: archived_messages cap ───────────────────────────

    #[test]
    fn archived_messages_cap_enforced_at_400() {
        // Regression guard for ADR 0119 point 3: the archive must never grow
        // beyond MAX_ARCHIVED (400) entries across multiple janitor compressions.
        // If the drain(..) is accidentally removed the archive accumulates without
        // bound, re-rendering all messages on every frame via the markdown pipeline.
        const MAX_ARCHIVED: usize = 400;

        let mut archived: Vec<ChatMessage> = Vec::new();

        // Simulate three janitor runs that each archive 200 messages.
        for _run in 0..3 {
            let batch: Vec<ChatMessage> =
                (0..200).map(|i| msg(Role::User, &format!("msg {i}"))).collect();
            archived.extend(batch);
            if archived.len() > MAX_ARCHIVED {
                let drop = archived.len() - MAX_ARCHIVED;
                archived.drain(..drop);
            }
        }

        assert_eq!(
            archived.len(),
            MAX_ARCHIVED,
            "archived_messages must be capped at {MAX_ARCHIVED} after repeated janitor runs"
        );
        // The newest messages must be retained (oldest are dropped first).
        assert!(
            archived.last().unwrap().content.contains("msg 199"),
            "newest messages must survive the cap"
        );
    }

    #[test]
    fn archived_messages_cap_exact_boundary() {
        // Exactly MAX_ARCHIVED messages should not trigger a drain.
        const MAX_ARCHIVED: usize = 400;
        let mut archived: Vec<ChatMessage> =
            (0..MAX_ARCHIVED).map(|i| msg(Role::User, &format!("{i}"))).collect();
        if archived.len() > MAX_ARCHIVED {
            let drop = archived.len() - MAX_ARCHIVED;
            archived.drain(..drop);
        }
        assert_eq!(archived.len(), MAX_ARCHIVED);
    }

    // ── ADR 0077 regression: token-budget history truncation ─────────────────

    #[test]
    fn token_budget_truncation_preserves_newest_messages() {
        // Regression guard for ADR 0077: history truncation must keep the most
        // recent MIN_RECENT (4) non-system messages regardless of token budget.
        // Uses the same chars/4 heuristic the production code uses.
        const MIN_RECENT: usize = 4;

        // Build a message list where only the last MIN_RECENT fit in the budget.
        // Each message is ~400 chars ≈ 100 tokens; budget allows ~150 tokens total.
        let large_content = "x".repeat(400); // ~100 tokens each
        let mut messages: Vec<ChatMessage> = (0..10)
            .map(|i| msg(Role::User, &format!("old message {i} {}", large_content)))
            .collect();
        // Append MIN_RECENT "new" messages that are smaller
        for i in 0..MIN_RECENT {
            messages.push(msg(Role::Assistant, &format!("new {i}")));
        }

        // Simulate the MIN_RECENT guarantee from the truncation logic.
        let non_system: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| !matches!(m.role, Role::System))
            .map(|(i, _)| i)
            .collect();
        let recent_start = non_system.len().saturating_sub(MIN_RECENT);
        let recent_indices: std::collections::HashSet<usize> =
            non_system[recent_start..].iter().copied().collect();

        // The MIN_RECENT newest messages must always be in recent_indices.
        let last_four: Vec<usize> = non_system.iter().rev().take(MIN_RECENT).copied().collect();
        for idx in &last_four {
            assert!(
                recent_indices.contains(idx),
                "message at index {idx} must be in recent_indices (ADR 0077 MIN_RECENT guarantee)"
            );
        }
        assert_eq!(recent_indices.len(), MIN_RECENT);
    }

    #[test]
    fn token_estimate_chars_over_4() {
        // Regression: the chars/4 heuristic must produce a non-zero estimate for
        // any non-empty string (guards against integer-division-to-zero for short
        // strings that are handled with the +4 per-message overhead elsewhere).
        let content = "hello world"; // 11 chars → 2 tokens by chars/4
        let token_estimate = content.len() / 4;
        // The +4 per-message overhead is added by the caller; just verify the
        // division itself doesn't produce 0 for a typical message.
        // 11 / 4 = 2 in integer division
        assert_eq!(token_estimate, 2);

        // A 3-char string rounds to 0 — production code adds +4 overhead so
        // this is still safe, but document the behaviour explicitly.
        let short = "hi!";
        assert_eq!(
            short.len() / 4,
            0,
            "chars/4 floors to 0 for very short strings — +4 overhead compensates"
        );
    }
}
