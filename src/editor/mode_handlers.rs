use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Editor;
use crate::keymap::Mode;

impl Editor {
    pub(super) fn handle_explorer_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => {
                // Blur explorer, return to editor (keep panel visible)
                self.show_file_info = false;
                self.file_explorer.blur();
                self.mode = Mode::Normal;
            },
            KeyCode::Up | KeyCode::Char('k') => {
                self.file_explorer.move_up();
            },
            KeyCode::Down | KeyCode::Char('j') => {
                self.file_explorer.move_down();
            },
            KeyCode::Enter | KeyCode::Char('l') => {
                let idx = self.file_explorer.cursor_idx;
                let selected = self.file_explorer.selected_path();
                if let Some(path) = selected {
                    if path.is_dir() {
                        self.file_explorer.toggle_node_at(idx);
                    } else {
                        // Open the file and return focus to editor
                        self.file_explorer.blur();
                        self.mode = Mode::Normal;
                        self.open_file(&path)?;
                    }
                }
            },
            // h — toggle hidden files visibility
            KeyCode::Char('h') => {
                self.file_explorer.toggle_hidden();
                let status = if self.file_explorer.show_hidden {
                    "Explorer: showing hidden files"
                } else {
                    "Explorer: hiding hidden files"
                };
                self.set_status(status.to_string());
            },
            // n — new file: pre-fill command mode with "e <dir>/" so the user
            //     only needs to type the filename and press Enter.
            KeyCode::Char('n') => {
                // Resolve the target directory: selected dir, or parent of selected file,
                // or fall back to the explorer root.
                let target_dir = self
                    .file_explorer
                    .selected_path()
                    .map(|p| {
                        if p.is_dir() {
                            p
                        } else {
                            p.parent()
                                .map(|x| x.to_path_buf())
                                .unwrap_or(self.file_explorer.root_path.clone())
                        }
                    })
                    .unwrap_or_else(|| self.file_explorer.root_path.clone());

                // Build a project-relative prefix for readability.
                let rel = target_dir
                    .strip_prefix(&self.file_explorer.root_path)
                    .unwrap_or(&target_dir)
                    .to_string_lossy()
                    .to_string();

                let prefill = if rel.is_empty() { "e ".to_string() } else { format!("e {}/", rel) };

                self.file_explorer.blur();
                self.command_buffer = prefill;
                self.mode = Mode::Command;
            },
            // r — rename selected entry (falls back to reload when nothing is selected).
            // R — reload / refresh the file tree from disk.
            KeyCode::Char('r') => {
                if let Some(path) = self.file_explorer.selected_path() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    self.rename_source = Some(path);
                    self.rename_buffer = name;
                    self.file_explorer.blur();
                    self.mode = Mode::RenameFile;
                } else {
                    self.file_explorer.reload();
                    self.set_status("Explorer refreshed".to_string());
                }
            },
            KeyCode::Char('R') => {
                self.file_explorer.reload();
                self.set_status("Explorer refreshed".to_string());
            },
            // d — delete selected entry (with confirmation popup).
            KeyCode::Char('d') => {
                if let Some(path) = self.file_explorer.selected_path() {
                    self.delete_confirm_path = Some(path);
                    self.file_explorer.blur();
                    self.mode = Mode::DeleteFile;
                }
            },
            // m — create a new folder inside the selected directory (or file's parent).
            KeyCode::Char('m') => {
                let target_dir = self
                    .file_explorer
                    .selected_path()
                    .map(|p| {
                        if p.is_dir() {
                            p
                        } else {
                            p.parent()
                                .map(|x| x.to_path_buf())
                                .unwrap_or(self.file_explorer.root_path.clone())
                        }
                    })
                    .unwrap_or_else(|| self.file_explorer.root_path.clone());
                self.new_folder_parent = Some(target_dir);
                self.new_folder_buffer.clear();
                self.file_explorer.blur();
                self.mode = Mode::NewFolder;
            },
            // i — toggle file-info popup for the selected entry.
            // Navigation (j/k) automatically refreshes the popup by re-computing
            // FileInfoData from the new cursor position on the next frame.
            KeyCode::Char('i') => {
                self.show_file_info = !self.show_file_info;
            },
            _ => {},
        }
        Ok(())
    }

    // ── Rename popup mode key handling ───────────────────────────────────────

    pub(super) fn handle_rename_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.rename_source = None;
                self.rename_buffer.clear();
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            KeyCode::Enter => {
                self.do_rename()?;
            },
            KeyCode::Backspace => {
                self.rename_buffer.pop();
            },
            KeyCode::Char(c) if c != '/' && c != '\\' => {
                self.rename_buffer.push(c);
            },
            _ => {},
        }
        Ok(())
    }

    pub(super) fn do_rename(&mut self) -> Result<()> {
        let new_name = self.rename_buffer.trim().to_string();
        if new_name.is_empty() {
            self.set_status("Rename cancelled: empty name".into());
            self.rename_source = None;
            self.rename_buffer.clear();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            return Ok(());
        }

        if let Some(src) = self.rename_source.take() {
            let dst = src
                .parent()
                .map(|p| p.join(&new_name))
                .ok_or_else(|| anyhow::anyhow!("No parent directory"))?;

            if dst.exists() {
                self.set_status(format!("Rename failed: '{}' already exists", new_name));
                self.rename_source = Some(src); // keep popup open so user can retry
                return Ok(());
            }

            std::fs::rename(&src, &dst)?;

            // Update any open buffer whose path matches the old path
            for buf in &mut self.buffers {
                if buf.file_path.as_deref() == Some(&src) {
                    buf.file_path = Some(dst.clone());
                }
            }

            // Refresh the explorer tree
            self.file_explorer.reload();

            let old_name = src.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            self.rename_buffer.clear();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            self.set_status(format!("Renamed '{}' → '{}'", old_name, new_name));
        }
        Ok(())
    }

    // ── Delete confirmation popup mode key handling ───────────────────────────

    pub(super) fn handle_delete_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.do_delete()?;
            },
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.delete_confirm_path = None;
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            _ => {},
        }
        Ok(())
    }

    // ── Binary file popup mode key handling ───────────────────────────────────

    pub(super) fn handle_binary_file_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('o') => {
                if let Some(ref path) = self.binary_file_path {
                    #[cfg(target_os = "macos")]
                    {
                        std::process::Command::new("open").arg(path).spawn().ok();
                    }
                    #[cfg(target_os = "linux")]
                    {
                        std::process::Command::new("xdg-open").arg(path).spawn().ok();
                    }
                    self.set_status("Opened in default app".to_string());
                }
                self.binary_file_path = None;
                self.mode = Mode::Normal;
            },
            KeyCode::Esc | KeyCode::Char('q') => {
                self.binary_file_path = None;
                self.mode = Mode::Normal;
            },
            _ => {},
        }
        Ok(())
    }

    pub(super) fn do_delete(&mut self) -> Result<()> {
        if let Some(path) = self.delete_confirm_path.take() {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }

            // Close any open buffers under the deleted path (handles dirs too)
            self.buffers.retain(|buf| buf.file_path.as_ref().is_none_or(|p| !p.starts_with(&path)));
            if self.current_buffer_idx >= self.buffers.len() {
                self.current_buffer_idx = self.buffers.len().saturating_sub(1);
            }

            self.file_explorer.reload();

            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            self.set_status(format!("Deleted '{}'", name));
        }
        Ok(())
    }

    // ── New folder popup mode key handling ───────────────────────────────────

    pub(super) fn handle_new_folder_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.new_folder_buffer.clear();
                self.new_folder_parent = None;
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            KeyCode::Enter => {
                self.do_create_folder()?;
            },
            KeyCode::Backspace => {
                self.new_folder_buffer.pop();
            },
            KeyCode::Char(c) if c != '/' && c != '\\' => {
                self.new_folder_buffer.push(c);
            },
            _ => {},
        }
        Ok(())
    }

    pub(super) fn do_create_folder(&mut self) -> Result<()> {
        let name = self.new_folder_buffer.trim().to_string();
        if name.is_empty() {
            self.set_status("Create folder cancelled: empty name".into());
            self.new_folder_buffer.clear();
            self.new_folder_parent = None;
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            return Ok(());
        }

        if let Some(parent) = self.new_folder_parent.take() {
            let new_dir = parent.join(&name);
            if new_dir.exists() {
                self.set_status(format!("Create folder failed: '{}' already exists", name));
                self.new_folder_parent = Some(parent); // keep popup open for retry
                return Ok(());
            }

            std::fs::create_dir_all(&new_dir)?;
            self.file_explorer.reload();

            self.new_folder_buffer.clear();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            self.set_status(format!("Created folder '{}'", name));
        }
        Ok(())
    }

    // ── In-file search mode key handling ─────────────────────────────────────

    // ── Markdown preview mode key handling ────────────────────────────────────

    pub(super) fn handle_preview_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // Esc / q — exit preview, return to Normal
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
            },

            // j / Down — scroll down one line
            KeyCode::Char('j') | KeyCode::Down => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
            },

            // k / Up — scroll up one line
            KeyCode::Char('k') | KeyCode::Up => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
            },

            // Ctrl+D — scroll down half-page (10 lines)
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.preview_scroll = self.preview_scroll.saturating_add(10);
            },

            // Ctrl+U — scroll up half-page (10 lines)
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.preview_scroll = self.preview_scroll.saturating_sub(10);
            },

            // g — jump to top
            KeyCode::Char('g') => {
                self.preview_scroll = 0;
            },

            // G — jump to bottom (approximate — capped in render())
            KeyCode::Char('G') => {
                self.preview_scroll = usize::MAX / 2; // capped by render()
            },

            _ => {},
        }
        Ok(())
    }

    // ── Review changes mode key handling (ADR 0113) ───────────────────────────

    pub(super) fn handle_review_changes_mode(&mut self, key: KeyEvent) -> Result<()> {
        use crate::editor::Verdict;

        match key.code {
            // Quit / cancel — close overlay, return to Normal
            KeyCode::Char('q') | KeyCode::Esc => {
                self.review_changes = None;
                self.mode = Mode::Normal;
            },

            // Scroll down one line
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(s) = self.review_changes.as_mut() {
                    s.scroll = s.scroll.saturating_add(1);
                }
            },

            // Scroll up one line
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(s) = self.review_changes.as_mut() {
                    s.scroll = s.scroll.saturating_sub(1);
                }
            },

            // Scroll down half-page
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(s) = self.review_changes.as_mut() {
                    s.scroll = s.scroll.saturating_add(10);
                }
            },

            // Scroll up half-page
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(s) = self.review_changes.as_mut() {
                    s.scroll = s.scroll.saturating_sub(10);
                }
            },

            // Jump to next file
            KeyCode::Char(']') => {
                if let Some(s) = self.review_changes.as_mut() {
                    let next = (s.focused_file + 1).min(s.diffs.len().saturating_sub(1));
                    s.focused_file = next;
                    s.scroll = s.file_offsets.get(next).copied().unwrap_or(s.scroll);
                }
            },

            // Jump to previous file
            KeyCode::Char('[') => {
                if let Some(s) = self.review_changes.as_mut() {
                    let prev = s.focused_file.saturating_sub(1);
                    s.focused_file = prev;
                    s.scroll = s.file_offsets.get(prev).copied().unwrap_or(s.scroll);
                }
            },

            // Accept focused file (keep current disk state — nothing to write)
            KeyCode::Char('y') => {
                // Phase 1: mark accepted and find next pending
                let (next_focused, next_offset) = {
                    let Some(s) = self.review_changes.as_mut() else {
                        return Ok(());
                    };
                    if let Some(diff) = s.diffs.get_mut(s.focused_file) {
                        diff.verdict = Verdict::Accepted;
                    }
                    let cur = s.focused_file;
                    let next = ((cur + 1)..s.diffs.len())
                        .find(|&i| s.diffs[i].verdict == Verdict::Pending)
                        .unwrap_or(cur);
                    let offset = s.file_offsets.get(next).copied().unwrap_or(s.scroll);
                    (next, offset)
                };
                if let Some(s) = self.review_changes.as_mut() {
                    s.focused_file = next_focused;
                    s.scroll = next_offset;
                }
            },

            // Reject focused file — restore from snapshot
            KeyCode::Char('n') => {
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                // Phase 1: collect what we need without holding mutable borrow
                let (rel_path, needs_restore, cur_focused) = {
                    let Some(s) = self.review_changes.as_ref() else {
                        return Ok(());
                    };
                    let focused = s.focused_file;
                    let diff = &s.diffs[focused];
                    (diff.rel_path.clone(), diff.verdict != Verdict::Rejected, focused)
                };
                // Phase 2: restore file (accesses agent_panel, not review_changes)
                if needs_restore {
                    if let Some(original) =
                        self.agent_panel.session_snapshots.get(&rel_path).cloned()
                    {
                        let abs = project_root.join(&rel_path);
                        if let Some(parent) = abs.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&abs, &original);
                        self.agent_panel.pending_reloads.push(rel_path.clone());
                    }
                }
                // Phase 3: mark rejected + advance focus
                let (next_focused, next_offset) = {
                    let Some(s) = self.review_changes.as_mut() else {
                        return Ok(());
                    };
                    if needs_restore {
                        if let Some(diff) = s.diffs.get_mut(cur_focused) {
                            diff.verdict = Verdict::Rejected;
                        }
                    }
                    let next = ((cur_focused + 1)..s.diffs.len())
                        .find(|&i| s.diffs[i].verdict == Verdict::Pending)
                        .unwrap_or(cur_focused);
                    let offset = s.file_offsets.get(next).copied().unwrap_or(s.scroll);
                    (next, offset)
                };
                if let Some(s) = self.review_changes.as_mut() {
                    s.focused_file = next_focused;
                    s.scroll = next_offset;
                }
            },

            // Accept all pending files
            KeyCode::Char('Y') => {
                if let Some(s) = self.review_changes.as_mut() {
                    for diff in &mut s.diffs {
                        if diff.verdict == Verdict::Pending {
                            diff.verdict = Verdict::Accepted;
                        }
                    }
                }
            },

            // Reject all pending files — restore each from snapshot
            KeyCode::Char('N') => {
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                // Phase 1: collect paths to restore
                let to_restore: Vec<String> = self
                    .review_changes
                    .as_ref()
                    .map(|s| {
                        s.diffs
                            .iter()
                            .filter(|d| d.verdict == Verdict::Pending)
                            .map(|d| d.rel_path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                // Phase 2: write originals back to disk
                for rel_path in &to_restore {
                    if let Some(original) =
                        self.agent_panel.session_snapshots.get(rel_path).cloned()
                    {
                        let abs = project_root.join(rel_path);
                        if let Some(parent) = abs.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&abs, &original);
                        self.agent_panel.pending_reloads.push(rel_path.clone());
                    }
                }
                // Phase 3: mark all as rejected
                if let Some(s) = self.review_changes.as_mut() {
                    for diff in &mut s.diffs {
                        if diff.verdict == Verdict::Pending {
                            diff.verdict = Verdict::Rejected;
                        }
                    }
                }
            },

            _ => {},
        }
        Ok(())
    }
}
