use std::path::{Path, PathBuf};

use super::AgentPanel;

// ─────────────────────────────────────────────────────────────────────────────
// Session path helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve the path for the persistent session-metrics JSONL file.
/// `~/.local/share/forgiven/sessions.jsonl` (XDG_DATA_HOME-aware).
pub fn metrics_data_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".local/share")
    };
    Some(base.join("forgiven").join("sessions.jsonl"))
}

/// Resolve the path for the conversation history JSONL file.
/// `~/.local/share/forgiven/history/<session_start_secs>.jsonl` (XDG_DATA_HOME-aware).
/// Returns `None` when `session_start_secs` is 0 (not yet set).
pub fn history_file_path(session_start_secs: u64) -> Option<PathBuf> {
    if session_start_secs == 0 {
        return None;
    }
    let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".local/share")
    };
    Some(base.join("forgiven").join("history").join(format!("{session_start_secs}.jsonl")))
}

/// Append one JSON line to the persistent session-metrics file.
/// Creates the directory and file on first use. Silently swallows I/O errors
/// so a permissions problem never interrupts the agentic loop.
pub fn append_session_metric(record: &serde_json::Value) {
    let Some(path) = metrics_data_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut line = record.to_string();
    line.push('\n');
    use std::io::Write as _;
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

// ─────────────────────────────────────────────────────────────────────────────
// Session checkpoint / revert
// ─────────────────────────────────────────────────────────────────────────────

impl AgentPanel {
    /// Returns `true` when the agent has modified or created at least one file
    /// this session and `SPC a u` can revert.
    pub fn has_checkpoint(&self) -> bool {
        !self.session_snapshots.is_empty() || !self.session_created_files.is_empty()
    }

    /// Restore all agent-touched files to their pre-session content and delete
    /// any files the agent created from scratch.
    ///
    /// Returns `(restored, deleted)` counts so the caller can build a status message.
    /// Clears both `session_snapshots` and `session_created_files` on completion.
    /// The caller should push `restored_paths` into `pending_reloads` so open
    /// buffers are refreshed.
    pub fn revert_session(&mut self, project_root: &Path) -> (Vec<String>, Vec<String>) {
        let mut restored = Vec::new();
        for (rel_path, original) in &self.session_snapshots {
            let abs = project_root.join(rel_path);
            if let Some(parent) = abs.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&abs, original) {
                Ok(()) => restored.push(rel_path.clone()),
                Err(e) => {
                    tracing::warn!("[checkpoint] failed to restore {rel_path}: {e}");
                },
            }
        }
        self.session_snapshots.clear();

        let mut deleted = Vec::new();
        for rel_path in &self.session_created_files {
            let abs = project_root.join(rel_path);
            match std::fs::remove_file(&abs) {
                Ok(()) => deleted.push(rel_path.clone()),
                Err(e) => {
                    tracing::warn!("[checkpoint] failed to delete created file {rel_path}: {e}");
                },
            }
        }
        self.session_created_files.clear();

        (restored, deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::{history_file_path, metrics_data_path};
    use std::path::PathBuf;
    use std::sync::Mutex;

    // Serialize all env-var-mutating tests so they don't race each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_metrics_data_path_xdg() {
        let _guard = ENV_LOCK.lock().unwrap();
        let orig = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", "/tmp/test_xdg_forgiven");
        let path = metrics_data_path().unwrap();
        match orig {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        assert_eq!(path, PathBuf::from("/tmp/test_xdg_forgiven/forgiven/sessions.jsonl"));
    }

    #[test]
    fn test_metrics_data_path_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let orig_xdg = std::env::var("XDG_DATA_HOME").ok();
        std::env::remove_var("XDG_DATA_HOME");
        let home = std::env::var("HOME").expect("HOME must be set");
        let path = metrics_data_path().unwrap();
        if let Some(v) = orig_xdg {
            std::env::set_var("XDG_DATA_HOME", v);
        }
        assert_eq!(path, PathBuf::from(&home).join(".local/share/forgiven/sessions.jsonl"));
    }

    #[test]
    fn test_history_file_path_nonzero() {
        // zero → None (no env mutation needed)
        assert!(history_file_path(0).is_none());
        // non-zero → filename is "<ts>.jsonl" under history/
        let _guard = ENV_LOCK.lock().unwrap();
        let orig = std::env::var("XDG_DATA_HOME").ok();
        std::env::set_var("XDG_DATA_HOME", "/tmp/test_xdg_forgiven");
        let path = history_file_path(12345).unwrap();
        match orig {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        assert_eq!(path, PathBuf::from("/tmp/test_xdg_forgiven/forgiven/history/12345.jsonl"));
    }
}
