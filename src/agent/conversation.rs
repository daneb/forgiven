use super::ChatMessage;

/// Message history, input box, and per-session token accounting for the agent panel.
///
/// Owned by `AgentPanel` as `pub conversation: Conversation`.  Extracted from the
/// panel god-struct so the input-editing and token-accumulation logic is testable
/// in isolation without the rest of the panel's state.
pub struct Conversation {
    /// Live messages in the current session (sent to the API each round).
    pub messages: Vec<ChatMessage>,
    /// Messages from sessions compressed by the Auto-Janitor.  Rendered above
    /// the live session in a dimmed style.  Excluded from API history.
    /// Cleared by `new_conversation()`.
    pub archived_messages: Vec<ChatMessage>,
    /// Text the user is currently typing in the input box.
    pub input: String,
    /// Byte offset of the cursor within `input`.
    pub input_cursor: usize,
    /// Submitted-input history (oldest first), capped at 50 entries.
    pub input_history: Vec<String>,
    /// Index into `input_history` while browsing (None = at live input).
    pub history_idx: Option<usize>,
    /// Draft input saved when history navigation begins; restored on return.
    pub input_saved: String,
    /// Token counts from the last API response (0 = not yet received).
    pub last_prompt_tokens: u32,
    pub last_completion_tokens: u32,
    /// Tokens served from the provider's prompt cache in the last response.
    pub last_cached_tokens: u32,
    /// Cumulative prompt tokens for the current conversation session.
    pub total_session_prompt_tokens: u32,
    /// Cumulative completion tokens for the current conversation session.
    pub total_session_completion_tokens: u32,
    /// Completed agent invocations in this session.  Incremented on `StreamEvent::Done`.
    pub session_rounds: u32,
    /// Unix timestamp (secs) of the first submit in this session.  0 before first submit.
    pub session_start_secs: u64,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            archived_messages: Vec::new(),
            input: String::new(),
            input_cursor: 0,
            input_history: Vec::new(),
            history_idx: None,
            input_saved: String::new(),
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
            last_cached_tokens: 0,
            total_session_prompt_tokens: 0,
            total_session_completion_tokens: 0,
            session_rounds: 0,
            session_start_secs: 0,
        }
    }

    pub fn input_char(&mut self, ch: char) {
        self.input.insert(self.input_cursor, ch);
        self.input_cursor += ch.len_utf8();
    }

    pub fn input_newline(&mut self) {
        self.input.insert(self.input_cursor, '\n');
        self.input_cursor += 1;
    }

    pub fn cursor_left(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        self.input_cursor =
            self.input[..self.input_cursor].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
    }

    pub fn cursor_right(&mut self) {
        if self.input_cursor >= self.input.len() {
            return;
        }
        let ch = self.input[self.input_cursor..].chars().next().unwrap();
        self.input_cursor += ch.len_utf8();
    }

    pub fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        match self.history_idx {
            None => {
                self.input_saved = self.input.clone();
                let idx = self.input_history.len() - 1;
                self.history_idx = Some(idx);
                self.input = self.input_history[idx].clone();
            },
            Some(0) => {},
            Some(i) => {
                self.history_idx = Some(i - 1);
                self.input = self.input_history[i - 1].clone();
            },
        }
        self.input_cursor = self.input.len();
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            None => {},
            Some(i) if i + 1 >= self.input_history.len() => {
                self.history_idx = None;
                self.input = std::mem::take(&mut self.input_saved);
                self.input_cursor = self.input.len();
            },
            Some(i) => {
                self.history_idx = Some(i + 1);
                self.input = self.input_history[i + 1].clone();
                self.input_cursor = self.input.len();
            },
        }
    }
}
