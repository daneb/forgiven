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

    /// Read a file for use as agent context.
    ///
    /// Returns `(display_name, content, line_count)` where `display_name` is the
    /// cwd-relative path, `content` is the (possibly truncated) file text, and
    /// `line_count` is the number of lines in the returned content.
    /// Files exceeding `AT_PICKER_MAX_LINES` are truncated and a notice is appended.
    pub(super) fn read_file_for_context(
        path: &std::path::Path,
        project_root: &std::path::Path,
    ) -> std::io::Result<(String, String, usize)> {
        use crate::agent::AT_PICKER_MAX_LINES;

        let display_name =
            path.strip_prefix(project_root).unwrap_or(path).to_string_lossy().into_owned();

        let raw = std::fs::read_to_string(path)?;
        let all_lines: Vec<&str> = raw.lines().collect();
        let total = all_lines.len();

        let (content, line_count) = if total > AT_PICKER_MAX_LINES {
            let truncated = all_lines[..AT_PICKER_MAX_LINES].join("\n");
            let warned =
                format!("{truncated}\n\n[Truncated: showing {AT_PICKER_MAX_LINES}/{total} lines]");
            (warned, AT_PICKER_MAX_LINES)
        } else {
            (raw, total)
        };

        Ok((display_name, content, line_count))
    }
}
