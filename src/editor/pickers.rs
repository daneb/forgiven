use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;

use super::Editor;
use crate::keymap::Mode;

impl Editor {
    pub(super) fn handle_pick_buffer_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status_message = None;
            },
            KeyCode::Enter => {
                self.current_buffer_idx = self.buffer_picker_idx;
                self.mode = Mode::Normal;
                self.status_message = None;
            },
            KeyCode::Up | KeyCode::Char('k') if self.buffer_picker_idx > 0 => {
                self.buffer_picker_idx -= 1;
            },
            KeyCode::Down | KeyCode::Char('j')
                if self.buffer_picker_idx + 1 < self.buffers.len() =>
            {
                self.buffer_picker_idx += 1;
            },
            _ => {},
        }
        Ok(())
    }

    /// Handle keys in PickFile mode (fuzzy search).
    pub(super) fn handle_pick_file_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.file_query.clear();
                self.status_message = None;
            },
            KeyCode::Enter => {
                if let Some((path, _)) = self.file_list.get(self.file_picker_idx) {
                    if !Self::is_picker_sentinel(path) {
                        let path_clone = path.clone();
                        self.file_query.clear();
                        self.mode = Mode::Normal;
                        self.open_file(&path_clone)?;
                    }
                }
            },
            KeyCode::Up => {
                let mut idx = self.file_picker_idx;
                while idx > 0 {
                    idx -= 1;
                    if !Self::is_picker_sentinel(&self.file_list[idx].0) {
                        self.file_picker_idx = idx;
                        break;
                    }
                }
            },
            KeyCode::Down => {
                let mut idx = self.file_picker_idx;
                while idx + 1 < self.file_list.len() {
                    idx += 1;
                    if !Self::is_picker_sentinel(&self.file_list[idx].0) {
                        self.file_picker_idx = idx;
                        break;
                    }
                }
            },
            KeyCode::Backspace => {
                self.file_query.pop();
                self.refilter_files();
            },
            KeyCode::Char(c) => {
                self.file_query.push(c);
                self.refilter_files();
            },
            _ => {},
        }
        Ok(())
    }

    /// Score `query` against `candidate` using a subsequence-match algorithm.
    /// Returns `None` if not all query chars appear in order in the candidate.
    /// Returns `Some((score, match_indices))` otherwise; higher score = better match.
    pub(super) fn fuzzy_score(query: &str, candidate: &str) -> Option<(i64, Vec<usize>)> {
        if query.is_empty() {
            return Some((0, vec![]));
        }
        let q_chars: Vec<char> = query.to_lowercase().chars().collect();
        let c_chars: Vec<char> = candidate.to_lowercase().chars().collect();

        // Subsequence scan — find the first left-to-right match
        let mut indices = Vec::with_capacity(q_chars.len());
        let mut qi = 0;
        for (ci, &cc) in c_chars.iter().enumerate() {
            if qi < q_chars.len() && cc == q_chars[qi] {
                indices.push(ci);
                qi += 1;
            }
        }
        if qi < q_chars.len() {
            return None; // not all query chars appeared
        }

        let mut score: i64 = 0;

        // Bonus: consecutive matched characters (runs feel like exact substrings)
        for i in 1..indices.len() {
            if indices[i] == indices[i - 1] + 1 {
                score += 10;
            }
        }

        // Bonus: match starts right after a path separator or word boundary
        for &idx in &indices {
            let prev = if idx == 0 { '/' } else { c_chars[idx - 1] };
            if matches!(prev, '/' | '\\' | '_' | '-' | '.') {
                score += 8;
            }
        }

        // Bonus: first matched char appears late in the path (filename > directory)
        if let Some(&first) = indices.first() {
            // Reward matches that are in the filename portion (after the last /)
            let last_sep =
                c_chars.iter().rposition(|&c| c == '/' || c == '\\').map(|p| p + 1).unwrap_or(0);
            if first >= last_sep {
                score += 15;
            }
        }

        // Penalty: longer paths score slightly lower (prefer direct matches)
        score -= candidate.len() as i64 / 6;

        Some((score, indices))
    }

    /// Rebuild `file_list` from `file_all` applying the current `file_query`.
    /// Results are sorted by fuzzy score descending; `file_picker_idx` is clamped.
    pub(super) fn refilter_files(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        if self.file_query.is_empty() {
            // No query → show recent files (scoped to cwd) first, then all project files.
            let recents: Vec<PathBuf> = self
                .recent_files
                .iter()
                .filter(|p| p.exists() && p.starts_with(&cwd))
                .cloned()
                .collect();

            let mut result: Vec<(PathBuf, Vec<usize>)> = Vec::new();

            if !recents.is_empty() {
                // PathBuf::new() (empty)  → "─── Recent ───" header sentinel.
                // PathBuf::from("\x01")   → closing divider sentinel.
                result.push((PathBuf::new(), vec![]));
                for p in &recents {
                    result.push((p.clone(), vec![]));
                }
                result.push((PathBuf::from("\x01"), vec![]));
            }

            for p in &self.file_all {
                if !recents.contains(p) {
                    result.push((p.clone(), vec![]));
                }
            }

            self.file_list = result;
            // Place cursor on the first recent file (index 1, skipping the sentinel header).
            self.file_picker_idx = if recents.is_empty() { 0 } else { 1 };
            return;
        } else {
            let mut scored: Vec<(i64, PathBuf, Vec<usize>)> = self
                .file_all
                .iter()
                .filter_map(|p| {
                    let display = p.strip_prefix(&cwd).unwrap_or(p).to_string_lossy().to_string();
                    Self::fuzzy_score(&self.file_query, &display)
                        .map(|(score, idxs)| (score, p.clone(), idxs))
                })
                .collect();
            scored.sort_by_key(|b| std::cmp::Reverse(b.0));
            self.file_list = scored.into_iter().map(|(_, p, idxs)| (p, idxs)).collect();
        }

        // Clamp selection index
        if self.file_list.is_empty() {
            self.file_picker_idx = 0;
        } else {
            self.file_picker_idx = self.file_picker_idx.min(self.file_list.len() - 1);
        }
    }

    /// Open the file-context picker (agent panel removed — no-op in slim build).
    #[allow(dead_code)]
    pub(super) fn open_at_picker(&mut self) {
        self.set_status("File-context picker removed in slim build".to_string());
    }

    /// Handle a key event while the file-context picker is open (no-op in slim build).
    #[allow(dead_code)]
    pub(super) fn handle_at_picker_key(&mut self, _key: KeyEvent) -> Result<()> {
        Ok(())
    }
}
