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
                    s.focused_hunk = None;
                    s.scroll = s.file_offsets.get(next).copied().unwrap_or(s.scroll);
                }
            },

            // Jump to previous file
            KeyCode::Char('[') => {
                if let Some(s) = self.review_changes.as_mut() {
                    let prev = s.focused_file.saturating_sub(1);
                    s.focused_file = prev;
                    s.focused_hunk = None;
                    s.scroll = s.file_offsets.get(prev).copied().unwrap_or(s.scroll);
                }
            },

            // Tab — advance to next hunk (within current file, then next file)
            KeyCode::Tab => {
                if let Some(s) = self.review_changes.as_mut() {
                    let fi = s.focused_file;
                    let hunk_count = s.diffs.get(fi).map_or(0, |d| d.hunk_verdicts.len());
                    let next_hunk = match s.focused_hunk {
                        None => {
                            if hunk_count > 0 {
                                Some(0)
                            } else {
                                None
                            }
                        },
                        Some(h) if h + 1 < hunk_count => Some(h + 1),
                        _ => {
                            // Wrap to first hunk of next file
                            let next_fi = (fi + 1).min(s.diffs.len().saturating_sub(1));
                            if next_fi != fi {
                                s.focused_file = next_fi;
                            }
                            let nc =
                                s.diffs.get(s.focused_file).map_or(0, |d| d.hunk_verdicts.len());
                            if nc > 0 {
                                Some(0)
                            } else {
                                None
                            }
                        },
                    };
                    s.focused_hunk = next_hunk;
                    // Scroll to the focused hunk
                    if let Some(h) = next_hunk {
                        if let Some(offset) =
                            s.hunk_line_offsets.get(s.focused_file).and_then(|v| v.get(h))
                        {
                            s.scroll = *offset;
                        }
                    }
                }
            },

            // Shift+Tab — go to previous hunk
            KeyCode::BackTab => {
                if let Some(s) = self.review_changes.as_mut() {
                    let fi = s.focused_file;
                    let prev_hunk = match s.focused_hunk {
                        None | Some(0) => {
                            // Wrap to last hunk of previous file
                            if fi > 0 {
                                s.focused_file = fi - 1;
                            }
                            let nc =
                                s.diffs.get(s.focused_file).map_or(0, |d| d.hunk_verdicts.len());
                            if nc > 0 {
                                Some(nc - 1)
                            } else {
                                None
                            }
                        },
                        Some(h) => Some(h - 1),
                    };
                    s.focused_hunk = prev_hunk;
                    if let Some(h) = prev_hunk {
                        if let Some(offset) =
                            s.hunk_line_offsets.get(s.focused_file).and_then(|v| v.get(h))
                        {
                            s.scroll = *offset;
                        }
                    }
                }
            },

            // Accept focused hunk (or file if no hunk focused)
            KeyCode::Char('y') => {
                let hunk_focused =
                    self.review_changes.as_ref().is_some_and(|s| s.focused_hunk.is_some());
                if hunk_focused {
                    return self.review_accept_focused_hunk();
                }
                // File-level accept: all hunks → Accepted, advance to next pending file
                let (next_focused, next_offset) = {
                    let Some(s) = self.review_changes.as_mut() else {
                        return Ok(());
                    };
                    let cur = s.focused_file;
                    if let Some(diff) = s.diffs.get_mut(cur) {
                        for v in &mut diff.hunk_verdicts {
                            *v = Verdict::Accepted;
                        }
                    }
                    let next = ((cur + 1)..s.diffs.len())
                        .find(|&i| s.diffs[i].file_verdict() == Verdict::Pending)
                        .unwrap_or(cur);
                    let offset = s.file_offsets.get(next).copied().unwrap_or(s.scroll);
                    (next, offset)
                };
                if let Some(s) = self.review_changes.as_mut() {
                    s.focused_file = next_focused;
                    s.focused_hunk = None;
                    s.scroll = next_offset;
                }
            },

            // Reject focused hunk (or file if no hunk focused)
            KeyCode::Char('n') => {
                let hunk_focused =
                    self.review_changes.as_ref().is_some_and(|s| s.focused_hunk.is_some());
                if hunk_focused {
                    return self.review_reject_focused_hunk();
                }
                // File-level reject: all hunks → Rejected, write original to disk
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let (rel_path, original, already_rejected, cur_focused) = {
                    let Some(s) = self.review_changes.as_ref() else {
                        return Ok(());
                    };
                    let focused = s.focused_file;
                    let diff = &s.diffs[focused];
                    (
                        diff.rel_path.clone(),
                        diff.original.clone(),
                        diff.file_verdict() == Verdict::Rejected,
                        focused,
                    )
                };
                if !already_rejected {
                    let abs = project_root.join(&rel_path);
                    if let Some(parent) = abs.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if original.is_empty() {
                        // Newly created file: delete it
                        let _ = std::fs::remove_file(&abs);
                    } else {
                        let _ = std::fs::write(&abs, &original);
                    }
                    self.agent_panel.pending_reloads.push(rel_path.clone());
                }
                let (next_focused, next_offset) = {
                    let Some(s) = self.review_changes.as_mut() else {
                        return Ok(());
                    };
                    if let Some(diff) = s.diffs.get_mut(cur_focused) {
                        for v in &mut diff.hunk_verdicts {
                            *v = Verdict::Rejected;
                        }
                    }
                    let next = ((cur_focused + 1)..s.diffs.len())
                        .find(|&i| s.diffs[i].file_verdict() == Verdict::Pending)
                        .unwrap_or(cur_focused);
                    let offset = s.file_offsets.get(next).copied().unwrap_or(s.scroll);
                    (next, offset)
                };
                if let Some(s) = self.review_changes.as_mut() {
                    s.focused_file = next_focused;
                    s.focused_hunk = None;
                    s.scroll = next_offset;
                }
            },

            // Accept focused hunk explicitly (also works when hunk is highlighted)
            KeyCode::Char('a') => {
                return self.review_accept_focused_hunk();
            },

            // Reject focused hunk explicitly
            KeyCode::Char('r') => {
                return self.review_reject_focused_hunk();
            },

            // Accept all pending files (all hunks → Accepted)
            KeyCode::Char('Y') => {
                if let Some(s) = self.review_changes.as_mut() {
                    for diff in &mut s.diffs {
                        for v in &mut diff.hunk_verdicts {
                            if *v == Verdict::Pending {
                                *v = Verdict::Accepted;
                            }
                        }
                    }
                }
            },

            // Reject all pending files — restore each from its original
            KeyCode::Char('N') => {
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                // Collect (rel_path, original) for all pending files
                let to_restore: Vec<(String, String)> = self
                    .review_changes
                    .as_ref()
                    .map(|s| {
                        s.diffs
                            .iter()
                            .filter(|d| d.file_verdict() == Verdict::Pending)
                            .map(|d| (d.rel_path.clone(), d.original.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                for (rel_path, original) in &to_restore {
                    let abs = project_root.join(rel_path);
                    if let Some(parent) = abs.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if original.is_empty() {
                        let _ = std::fs::remove_file(&abs);
                    } else {
                        let _ = std::fs::write(&abs, original);
                    }
                    self.agent_panel.pending_reloads.push(rel_path.clone());
                }
                if let Some(s) = self.review_changes.as_mut() {
                    for diff in &mut s.diffs {
                        if diff.file_verdict() == Verdict::Pending {
                            for v in &mut diff.hunk_verdicts {
                                *v = Verdict::Rejected;
                            }
                        }
                    }
                }
            },

            _ => {},
        }
        Ok(())
    }

    /// Accept the currently focused hunk and write the effective file content to disk.
    fn review_accept_focused_hunk(&mut self) -> Result<()> {
        use crate::editor::{apply_hunk_verdicts, Verdict};
        let project_root =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let (fi, hunk_idx, rel_path, original, agent_version) = {
            let Some(s) = self.review_changes.as_ref() else {
                return Ok(());
            };
            let fi = s.focused_file;
            let Some(h) = s.focused_hunk else {
                return Ok(());
            };
            let diff = &s.diffs[fi];
            (fi, h, diff.rel_path.clone(), diff.original.clone(), diff.agent_version.clone())
        };
        // Mark hunk accepted
        if let Some(s) = self.review_changes.as_mut() {
            if let Some(diff) = s.diffs.get_mut(fi) {
                if let Some(v) = diff.hunk_verdicts.get_mut(hunk_idx) {
                    *v = Verdict::Accepted;
                }
                // Write effective content to disk
                let content = apply_hunk_verdicts(&original, &agent_version, &diff.hunk_verdicts);
                let abs = project_root.join(&rel_path);
                if let Some(parent) = abs.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&abs, &content);
            }
        }
        self.agent_panel.pending_reloads.push(rel_path);
        Ok(())
    }

    /// Reject the currently focused hunk and write the effective file content to disk.
    fn review_reject_focused_hunk(&mut self) -> Result<()> {
        use crate::editor::{apply_hunk_verdicts, Verdict};
        let project_root =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let (fi, hunk_idx, rel_path, original, agent_version) = {
            let Some(s) = self.review_changes.as_ref() else {
                return Ok(());
            };
            let fi = s.focused_file;
            let Some(h) = s.focused_hunk else {
                return Ok(());
            };
            let diff = &s.diffs[fi];
            (fi, h, diff.rel_path.clone(), diff.original.clone(), diff.agent_version.clone())
        };
        if let Some(s) = self.review_changes.as_mut() {
            if let Some(diff) = s.diffs.get_mut(fi) {
                if let Some(v) = diff.hunk_verdicts.get_mut(hunk_idx) {
                    *v = Verdict::Rejected;
                }
                let content = apply_hunk_verdicts(&original, &agent_version, &diff.hunk_verdicts);
                let abs = project_root.join(&rel_path);
                if let Some(parent) = abs.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if content.is_empty() && original.is_empty() {
                    // All hunks rejected for a created file — delete it
                    let _ = std::fs::remove_file(&abs);
                } else {
                    let _ = std::fs::write(&abs, &content);
                }
            }
        }
        self.agent_panel.pending_reloads.push(rel_path);
        Ok(())
    }

    // ── Insights dashboard mode key handling (ADR 0129 Phase 3) ──────────────

    pub(super) fn handle_insights_dashboard_mode(&mut self, key: KeyEvent) -> Result<()> {
        use crossterm::event::KeyModifiers;

        match key.code {
            // Close — return to Normal
            KeyCode::Char('q') | KeyCode::Esc => {
                self.insights_dashboard = None;
                self.mode = crate::keymap::Mode::Normal;
            },
            // Tab forward / backward
            KeyCode::Tab => {
                if let Some(d) = self.insights_dashboard.as_mut() {
                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                        d.prev_tab();
                    } else {
                        d.next_tab();
                    }
                }
            },
            KeyCode::BackTab => {
                if let Some(d) = self.insights_dashboard.as_mut() {
                    d.prev_tab();
                }
            },
            // Scroll down: j / Down / Ctrl-d
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(d) = self.insights_dashboard.as_mut() {
                    d.scroll_down();
                }
            },
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(d) = self.insights_dashboard.as_mut() {
                    for _ in 0..10 {
                        d.scroll_down();
                    }
                }
            },
            // Scroll up: k / Up / Ctrl-u
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(d) = self.insights_dashboard.as_mut() {
                    d.scroll_up();
                }
            },
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(d) = self.insights_dashboard.as_mut() {
                    for _ in 0..10 {
                        d.scroll_up();
                    }
                }
            },
            // Number keys 1-5: jump directly to tab
            KeyCode::Char(c @ '1'..='5') => {
                if let Some(d) = self.insights_dashboard.as_mut() {
                    use crate::insights::panel::InsightsTab;
                    d.active_tab = match c {
                        '1' => InsightsTab::Summary,
                        '2' => InsightsTab::Activity,
                        '3' => InsightsTab::Models,
                        '4' => InsightsTab::Efficiency,
                        '5' => InsightsTab::Errors,
                        _ => unreachable!(),
                    };
                    d.scroll = 0;
                }
            },
            _ => {},
        }
        Ok(())
    }
}
