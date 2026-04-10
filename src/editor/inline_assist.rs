use super::state::{InlineAssistPhase, InlineAssistState};
use super::Editor;
use crate::keymap::{Action, Mode};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

impl Editor {
    /// Poll the inline assist stream for new tokens.
    /// Called once per frame from the run loop, alongside `agent_panel.poll_stream()`.
    /// Returns `true` when the frame should be re-rendered.
    pub(super) fn poll_inline_assist(&mut self) -> bool {
        use crate::agent::StreamEvent;
        const MAX_TOKENS_PER_FRAME: usize = 64;

        // Only active during the Generating phase.
        if !matches!(
            self.inline_assist.as_ref().map(|s| s.phase),
            Some(InlineAssistPhase::Generating)
        ) {
            return false;
        }

        let mut active = false;
        let mut token_count = 0usize;
        let mut error: Option<String> = None;

        if let Some(state) = self.inline_assist.as_mut() {
            if let Some(rx) = state.stream_rx.as_mut() {
                loop {
                    match rx.try_recv() {
                        Ok(StreamEvent::Token(t)) => {
                            active = true;
                            state.response.push_str(&t);
                            token_count += 1;
                            if token_count >= MAX_TOKENS_PER_FRAME {
                                break;
                            }
                        },
                        Ok(StreamEvent::Done) => {
                            active = true;
                            // Strip any wrapping code fence the LLM may have added.
                            state.response = strip_assist_fence(&state.response);
                            state.phase = InlineAssistPhase::Preview;
                            break;
                        },
                        Ok(StreamEvent::Error(e)) => {
                            active = true;
                            error = Some(e);
                            break;
                        },
                        // Ignore tool / file / task events — inline assist has no tools.
                        Ok(_) => {
                            active = true;
                        },
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            // Stream ended without explicit Done.
                            active = true;
                            state.response = strip_assist_fence(&state.response);
                            state.phase = InlineAssistPhase::Preview;
                            break;
                        },
                    }
                }
            }
        }

        if let Some(e) = error {
            self.set_status(format!("Inline assist error: {e}"));
            self.inline_assist = None;
            self.mode = Mode::Normal;
        }

        active
    }

    /// Handle key events while `Mode::InlineAssist` is active.
    ///
    /// - `Phase::Input`     — accumulate the user's prompt; Enter submits, Esc cancels.
    /// - `Phase::Generating` — Esc aborts the in-flight request.
    /// - `Phase::Preview`   — Enter accepts the replacement, Esc/q discards it.
    pub(super) fn handle_inline_assist_mode(&mut self, key: KeyEvent) -> Result<()> {
        let phase = match self.inline_assist.as_ref() {
            Some(s) => s.phase,
            None => {
                self.mode = Mode::Normal;
                return Ok(());
            },
        };

        match phase {
            InlineAssistPhase::Input => match key.code {
                KeyCode::Esc => {
                    self.inline_assist = None;
                    self.mode = Mode::Normal;
                },
                KeyCode::Enter => {
                    // Capture all needed values before any async work.
                    let (prompt, selection_text, target_buffer_idx, original_selection, language) =
                        match self.inline_assist.as_ref() {
                            Some(s) => (
                                s.prompt.clone(),
                                s.original_text.clone(),
                                s.target_buffer_idx,
                                s.original_selection.clone(),
                                s.language.clone(),
                            ),
                            None => return Ok(()),
                        };

                    if prompt.trim().is_empty() {
                        self.set_status("Inline assist: prompt cannot be empty".to_string());
                        return Ok(());
                    }

                    let project_root =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

                    // Build the future while agent_panel is mutably borrowed,
                    // then await it inside block_in_place (same pattern as submit()).
                    let fut = self.agent_panel.start_inline_assist(
                        selection_text.clone(),
                        prompt.clone(),
                        project_root,
                        language.clone(),
                    );
                    let result = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(fut)
                    });

                    match result {
                        Ok((rx, abort_tx)) => {
                            self.inline_assist = Some(InlineAssistState {
                                prompt,
                                original_text: selection_text,
                                original_selection,
                                target_buffer_idx,
                                language,
                                response: String::new(),
                                phase: InlineAssistPhase::Generating,
                                stream_rx: Some(rx),
                                abort_tx: Some(abort_tx),
                            });
                        },
                        Err(e) => {
                            self.set_status(format!("Inline assist error: {e}"));
                            self.inline_assist = None;
                            self.mode = Mode::Normal;
                        },
                    }
                },
                KeyCode::Backspace => {
                    if let Some(s) = self.inline_assist.as_mut() {
                        s.prompt.pop();
                    }
                },
                KeyCode::Char(c) => {
                    if let Some(s) = self.inline_assist.as_mut() {
                        s.prompt.push(c);
                    }
                },
                _ => {},
            },

            InlineAssistPhase::Generating => {
                if key.code == KeyCode::Esc {
                    self.inline_assist = None;
                    self.mode = Mode::Normal;
                }
            },

            InlineAssistPhase::Preview => match key.code {
                KeyCode::Enter => {
                    self.execute_action(Action::InlineAssistAccept)?;
                },
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.execute_action(Action::InlineAssistCancel)?;
                },
                _ => {},
            },
        }

        Ok(())
    }
}

/// Strip a wrapping code fence from an inline assist response.
///
/// Many models wrap their output in ` ```lang\n…\n``` ` despite being told not
/// to.  This strips the opening fence (including any language tag) and the
/// closing fence, returning only the code body.
pub(super) fn strip_assist_fence(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Skip optional language tag up to the first newline.
        let body = if let Some(nl) = rest.find('\n') { &rest[nl + 1..] } else { rest };
        // Strip closing fence.
        let body = body.strip_suffix("```").unwrap_or(body).trim_end();
        return body.to_string();
    }
    trimmed.to_string()
}
