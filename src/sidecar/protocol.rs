use serde::{Deserialize, Serialize};

/// File and cursor context sent alongside every Nexus event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusContext {
    pub file_path: Option<String>,
    pub cursor_line: Option<u32>,
    pub mode: Option<String>,
}

/// A message broadcast from the TUI to the Tauri sidecar over the UDS.
///
/// Wire format: one JSON object per line (`\n`-terminated), matching the
/// schema in the Hybrid Reliability plan §5.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusEvent {
    /// Discriminant: `"buffer_update"` | `"cursor_move"` | `"mode_change"` | `"shutdown"`
    pub event: String,
    /// MIME-like content type derived from the file extension (`"markdown"`, `"rust"`, …).
    pub content_type: Option<String>,
    /// Buffer text for `buffer_update`; `None` for other event types.
    pub payload: Option<String>,
    pub context: NexusContext,
}

impl NexusEvent {
    pub fn buffer_update(
        content: &str,
        content_type: &str,
        file_path: Option<&str>,
        cursor_line: u32,
    ) -> Self {
        Self {
            event: "buffer_update".into(),
            content_type: Some(content_type.into()),
            payload: Some(content.into()),
            context: NexusContext {
                file_path: file_path.map(Into::into),
                cursor_line: Some(cursor_line),
                mode: None,
            },
        }
    }

    pub fn cursor_move(file_path: Option<&str>, cursor_line: u32) -> Self {
        Self {
            event: "cursor_move".into(),
            content_type: None,
            payload: None,
            context: NexusContext {
                file_path: file_path.map(Into::into),
                cursor_line: Some(cursor_line),
                mode: None,
            },
        }
    }

    pub fn mode_change(mode: &str) -> Self {
        Self {
            event: "mode_change".into(),
            content_type: None,
            payload: None,
            context: NexusContext { file_path: None, cursor_line: None, mode: Some(mode.into()) },
        }
    }

    pub fn shutdown() -> Self {
        Self {
            event: "shutdown".into(),
            content_type: None,
            payload: None,
            context: NexusContext { file_path: None, cursor_line: None, mode: None },
        }
    }
}
