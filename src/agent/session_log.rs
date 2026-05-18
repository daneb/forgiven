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
// Session-start record (Phase 2)
// ─────────────────────────────────────────────────────────────────────────────

/// Write a `"session_start"` record to `sessions.jsonl` on the first submit
/// of a new conversation.  Provides the start timestamp, model, provider, and
/// project root that `session_end` lacks, enabling session-duration and
/// efficiency-ratio queries in Phase 3.
pub fn append_session_start_record(model: &str, provider: &str, project_root: &str) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    append_session_metric(&serde_json::json!({
        "type": "session_start",
        "ts": ts,
        "model": model,
        "provider": provider,
        "project_root": project_root,
    }));
}

// ─────────────────────────────────────────────────────────────────────────────
// Round tool-call record (Phase 2)
// ─────────────────────────────────────────────────────────────────────────────

/// Write a `"round_tools"` record to `history/<ts>.jsonl` capturing the tool
/// calls made during one agent round.  Called from the `Done` event handler
/// when `pending_tool_calls` is non-empty.
pub fn append_round_tools(session_start_secs: u64, tools: &[(String, bool)]) {
    let Some(path) = history_file_path(session_start_secs) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let entry = serde_json::json!({
        "type": "round_tools",
        "ts": ts,
        "tools": tools.iter()
            .map(|(name, success)| serde_json::json!({"name": name, "success": success}))
            .collect::<Vec<_>>(),
    });
    use std::io::Write as _;
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{entry}"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Session-end efficiency record (Phase 4.1)
// ─────────────────────────────────────────────────────────────────────────────

/// Write a `"session_end"` record to `sessions.jsonl` capturing the efficiency
/// signal for this conversation boundary.
///
/// `ended_by` is either `"new_conversation"` or `"janitor"`.
/// Only call when `session_rounds > 0` — skip empty sessions.
pub fn append_session_end_record(
    model: &str,
    session_prompt_total: u32,
    session_completion_total: u32,
    session_rounds: u32,
    files_changed: usize,
    ended_by: &str,
) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    append_session_metric(&serde_json::json!({
        "type": "session_end",
        "ts": ts,
        "model": model,
        "session_prompt_total": session_prompt_total,
        "session_completion_total": session_completion_total,
        "session_rounds": session_rounds,
        "files_changed": files_changed,
        "ended_by": ended_by,
    }));
}

// ─────────────────────────────────────────────────────────────────────────────
// Adaptive round-limit hint (Phase 4.2)
// ─────────────────────────────────────────────────────────────────────────────

/// Read the last 200 `"session_end"` records from `sessions.jsonl` for `model`
/// and return the median `session_rounds + 2` as a suggested round ceiling.
///
/// Returns `None` when:
/// - the file doesn't exist or can't be read,
/// - fewer than 3 matching records are found (not enough history),
/// - or the median would equal the default (no change needed).
pub fn suggest_max_rounds(model: &str) -> Option<usize> {
    let path = metrics_data_path()?;
    let content = std::fs::read_to_string(&path).ok()?;

    let mut rounds: Vec<u64> = content
        .lines()
        .rev()
        .take(200)
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            if v.get("type")?.as_str()? != "session_end" {
                return None;
            }
            if v.get("model")?.as_str()? != model {
                return None;
            }
            v.get("session_rounds")?.as_u64()
        })
        .collect();

    if rounds.len() < 3 {
        return None;
    }

    rounds.sort_unstable();
    let median = rounds[rounds.len() / 2] as usize;
    Some(median + 2)
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

// ─────────────────────────────────────────────────────────────────────────────
// P2-S7: Project init — create .forgiven/ + starter constitution
// ─────────────────────────────────────────────────────────────────────────────

/// Default constitution written on first run when `.forgiven/` is absent.
pub const CONSTITUTION_TEMPLATE: &str = "\
# Project Constitution

## Role
You are a helpful coding assistant for this project.

## Code style
- Follow existing conventions and patterns in the codebase.
- Prefer small, focused changes over large rewrites.
- Run tests after edits when a test suite is present.

## Constraints
- Never modify files outside the project root.
- Ask before irreversible or destructive operations.
- Prefer `edit_file` over `write_file` for targeted changes.
";

/// Create `.forgiven/` and a starter `constitution.md` if neither exists.
/// Returns `true` when the directory (and file) were freshly created.
pub fn project_init(root: &Path) -> bool {
    let dir = root.join(".forgiven");
    if dir.is_dir() {
        return false;
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let _ = std::fs::write(dir.join("constitution.md"), CONSTITUTION_TEMPLATE);
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// P2-S8/S9: Project-local session persistence
// ─────────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SerializedMessage {
    pub role: String,
    pub content: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SavedSession {
    pub session_start_secs: u64,
    pub session_rounds: u32,
    pub messages: Vec<SerializedMessage>,
}

/// Serialize the active conversation to `.forgiven/sessions/<session_start_secs>.json`.
/// Skips sessions that haven't completed at least one round.
pub fn save_session(
    root: &Path,
    session_start_secs: u64,
    messages: &[super::ChatMessage],
    rounds: u32,
) {
    if session_start_secs == 0 || rounds == 0 {
        return;
    }
    let dir = root.join(".forgiven").join("sessions");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let serialized: Vec<SerializedMessage> = messages
        .iter()
        .filter(|m| matches!(m.role, super::Role::User | super::Role::Assistant))
        .map(|m| SerializedMessage {
            role: m.role.as_str().to_string(),
            content: m.content.clone(),
        })
        .collect();
    let session = SavedSession { session_start_secs, session_rounds: rounds, messages: serialized };
    if let Ok(json) = serde_json::to_string_pretty(&session) {
        let _ = std::fs::write(dir.join(format!("{session_start_secs}.json")), json);
    }
}

/// Load the most-recently saved session from `.forgiven/sessions/`.
/// Returns `None` if no session files exist or the directory is absent.
pub fn load_most_recent_session(root: &Path) -> Option<SavedSession> {
    let dir = root.join(".forgiven").join("sessions");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    // Filenames are `<unix_ts>.json`; descending sort gives newest first.
    entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    let path = entries.first()?.path();
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// P2-S10: Plan block extraction
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the content of the first `## Plan` section from `text`.
/// Stops at the next `##`-level heading or end of string.
/// Returns `None` when no plan section is found.
pub fn extract_plan_block(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    // Accept "## plan" with optional trailing punctuation / label.
    let start = lower.find("## plan")?;
    let section = &text[start..];
    // Find where the next ##-level heading begins (after the first newline).
    let next_heading = section
        .char_indices()
        .skip(1) // skip the '##' we just found
        .find(|&(i, _)| section[i..].starts_with("\n##") || section[i..].starts_with("\r\n##"))
        .map(|(i, _)| i + 1); // +1 to include the newline as boundary
    let end = next_heading.unwrap_or(section.len());
    Some(section[..end].trim().to_string())
}

/// Write a plan block to `.forgiven/plan.md`, creating the directory if needed.
pub fn save_plan(root: &Path, plan_text: &str) {
    let dir = root.join(".forgiven");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("plan.md"), plan_text);
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

// ─────────────────────────────────────────────────────────────────────────────
// P2-S11: Harness tests — init, session resume, plan block
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod harness_tests {
    use super::*;
    use std::fs;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "forgiven_harness_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let _ = fs::create_dir_all(&p);
        p
    }

    // ── P2-S7: project_init ──────────────────────────────────────────────────

    #[test]
    fn init_creates_forgiven_dir_and_constitution() {
        let root = tmp_dir("init");
        assert!(project_init(&root), "should return true on first init");
        assert!(root.join(".forgiven").is_dir(), ".forgiven/ must exist");
        assert!(root.join(".forgiven/constitution.md").exists(), "constitution.md must be created");
        let content = fs::read_to_string(root.join(".forgiven/constitution.md")).unwrap();
        assert!(content.contains("# Project Constitution"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn init_is_idempotent() {
        let root = tmp_dir("init_idem");
        assert!(project_init(&root));
        // Second call must return false (dir already exists).
        assert!(!project_init(&root), "second init must return false");
        let _ = fs::remove_dir_all(&root);
    }

    // ── P2-S8/S9: save_session / load_most_recent_session ───────────────────

    #[test]
    fn save_and_load_session_roundtrip() {
        let root = tmp_dir("session");
        let msgs = vec![
            super::super::ChatMessage {
                role: super::super::Role::User,
                content: "Hello agent".to_string(),
                images: vec![],
            },
            super::super::ChatMessage {
                role: super::super::Role::Assistant,
                content: "Hi there!".to_string(),
                images: vec![],
            },
        ];
        save_session(&root, 9999, &msgs, 1);

        let loaded = load_most_recent_session(&root).expect("session must be loadable");
        assert_eq!(loaded.session_start_secs, 9999);
        assert_eq!(loaded.session_rounds, 1);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].role, "user");
        assert_eq!(loaded.messages[0].content, "Hello agent");
        assert_eq!(loaded.messages[1].role, "assistant");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_session_skips_empty_or_zero_start() {
        let root = tmp_dir("session_skip");
        let msgs: Vec<super::super::ChatMessage> = vec![];
        // session_start_secs == 0 → no file written
        save_session(&root, 0, &msgs, 1);
        // rounds == 0 → no file written
        save_session(&root, 1234, &msgs, 0);
        assert!(load_most_recent_session(&root).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_most_recent_returns_newest() {
        let root = tmp_dir("session_newest");
        let msgs = vec![super::super::ChatMessage {
            role: super::super::Role::User,
            content: "old".to_string(),
            images: vec![],
        }];
        save_session(&root, 1000, &msgs, 1);
        let msgs2 = vec![super::super::ChatMessage {
            role: super::super::Role::User,
            content: "new".to_string(),
            images: vec![],
        }];
        save_session(&root, 2000, &msgs2, 2);

        let loaded = load_most_recent_session(&root).unwrap();
        assert_eq!(loaded.session_start_secs, 2000, "must return the newest session");
        let _ = fs::remove_dir_all(&root);
    }

    // ── P2-S10: extract_plan_block / save_plan ───────────────────────────────

    #[test]
    fn extract_plan_block_basic() {
        let text =
            "Some preamble.\n\n## Plan\n- Step 1\n- Step 2\n\n## Notes\nfollow-up text here.";
        let plan = extract_plan_block(text).expect("should find plan");
        assert!(plan.contains("## Plan"));
        assert!(plan.contains("Step 1"));
        assert!(plan.contains("Step 2"));
        // Content under the subsequent ## heading must not bleed into the plan block.
        assert!(!plan.contains("follow-up"), "plan={plan:?}");
    }

    #[test]
    fn extract_plan_block_stops_at_next_heading() {
        let text = "## Plan\n- Do X\n\n## Implementation\ncode here";
        let plan = extract_plan_block(text).unwrap();
        assert!(plan.contains("Do X"));
        assert!(!plan.contains("Implementation"));
    }

    #[test]
    fn extract_plan_block_returns_none_when_absent() {
        assert!(extract_plan_block("No plan section here.").is_none());
    }

    #[test]
    fn save_plan_creates_file() {
        let root = tmp_dir("plan");
        save_plan(&root, "## Plan\n- Step 1");
        let path = root.join(".forgiven/plan.md");
        assert!(path.exists(), "plan.md must be created");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Step 1"));
        let _ = fs::remove_dir_all(&root);
    }
}
