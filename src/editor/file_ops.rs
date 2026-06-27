use anyhow::Result;
use std::path::PathBuf;

use super::Editor;

impl Editor {
    #[inline]
    pub(super) fn is_picker_sentinel(path: &std::path::Path) -> bool {
        path.as_os_str().is_empty() || path.to_str() == Some("\x01")
    }

    pub(super) fn recents_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
        PathBuf::from(home).join(".local/share/forgiven/recent_files.txt")
    }

    pub(super) fn load_recents() -> Vec<PathBuf> {
        let Ok(content) = std::fs::read_to_string(Self::recents_path()) else {
            return vec![];
        };
        content
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .take(5)
            .collect()
    }

    pub(super) fn save_recents(&self) -> Result<()> {
        let path = Self::recents_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = self
            .recent_files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, text)?;
        Ok(())
    }

    pub(super) fn scan_files(&mut self) {
        self.file_all.clear();
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.scan_directory(&current_dir, 0);
        self.file_all.sort();
    }

    /// Recursively scan a directory for files
    pub(super) fn scan_directory(&mut self, dir: &PathBuf, depth: usize) {
        // Limit recursion depth to avoid scanning too deep
        if depth > 5 {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Skip hidden files, common build dirs, and IDE folders
            if file_name.starts_with('.')
                || file_name == "target"
                || file_name == "node_modules"
                || file_name == "dist"
                || file_name == "build"
            {
                continue;
            }

            if path.is_file() {
                // Skip binary and lock files
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_str().unwrap_or("");
                    if ext_str == "lock" || ext_str == "exe" || ext_str == "dll" || ext_str == "so"
                    {
                        continue;
                    }
                }
                self.file_all.push(path);
            } else if path.is_dir() {
                self.scan_directory(&path, depth + 1);
            }
        }
    }

    /// Open lazygit in the current terminal, suspending the TUI.
    pub(super) fn open_lazygit(&mut self) -> anyhow::Result<()> {
        // Suspend the TUI, run lazygit, then restore.
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableBracketedPaste,
        )?;
        self.terminal.show_cursor()?;

        let status = std::process::Command::new("lazygit").status();

        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableBracketedPaste,
        )?;
        self.terminal.clear()?;

        match status {
            Ok(s) if s.success() => self.set_status("lazygit closed".to_string()),
            Ok(_) => self.set_status("lazygit exited with non-zero status".to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.set_status(
                    "lazygit not found — install it and ensure it is on $PATH".to_string(),
                );
            },
            Err(e) => self.set_status(format!("Failed to launch lazygit: {e}")),
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

        match std::process::Command::new(opener).arg(&path).spawn() {
            Ok(_) => self.set_status(format!("Opened in browser: {}", path.display())),
            Err(e) => self.set_status(format!("Failed to open browser: {e}")),
        }
    }

    /// Read a file for use as agent context.
    ///
    /// Returns `(display_name, content, line_count)` where `display_name` is the
    /// cwd-relative path, `content` is the (possibly truncated) file text, and
    /// `line_count` is the number of lines in the returned content.
    /// Files exceeding the line limit are truncated and a notice is appended.
    #[allow(dead_code)]
    pub(super) fn read_file_for_context(
        path: &std::path::Path,
        project_root: &std::path::Path,
    ) -> std::io::Result<(String, String, usize)> {
        const MAX_LINES: usize = 2000;

        let display_name =
            path.strip_prefix(project_root).unwrap_or(path).to_string_lossy().into_owned();

        let raw = std::fs::read_to_string(path)?;
        let all_lines: Vec<&str> = raw.lines().collect();
        let total = all_lines.len();

        let (content, line_count) = if total > MAX_LINES {
            let truncated = all_lines[..MAX_LINES].join("\n");
            let warned = format!("{truncated}\n\n[Truncated: showing {MAX_LINES}/{total} lines]");
            (warned, MAX_LINES)
        } else {
            (raw, total)
        };

        Ok((display_name, content, line_count))
    }
}
