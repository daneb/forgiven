use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use std::time::Instant;
use tokio::sync::oneshot;

use super::Editor;
use crate::keymap::Mode;
use crate::search::{run_search, SearchStatus};

impl Editor {
    pub(super) fn handle_in_file_search_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.in_file_search_buffer.clear();
            },
            KeyCode::Enter => {
                let pattern = self.in_file_search_buffer.clone();
                self.in_file_search_buffer.clear();
                self.mode = Mode::Normal;
                let count = self.with_buffer(|buf| buf.set_search_pattern(pattern)).unwrap_or(0);
                if count == 0 {
                    self.set_status("Pattern not found".to_string());
                } else {
                    self.set_status(format!("{} match(es) found", count));
                }
            },
            KeyCode::Char(c) => {
                self.in_file_search_buffer.push(c);
            },
            KeyCode::Backspace => {
                self.in_file_search_buffer.pop();
            },
            _ => {},
        }
        Ok(())
    }

    pub(super) fn handle_search_mode(&mut self, key: KeyEvent) -> Result<()> {
        use crate::search::SearchFocus;
        match key.code {
            // Esc — close the search overlay, return to Normal.
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.search_rx = None;
                self.last_search_instant = None;
            },

            // Enter — open the selected result at the matched line.
            KeyCode::Enter => {
                if let Some(result) = self.search_state.selected_result() {
                    let path = result.path.clone();
                    let line = result.line;
                    self.mode = Mode::Normal;
                    self.search_rx = None;
                    self.last_search_instant = None;
                    self.open_file(&path)?;
                    self.with_buffer(|buf| buf.goto_line(line + 1)); // goto_line expects 1-based
                }
            },

            // Tab — switch focus between query and glob fields.
            KeyCode::Tab => {
                self.search_state.focus = match self.search_state.focus {
                    SearchFocus::Query => SearchFocus::Glob,
                    SearchFocus::Glob => SearchFocus::Query,
                };
            },

            // Navigation within results list.
            KeyCode::Up | KeyCode::Char('k') => {
                self.search_state.select_up();
            },
            KeyCode::Down | KeyCode::Char('j') => {
                self.search_state.select_down();
            },

            // Text editing in the focused field.
            KeyCode::Backspace => {
                match self.search_state.focus {
                    SearchFocus::Query => {
                        self.search_state.query.pop();
                    },
                    SearchFocus::Glob => {
                        self.search_state.glob.pop();
                    },
                }
                self.on_search_input_changed();
            },
            KeyCode::Char(c) => {
                match self.search_state.focus {
                    SearchFocus::Query => {
                        self.search_state.query.push(c);
                    },
                    SearchFocus::Glob => {
                        self.search_state.glob.push(c);
                    },
                }
                self.on_search_input_changed();
            },

            _ => {},
        }
        Ok(())
    }

    /// Called whenever the query or glob field changes — resets the debounce timer
    /// and cancels any in-flight search so only the settled value is searched.
    pub(super) fn on_search_input_changed(&mut self) {
        self.last_search_instant = Some(Instant::now());
        self.search_state.status = SearchStatus::Running;
        self.search_rx = None; // cancel previous in-flight request
    }

    /// Spawn a tokio task that runs ripgrep and delivers results via oneshot channel.
    pub(super) fn fire_search(&mut self) {
        let query = self.search_state.query.clone();
        let glob = self.search_state.glob.clone();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let (tx, rx) = oneshot::channel();
        self.search_rx = Some(rx);
        self.search_state.status = SearchStatus::Running;
        tokio::spawn(async move {
            let result = run_search(&query, &glob, &cwd).await;
            let _ = tx.send(result);
        });
    }
}
