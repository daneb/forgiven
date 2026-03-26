use anyhow::Result;
use crossterm::{
    event::{KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tokio::sync::oneshot;

use super::Editor;
use crate::keymap::Mode;
use crate::lsp::LspManager;

// =============================================================================
// Free functions
// =============================================================================

/// Fix unquoted node labels containing parentheses in Mermaid source.
///
/// Mermaid breaks when a square-bracket label contains `(` or `)` without
/// surrounding quotes.  AI models frequently generate this form, e.g.:
///   `K[UseHttpMetrics (Prometheus)]`
///
/// The fix is to wrap the label in double-quotes:
///   `K["UseHttpMetrics (Prometheus)"]`
///
/// This function scans each line character-by-character and applies the
/// transformation only to `[…]` groups that (a) contain `(` or `)` and
/// (b) are not already quoted.
pub(super) fn fix_mermaid_parens(source: &str) -> String {
    let mut result = String::with_capacity(source.len() + 64);
    for line in source.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '[' {
                // Find the matching `]`, tracking depth.
                let start = i;
                let mut j = i + 1;
                let mut depth = 1usize;
                while j < chars.len() && depth > 0 {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {},
                    }
                    j += 1;
                }
                if depth == 0 {
                    // chars[start+1 .. j-1] is the inner label content.
                    let inner: String = chars[start + 1..j - 1].iter().collect();
                    if (inner.contains('(') || inner.contains(')')) && !inner.starts_with('"') {
                        result.push('[');
                        result.push('"');
                        result.push_str(&inner);
                        result.push('"');
                        result.push(']');
                    } else {
                        result.extend(chars[start..j].iter());
                    }
                    i = j;
                    continue;
                }
            }
            result.push(chars[i]);
            i += 1;
        }
        result.push('\n');
    }
    // Preserve original trailing-newline behaviour.
    if !source.ends_with('\n') {
        result.pop();
    }
    result
}

/// Strip a wrapping code fence that the AI may add despite being told not to.
/// Handles ` ```markdown `, ` ```md `, and plain ` ``` ` opening fences.
pub(super) fn strip_markdown_fence(s: &str) -> String {
    let trimmed = s.trim();
    let after_open = trimmed
        .strip_prefix("```markdown")
        .or_else(|| trimmed.strip_prefix("```md"))
        .or_else(|| trimmed.strip_prefix("```"));
    if let Some(rest) = after_open {
        // Strip the newline immediately after the opening fence, then the closing fence.
        let body = rest.strip_prefix('\n').unwrap_or(rest);
        if let Some(stripped) = body.strip_suffix("```") {
            return stripped.trim_end().to_string();
        }
    }
    trimmed.to_string()
}

// =============================================================================
// impl Editor — AI / git integration methods
// =============================================================================

impl Editor {
    /// Send a `textDocument/inlineCompletion` request to any available LSP client.
    /// Tries the file's language client first, then falls back to "copilot".
    pub(super) fn request_inline_completion(&mut self) {
        let buf = match self.current_buffer() {
            Some(b) => b,
            None => return,
        };
        let path = match buf.file_path.clone() {
            Some(p) => p,
            None => return,
        };
        let row = buf.cursor.row as u32;
        let col = buf.cursor.col as u32;

        let uri = match LspManager::path_to_uri(&path) {
            Ok(u) => u,
            Err(_) => return,
        };

        // Always prefer the "copilot" client for inline completions — language servers
        // like rust-analyzer don't support textDocument/inlineCompletion.
        // Fall back to the file-language client only if no Copilot client is registered.
        // Two separate lookups avoid a double-mutable-borrow on lsp_manager.
        let language = LspManager::language_from_path(&path);
        let has_copilot = self.lsp_manager.get_client("copilot").is_some();
        let client = if has_copilot {
            self.lsp_manager.get_client("copilot")
        } else {
            self.lsp_manager.get_client(&language)
        };

        if let Some(client) = client {
            match client.inline_completion(&uri, row, col) {
                Ok(rx) => self.pending_completion = Some(rx),
                Err(e) => tracing::debug!("inline_completion request failed: {}", e),
            }
        }
    }

    /// Suspend the TUI, open lazygit, then restore the TUI.
    ///
    /// The suspend/resume pattern:
    ///   1. Leave alternate screen + disable raw mode  → terminal is back to normal
    ///   2. Spawn `lazygit` as a child process with inherited stdio  → lazygit takes over
    ///   3. Wait for lazygit to exit
    ///   4. Re-enter alternate screen + re-enable raw mode  → our TUI resumes
    ///   5. Force a full redraw so no lazygit artifacts remain
    ///   6. Reload every open buffer from disk (git ops may have changed files)
    pub(super) fn open_lazygit(&mut self) -> Result<()> {
        // ── Suspend TUI ──────────────────────────────────────────────────────
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;

        // ── Run lazygit ───────────────────────────────────────────────────────
        // lazygit is itself a full-screen TUI; it inherits our stdin/stdout/stderr.
        let result = std::process::Command::new("lazygit").status();

        // ── Restore TUI (always, even if lazygit errored) ─────────────────────
        enable_raw_mode()?;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        // Force ratatui to repaint every cell — clears any lazygit residue.
        self.terminal.clear()?;

        // ── Handle outcome ────────────────────────────────────────────────────
        match result {
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.set_status(
                    "lazygit not found — install it (e.g. brew install lazygit)".to_string(),
                );
            },
            Err(e) => {
                self.set_status(format!("lazygit error: {e}"));
            },
            Ok(_) => {
                // Reload every open buffer — a git pull/checkout/rebase may have
                // changed files on disk that are currently open in the editor.
                for buf in &mut self.buffers {
                    if buf.file_path.is_some() {
                        let _ = buf.reload_from_disk();
                    }
                }
                self.set_status("Returned from lazygit".to_string());
            },
        }

        Ok(())
    }

    // ── Commit-message generation ──────────────────────────────────────────────

    /// Kick off a background AI task to generate a commit message.
    /// `from_staged = true`  → use `git diff --staged`
    /// `from_staged = false` → use `git show HEAD --stat -p`
    pub(super) fn start_commit_msg(&mut self, from_staged: bool) {
        // Run the git command synchronously (it's fast).
        let diff_cmd = if from_staged {
            std::process::Command::new("git").args(["diff", "--staged"]).output()
        } else {
            std::process::Command::new("git").args(["show", "HEAD", "--stat", "-p"]).output()
        };

        let diff_text = match diff_cmd {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
            Err(e) => {
                self.set_status(format!("git error: {e}"));
                return;
            },
        };

        if diff_text.trim().is_empty() {
            let msg = if from_staged {
                "No staged changes — stage files first (git add)".to_string()
            } else {
                "No commits found".to_string()
            };
            self.set_status(msg);
            return;
        }

        let model_id = self
            .agent_panel
            .selected_model_id_with_fallback(&self.config.default_copilot_model)
            .to_string();

        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let system = "You are a concise git commit message writer. \
                Given a git diff, output ONLY a single commit message: \
                a short subject line (≤72 chars, imperative mood), then a blank line, \
                then an optional bullet-point body with the key changes. \
                No preamble, no explanation, just the commit message.";
            let user = format!("Write a commit message for this diff:\n\n```\n{diff_text}\n```");
            let result = async {
                let api_token = crate::agent::acquire_copilot_token().await?;
                crate::agent::one_shot_complete(&api_token, &model_id, system, &user, 256).await
            }
            .await;
            let _ = tx.send(result);
        });

        self.commit_msg.rx = Some(rx);
        self.commit_msg.from_staged = from_staged;
        self.commit_msg.buffer = String::new();
        self.mode = Mode::CommitMsg;
        self.set_status("Generating commit message…".to_string());
    }

    /// Handle key events while in Mode::CommitMsg.
    pub(super) fn handle_commit_msg_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // Esc — discard
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.commit_msg.buffer.clear();
                self.commit_msg.rx = None;
                self.set_status("Commit message discarded".to_string());
            },
            // Enter — commit with the current message
            KeyCode::Enter => {
                let msg = self.commit_msg.buffer.trim().to_string();
                if msg.is_empty() {
                    self.set_status("Commit message is empty".to_string());
                    return Ok(());
                }
                let out = std::process::Command::new("git").args(["commit", "-m", &msg]).output();
                self.mode = Mode::Normal;
                self.commit_msg.buffer.clear();
                self.commit_msg.rx = None;
                match out {
                    Ok(o) if o.status.success() => {
                        self.set_status("Committed successfully".to_string());
                    },
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr).to_string();
                        self.set_status(format!("git commit failed: {err}"));
                    },
                    Err(e) => self.set_status(format!("git error: {e}")),
                }
            },
            // Backspace — delete last character
            KeyCode::Backspace => {
                self.commit_msg.buffer.pop();
            },
            // Regular characters
            KeyCode::Char(ch) => {
                self.commit_msg.buffer.push(ch);
            },
            _ => {},
        }
        Ok(())
    }

    /// Enter `Mode::ReleaseNotes` — count-input phase.
    pub(super) fn start_release_notes(&mut self) {
        self.release_notes.buffer.clear();
        self.release_notes.count_input = String::from("10");
        self.release_notes.scroll = 0;
        self.mode = Mode::ReleaseNotes;
        self.set_status(
            "Enter number of commits (default 10) then press Enter to generate release notes"
                .to_string(),
        );
    }

    /// Run git log, spawn AI task, transition to generating phase.
    pub(super) fn trigger_release_notes_generation(&mut self) {
        let count: usize =
            self.release_notes.count_input.trim().parse::<usize>().unwrap_or(10).clamp(1, 200);

        let log_output = std::process::Command::new("git")
            .args(["log", "--format=%H%n%s%n%b%n---", &format!("-{count}")])
            .output();

        let log_text = match log_output {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
            Err(e) => {
                self.mode = Mode::Normal;
                self.set_status(format!("git error: {e}"));
                return;
            },
        };

        if log_text.trim().is_empty() {
            self.set_status("No commits found".to_string());
            return;
        }

        let model_id = self
            .agent_panel
            .selected_model_id_with_fallback(&self.config.default_copilot_model)
            .to_string();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let system = "You are a technical writer creating release notes for a software project. \
                Given a list of git commits, produce clean, user-friendly release notes in markdown. \
                Group related changes under headings like '### Features', '### Bug Fixes', '### Improvements'. \
                Be concise but descriptive. Omit merge commits and trivial chore changes. \
                Output markdown only — no preamble, no explanation.";
            let user = format!(
                "Generate release notes from these {count} commits:\n\n```\n{log_text}\n```"
            );
            let result = async {
                let api_token = crate::agent::acquire_copilot_token().await?;
                crate::agent::one_shot_complete(&api_token, &model_id, system, &user, 1024).await
            }
            .await;
            let _ = tx.send(result);
        });

        self.release_notes.rx = Some(rx);
        self.set_status(format!("Generating release notes from {count} commits…"));
    }

    /// Handle key events while in `Mode::ReleaseNotes`.
    pub(super) fn handle_release_notes_mode(&mut self, key: KeyEvent) -> Result<()> {
        // Phase 2: generating — only Esc cancels.
        if self.release_notes.rx.is_some() {
            if key.code == KeyCode::Esc {
                self.release_notes.rx = None;
                self.mode = Mode::Normal;
                self.set_status("Release notes cancelled".to_string());
            }
            return Ok(());
        }

        // Phase 3: displaying the result.
        if !self.release_notes.buffer.is_empty() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                    self.release_notes.buffer.clear();
                    self.release_notes.scroll = 0;
                    self.set_status("Release notes closed".to_string());
                },
                KeyCode::Char('y') => {
                    let text = self.release_notes.buffer.clone();
                    self.sync_system_clipboard(&text);
                    self.set_status("Release notes copied to clipboard".to_string());
                },
                KeyCode::Char('j') | KeyCode::Down => {
                    self.release_notes.scroll = self.release_notes.scroll.saturating_add(1);
                },
                KeyCode::Char('k') | KeyCode::Up => {
                    self.release_notes.scroll = self.release_notes.scroll.saturating_sub(1);
                },
                _ => {},
            }
            return Ok(());
        }

        // Phase 1: count-entry input.
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.release_notes.count_input = String::from("10");
                self.set_status("Cancelled".to_string());
            },
            KeyCode::Enter => {
                self.trigger_release_notes_generation();
            },
            KeyCode::Backspace => {
                self.release_notes.count_input.pop();
            },
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                self.release_notes.count_input.push(ch);
            },
            _ => {},
        }
        Ok(())
    }

    /// Render the current buffer as HTML and open it in the system browser.
    ///
    /// Writes a self-contained HTML file to the OS temp directory and spawns
    /// the platform opener (`open` on macOS, `xdg-open` on Linux).  The opener
    /// runs detached — the TUI stays alive and no suspend/restore is needed.
    pub(super) fn open_markdown_in_browser(&mut self) {
        let content = match self.current_buffer() {
            Some(buf) => buf.lines().join("\n"),
            None => {
                self.set_status("No buffer open".to_string());
                return;
            },
        };

        let file_stem = self
            .current_buffer()
            .and_then(|b| b.file_path.as_ref())
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("preview")
            .to_string();

        // ── If the file is already HTML, open it directly ─────────────────────
        let is_html = self
            .current_buffer()
            .and_then(|b| b.file_path.as_ref())
            .and_then(|p| p.extension())
            .map(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
            .unwrap_or(false);

        if is_html {
            let path = std::env::temp_dir().join(format!("forgiven_{file_stem}.html"));
            if let Err(e) = std::fs::write(&path, &content) {
                self.set_status(format!("Failed to write temp file: {e}"));
                return;
            }
            #[cfg(target_os = "macos")]
            let opener = "open";
            #[cfg(target_os = "linux")]
            let opener = "xdg-open";
            #[cfg(target_os = "windows")]
            let opener = "explorer";
            match std::process::Command::new(opener).arg(&path).spawn() {
                Ok(_) => self.set_status(format!("Opened {file_stem}.html in browser")),
                Err(e) => self.set_status(format!("Failed to open browser: {e}")),
            }
            return;
        }

        // ── Render markdown → HTML body ───────────────────────────────────────
        let parser = pulldown_cmark::Parser::new_ext(&content, pulldown_cmark::Options::all());
        let mut body = String::new();
        pulldown_cmark::html::push_html(&mut body, parser);

        // ── Wrap in a minimal, readable HTML page ─────────────────────────────
        // The script converts pulldown-cmark's  <pre><code class="language-mermaid">
        // output into <div class="mermaid"> elements, then loads Mermaid.js to
        // render them.
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  body {{
    max-width: 720px;
    margin: 0 auto;
    padding: 64px 32px 96px;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    font-size: 16px;
    line-height: 1.75;
    color: #1a1a1a;
    background: #fff;
    -webkit-font-smoothing: antialiased;
  }}
  h1, h2, h3, h4, h5, h6 {{
    font-weight: 600;
    line-height: 1.25;
    margin: 2em 0 0.5em;
    color: #111;
  }}
  h1 {{ font-size: 2em; border-bottom: 2px solid #e8e8e8; padding-bottom: 0.3em; margin-top: 0; }}
  h2 {{ font-size: 1.4em; border-bottom: 1px solid #e8e8e8; padding-bottom: 0.25em; }}
  h3 {{ font-size: 1.15em; }}
  p  {{ margin: 0 0 1.25em; }}
  a  {{ color: #0969da; text-decoration: none; }}
  a:hover {{ text-decoration: underline; }}
  strong {{ font-weight: 600; }}
  em {{ font-style: italic; }}
  hr {{ border: none; border-top: 1px solid #e8e8e8; margin: 2.5em 0; }}
  ul, ol {{ padding-left: 1.5em; margin: 0 0 1.25em; }}
  li {{ margin: 0.3em 0; }}
  li + li {{ margin-top: 0.25em; }}
  blockquote {{
    border-left: 3px solid #d0d0d0;
    margin: 1.5em 0;
    padding: 0.25em 0 0.25em 1.25em;
    color: #555;
  }}
  blockquote p {{ margin-bottom: 0; }}
  code {{
    font-family: "SFMono-Regular", "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.875em;
    background: #f3f3f3;
    padding: 0.15em 0.35em;
    border-radius: 3px;
    color: #d63384;
  }}
  pre {{
    background: #f6f6f6;
    border: 1px solid #e8e8e8;
    border-radius: 6px;
    padding: 1em 1.25em;
    overflow-x: auto;
    margin: 0 0 1.5em;
    line-height: 1.5;
  }}
  pre code {{
    background: none;
    padding: 0;
    border-radius: 0;
    font-size: 0.85em;
    color: inherit;
  }}
  img {{ max-width: 100%; height: auto; border-radius: 4px; }}
  table {{ border-collapse: collapse; width: 100%; margin: 0 0 1.5em; }}
  th, td {{ border: 1px solid #e0e0e0; padding: 0.5em 0.75em; text-align: left; }}
  th {{ background: #f6f6f6; font-weight: 600; }}
  tr:nth-child(even) {{ background: #fafafa; }}
</style>
</head>
<body>
{body}
<script src="https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js"></script>
<script>
  document.querySelectorAll('pre > code.language-mermaid').forEach(function(code) {{
    var div = document.createElement('div');
    div.className = 'mermaid';
    div.textContent = code.textContent;
    code.parentNode.replaceWith(div);
  }});
  mermaid.initialize({{ startOnLoad: false }});
  mermaid.run();
</script>
</body>
</html>"#,
            title = file_stem,
            body = body,
        );

        // ── Write temp file ───────────────────────────────────────────────────
        let path = std::env::temp_dir().join(format!("forgiven_{file_stem}.html"));
        if let Err(e) = std::fs::write(&path, &html) {
            self.set_status(format!("Failed to write temp file: {e}"));
            return;
        }

        // ── Spawn platform opener (detached) ──────────────────────────────────
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(target_os = "linux")]
        let opener = "xdg-open";
        #[cfg(target_os = "windows")]
        let opener = "explorer";

        match std::process::Command::new(opener).arg(&path).spawn() {
            Ok(_) => self.set_status(format!("Opened in browser: {}", path.display())),
            Err(e) => self.set_status(format!("Failed to open browser: {e}")),
        }
    }

    /// Extract the current (or next) mermaid diagram from the last agent reply,
    /// auto-fix parentheses in node labels, write a self-contained HTML file to
    /// the system temp directory, and open it in the default browser.
    pub(super) fn open_mermaid_in_browser(&mut self) {
        let reply = match self.agent_panel.last_assistant_reply() {
            Some(r) => r,
            None => {
                self.set_status("No agent reply to render".to_string());
                return;
            },
        };

        let blocks = crate::agent::AgentPanel::extract_mermaid_blocks(&reply);
        if blocks.is_empty() {
            self.set_status("No mermaid blocks in last reply".to_string());
            return;
        }

        let idx = self.agent_panel.mermaid_block_idx % blocks.len();
        let source = fix_mermaid_parens(&blocks[idx]);
        self.agent_panel.mermaid_block_idx =
            (self.agent_panel.mermaid_block_idx + 1) % blocks.len();

        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Mermaid diagram {num}/{total}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  body {{
    margin: 0;
    padding: 2rem;
    background: #1e1e2e;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    min-height: 100vh;
  }}
  .mermaid {{ max-width: 100%; }}
</style>
</head>
<body>
<pre class="mermaid">
{source}
</pre>
<script type="module">
  import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs';
  mermaid.initialize({{ startOnLoad: true, theme: 'dark' }});
</script>
</body>
</html>"#,
            num = idx + 1,
            total = blocks.len(),
            source = source,
        );

        let path = std::env::temp_dir().join(format!("forgiven_mermaid_{}.html", idx + 1));
        if let Err(e) = std::fs::write(&path, &html) {
            self.set_status(format!("Failed to write mermaid file: {e}"));
            return;
        }

        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(target_os = "linux")]
        let opener = "xdg-open";
        #[cfg(target_os = "windows")]
        let opener = "explorer";

        match std::process::Command::new(opener).arg(&path).spawn() {
            Ok(_) => self.set_status(format!(
                "Mermaid diagram {}/{} opened in browser  (Ctrl+M for next)",
                idx + 1,
                blocks.len()
            )),
            Err(e) => self.set_status(format!("Failed to open browser: {e}")),
        }
    }
}
