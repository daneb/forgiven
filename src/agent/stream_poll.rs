use tracing::{info, warn};

use super::{
    append_round_tools, append_session_end_record, append_session_metric, AgentPanel, AgentStatus,
    AgentTask, AskUserInputState, AskUserState, ChatMessage, Role, StreamEvent,
};

impl AgentPanel {
    /// Drain pending stream events from `stream_rx` and the investigation sub-stream.
    ///
    /// Returns `true` if any event was processed (tells the event loop to re-render).
    /// Called from `editor/event_loop.rs` on every tick.
    ///
    /// # Cross-file flag
    /// `self.janitor_compressing` is set in `context.rs::compress_history()` and cleared
    /// here in the `Done` handler.  The flag is the handshake between the two files.
    pub fn poll_stream(&mut self) -> bool {
        // Process at most this many tokens per frame to avoid stalling the render loop
        // when the LLM is streaming a large response at high speed.
        const MAX_TOKENS_PER_FRAME: usize = 64;
        let mut active = false;
        let mut token_count = 0usize;
        if let Some(rx) = self.stream_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(StreamEvent::Token(t)) => {
                        active = true;
                        self.status = if self.janitor_compressing {
                            AgentStatus::Compressing
                        } else {
                            AgentStatus::Streaming { round: self.current_round }
                        };
                        if let Some(r) = self.streaming_reply.as_mut() {
                            r.push_str(&t);
                        }
                        token_count += 1;
                        if token_count >= MAX_TOKENS_PER_FRAME {
                            break;
                        }
                    },
                    Ok(StreamEvent::ToolStart { name, args_summary }) => {
                        active = true;
                        self.status = AgentStatus::CallingTool {
                            round: self.current_round,
                            name: name.clone(),
                        };
                        // Task lifecycle tools are shown in the plan strip — skip them here.
                        if !matches!(name.as_str(), "create_task" | "complete_task") {
                            // Double newline = paragraph break in CommonMark, so each
                            // tool call renders on its own line rather than running together.
                            let line = format!("\n\n⚙  {name}({args_summary})");
                            match self.streaming_reply.as_mut() {
                                Some(r) => r.push_str(&line),
                                None => self.streaming_reply = Some(line),
                            }
                        }
                    },
                    Ok(StreamEvent::ToolDone { name, result_summary, success }) => {
                        active = true;
                        self.status = AgentStatus::WaitingForResponse { round: self.current_round };
                        self.pending_tool_calls.push((name.clone(), success));
                        if !matches!(name.as_str(), "create_task" | "complete_task") {
                            if let Some(r) = self.streaming_reply.as_mut() {
                                r.push_str(&format!(" → {result_summary}"));
                            }
                        }
                    },
                    Ok(StreamEvent::FileModified { path }) => {
                        active = true;
                        self.pending_reloads.push(path);
                    },
                    Ok(StreamEvent::FileSnapshot { path, original }) => {
                        active = true;
                        // Only store the first snapshot per path per session.
                        self.session_snapshots.entry(path).or_insert(original);
                    },
                    Ok(StreamEvent::FileCreated { path }) => {
                        active = true;
                        if !self.session_created_files.contains(&path) {
                            self.session_created_files.push(path);
                        }
                    },
                    Ok(StreamEvent::TaskCreated { title }) => {
                        active = true;
                        self.tasks.push(AgentTask { title, done: false });
                    },
                    Ok(StreamEvent::TaskCompleted { title }) => {
                        active = true;
                        if let Some(t) = self.tasks.iter_mut().find(|t| t.title == title) {
                            t.done = true;
                        }
                    },
                    Ok(StreamEvent::RoundProgress { current, max }) => {
                        active = true;
                        self.current_round = current;
                        self.max_rounds = max;
                        self.status = AgentStatus::WaitingForResponse { round: current };
                    },
                    Ok(StreamEvent::MaxRoundsWarning { current, max, remaining }) => {
                        active = true;
                        let warning = format!(
                            "\n⚠  Agent: {} of {} rounds complete ({} remaining)",
                            current, max, remaining
                        );
                        if let Some(r) = self.streaming_reply.as_mut() {
                            r.push_str(&warning);
                        }
                    },
                    Ok(StreamEvent::AwaitingContinuation) => {
                        active = true;
                        self.awaiting_continuation = true;
                    },
                    Ok(StreamEvent::AskingUser { question, options }) => {
                        active = true;
                        self.asking_user = Some(AskUserState { question, options, selected: 0 });
                    },
                    Ok(StreamEvent::AskingUserInput { question, placeholder }) => {
                        active = true;
                        self.asking_user_input = Some(AskUserInputState {
                            question,
                            placeholder,
                            input: String::new(),
                            cursor: 0,
                        });
                    },
                    Ok(StreamEvent::Retrying { attempt, max }) => {
                        active = true;
                        self.status = AgentStatus::Retrying { attempt, max };
                    },
                    Ok(StreamEvent::Usage { prompt_tokens, completion_tokens, cached_tokens }) => {
                        self.conversation.last_prompt_tokens = prompt_tokens;
                        self.conversation.last_completion_tokens = completion_tokens;
                        self.conversation.last_cached_tokens = cached_tokens;
                        self.usage_received_this_round = true;
                        self.conversation.total_session_prompt_tokens = self
                            .conversation
                            .total_session_prompt_tokens
                            .saturating_add(prompt_tokens);
                        self.conversation.total_session_completion_tokens = self
                            .conversation
                            .total_session_completion_tokens
                            .saturating_add(completion_tokens);
                        let window = if self.available_models.is_empty() {
                            128_000u32
                        } else {
                            self.available_models
                                [self.selected_model.min(self.available_models.len() - 1)]
                            .context_window
                        }
                        .max(1);
                        let pct = prompt_tokens * 100 / window;
                        let cached_note = if cached_tokens > 0 {
                            format!("  cached={cached_tokens}t")
                        } else {
                            String::new()
                        };
                        if pct >= 80 {
                            warn!(
                                "[usage] prompt={prompt_tokens}t ({pct}% of {window}t window)  \
                                 completion={completion_tokens}t{cached_note}  \
                                 session_total={}t",
                                self.conversation.total_session_prompt_tokens
                            );
                        } else {
                            info!(
                                "[usage] prompt={prompt_tokens}t ({pct}% of {window}t window)  \
                                 completion={completion_tokens}t{cached_note}  \
                                 session_total={}t",
                                self.conversation.total_session_prompt_tokens
                            );
                        }
                    },
                    Ok(StreamEvent::ModelSwitched { from, to }) => {
                        active = true;
                        // Unexpected switch = premium quota exceeded; update selection and warn.
                        if let Some(idx) =
                            self.available_models.iter().position(|m| m.id == to || m.version == to)
                        {
                            self.selected_model = idx;
                        }
                        let notice = format!(
                            "\n\n> ⚠  Copilot switched model: **{from}** → **{to}** \
                             (premium quota exceeded)\n\n"
                        );
                        match self.streaming_reply.as_mut() {
                            Some(r) => r.push_str(&notice),
                            None => self.streaming_reply = Some(notice),
                        }
                    },
                    Ok(StreamEvent::Done) => {
                        // Flush accumulated tool calls to history JSONL before
                        // the assistant reply is pushed, so the record is ordered
                        // before the final text response in the file.
                        let round_tools = std::mem::take(&mut self.pending_tool_calls);
                        if !round_tools.is_empty() {
                            append_round_tools(self.conversation.session_start_secs, &round_tools);
                            for (name, success) in &round_tools {
                                if *success {
                                    match name.as_str() {
                                        "read_file" | "read_files" => {
                                            self.session_read_file_count += 1
                                        },
                                        "get_symbol_context" => self.session_symbol_count += 1,
                                        "get_file_outline" => self.session_outline_count += 1,
                                        _ => {},
                                    }
                                }
                            }
                        }
                        if let Some(text) = self.streaming_reply.take() {
                            if !text.is_empty() {
                                self.conversation.messages.push(ChatMessage {
                                    role: Role::Assistant,
                                    content: text,
                                    images: vec![],
                                });
                            }
                        }
                        // ── Auto-Janitor: apply summary if this was a compression round ──
                        if self.janitor_compressing {
                            self.janitor_compressing = false;
                            // The last message is the LLM's summary — extract and rebuild history.
                            let summary = self
                                .conversation
                                .messages
                                .last()
                                .filter(|m| matches!(m.role, Role::Assistant))
                                .map(|m| m.content.clone())
                                .unwrap_or_default();
                            // Write session-end record before clearing counters.
                            if self.conversation.session_rounds > 0 {
                                let files_changed =
                                    self.session_snapshots.len() + self.session_created_files.len();
                                append_session_end_record(
                                    &self.last_submit_model,
                                    self.conversation.total_session_prompt_tokens,
                                    self.conversation.total_session_completion_tokens,
                                    self.conversation.session_rounds,
                                    files_changed,
                                    "janitor",
                                );
                            }
                            // Original messages were already archived in compress_history().
                            // Discard the janitor round (prompt + response) — it's a
                            // technical artifact, not a real conversation turn.
                            self.conversation.messages.clear();
                            self.conversation.total_session_prompt_tokens = 0;
                            self.conversation.total_session_completion_tokens = 0;
                            self.conversation.session_rounds = 0;
                            self.conversation.messages.push(ChatMessage {
                                role: Role::System,
                                content: "── Context compressed by Auto-Janitor ──".to_string(),
                                images: vec![],
                            });
                            if !summary.is_empty() {
                                // Store as a User/Assistant exchange so the model treats
                                // the summary as part of its own conversational memory
                                // rather than as an external system instruction.  This
                                // prevents "context amnesia" where the model ignores the
                                // summary and acts as if starting fresh.
                                self.conversation.messages.push(ChatMessage {
                                    role: Role::User,
                                    content: "Briefly recap what we accomplished.".to_string(),
                                    images: vec![],
                                });
                                self.conversation.messages.push(ChatMessage {
                                    role: Role::Assistant,
                                    content: summary,
                                    images: vec![],
                                });
                            }
                            // Skip metrics append and the threshold check below — the session
                            // counters were just reset.
                            self.code_block_idx = 0;
                            self.mermaid_block_idx = 0;
                            self.scroll = 0;
                            self.stream_rx = None;
                            self.continuation_tx = None;
                            self.question_tx = None;
                            self.asking_user = None;
                            self.asking_user_input = None;
                            self.awaiting_continuation = false;
                            self.current_round = 0;
                            self.status = AgentStatus::JanitorDone;
                            break;
                        }
                        // ── Token estimation fallback (Ollama + providers without usage events) ──
                        // If no StreamEvent::Usage arrived this round, estimate from message
                        // content so the janitor threshold can still fire.
                        if !self.usage_received_this_round {
                            let estimated: u32 = self
                                .conversation
                                .messages
                                .iter()
                                .map(|m| (m.content.len() / 4 + 4) as u32)
                                .sum::<u32>()
                                .max(1);
                            self.conversation.total_session_prompt_tokens = self
                                .conversation
                                .total_session_prompt_tokens
                                .saturating_add(estimated);
                        }
                        self.usage_received_this_round = false;
                        self.round_hint = None; // hint served its purpose after first round
                                                // ── Persist invocation metrics ───────────────────────
                        self.conversation.session_rounds =
                            self.conversation.session_rounds.saturating_add(1);
                        if self.conversation.last_prompt_tokens > 0 {
                            let ts = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let (ctx_window, sys_tokens, budget) = self
                                .last_submit_ctx
                                .map(|c| (c.ctx_window, c.sys_tokens, c.budget_for_history))
                                .unwrap_or((128_000, 0, 0));
                            let pct =
                                self.conversation.last_prompt_tokens * 100 / ctx_window.max(1);
                            append_session_metric(&serde_json::json!({
                                "ts": ts,
                                "model": self.last_submit_model,
                                "prompt_tokens": self.conversation.last_prompt_tokens,
                                "completion_tokens": self.conversation.last_completion_tokens,
                                "cached_tokens": self.conversation.last_cached_tokens,
                                "ctx_window": ctx_window,
                                "sys_tokens": sys_tokens,
                                "budget_for_history": budget,
                                "session_prompt_total": self.conversation.total_session_prompt_tokens,
                                "session_completion_total": self.conversation.total_session_completion_tokens,
                                "pct": pct,
                            }));
                        }
                        // ── 100 k session-total warning ──────────────────────
                        // Fires once per conversation when cumulative re-send cost
                        // crosses 100k tokens — earlier than the 90% per-round check,
                        // giving the user a softer nudge to run the janitor soon.
                        if self.conversation.total_session_prompt_tokens > 100_000
                            && !self.session_total_100k_warned
                        {
                            self.session_total_100k_warned = true;
                            self.conversation.messages.push(ChatMessage {
                                role: Role::System,
                                content: format!(
                                    "\u{2139}  Session total: {}k tokens. \
                                     Consider running SPC a j before your next task.",
                                    self.conversation.total_session_prompt_tokens / 1_000
                                ),
                                images: vec![],
                            });
                        }
                        // ── 90 % context-window warning ──────────────────────
                        // Post a visible chat message the first time a round's prompt
                        // reaches 90 % of the model's context window.  The fuel gauge
                        // in the panel title already turns red at 80 %; this fires a
                        // more actionable in-chat nudge at the higher threshold so the
                        // user knows to run SPC a j before the session hits the limit.
                        if self.conversation.last_prompt_tokens > 0
                            && !self.context_near_limit_warned
                        {
                            let window = self.context_window_size();
                            let pct = self.conversation.last_prompt_tokens * 100 / window.max(1);
                            if pct >= 90 {
                                self.context_near_limit_warned = true;
                                self.conversation.messages.push(ChatMessage {
                                    role: Role::System,
                                    content: format!(
                                        "⚠\u{fe0f}  Context {pct}% full — \
                                         press SPC a j to compress history \
                                         before your next message."
                                    ),
                                    images: vec![],
                                });
                            }
                        }
                        self.code_block_idx = 0;
                        self.mermaid_block_idx = 0;
                        self.scroll = 0;
                        self.stream_rx = None;
                        self.continuation_tx = None;
                        self.question_tx = None;
                        self.asking_user = None;
                        self.asking_user_input = None;
                        self.awaiting_continuation = false;
                        self.current_round = 0;
                        self.status = AgentStatus::Idle;
                        break;
                    },
                    Ok(StreamEvent::Error(e)) => {
                        warn!("Copilot Chat stream error: {}", e);
                        self.conversation.messages.push(ChatMessage {
                            role: Role::Assistant,
                            content: format!("[Error: {e}]"),
                            images: vec![],
                        });
                        self.last_error = Some(e);
                        self.streaming_reply = None;
                        self.stream_rx = None;
                        self.continuation_tx = None;
                        self.question_tx = None;
                        self.asking_user = None;
                        self.asking_user_input = None;
                        self.awaiting_continuation = false;
                        self.current_round = 0;
                        self.status = AgentStatus::Idle;
                        break;
                    },
                    Err(_) => break,
                }
            }
        }

        // ── Investigation subagent drain ─────────────────────────────────────
        // Drain the investigation stream independently of the main stream.
        // On Done, inject the collected summary as a System message so the user
        // can see it and the main session has it in context for the next round.
        if let Some(rx) = self.investigation_rx.as_mut() {
            loop {
                match rx.try_recv() {
                    Ok(StreamEvent::Token(t)) => {
                        active = true;
                        self.investigation_buf.push_str(&t);
                    },
                    Ok(StreamEvent::Done) => {
                        let summary = std::mem::take(&mut self.investigation_buf);
                        if !summary.trim().is_empty() {
                            self.conversation.messages.push(ChatMessage {
                                role: Role::System,
                                content: format!("🔍 Investigation result:\n{summary}"),
                                images: vec![],
                            });
                        }
                        self.investigation_rx = None;
                        self.status = AgentStatus::Idle;
                        active = true;
                        break;
                    },
                    Ok(StreamEvent::Error(e)) => {
                        self.conversation.messages.push(ChatMessage {
                            role: Role::System,
                            content: format!("🔍 Investigation error: {e}"),
                            images: vec![],
                        });
                        self.investigation_rx = None;
                        self.status = AgentStatus::Idle;
                        active = true;
                        break;
                    },
                    Ok(_) => {}, // tool events — investigation is read-only; ignore
                    Err(_) => break,
                }
            }
        }

        active
    }
}
