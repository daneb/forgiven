use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter as _};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::UnixStream;

// Use Tauri's runtime handle — works from setup() before tokio::main is entered.
use tauri::async_runtime as rt;

/// Mirrors the NexusEvent wire format from src/sidecar/protocol.rs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusContext {
    pub file_path: Option<String>,
    pub cursor_line: Option<u32>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusEvent {
    pub event: String,
    pub content_type: Option<String>,
    pub payload: Option<String>,
    pub context: NexusContext,
}

/// Tauri-side event payloads forwarded to the webview.
#[derive(Clone, Serialize)]
pub struct UpdatePayload {
    pub content: String,
    pub content_type: String,
    pub file_path: Option<String>,
    pub cursor_line: Option<u32>,
}

#[derive(Clone, Serialize)]
pub struct CursorPayload {
    pub file_path: Option<String>,
    pub cursor_line: u32,
}

/// Resolve the Nexus UDS socket path.
///
/// Uses the `NEXUS_SOCKET` environment variable if set (injected by the TUI on
/// launch). Falls back to `/tmp/forgiven-nexus-{pid}.sock` with the PID from
/// `NEXUS_PID`, or tries to find any matching socket as a last resort.
pub fn socket_path() -> Option<PathBuf> {
    // Explicit path from TUI launch env — trim whitespace/newlines defensively.
    if let Ok(p) = std::env::var("NEXUS_SOCKET") {
        let p = p.trim().to_owned();
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    // PID from env — take only the first line in case pgrep returned multiple.
    if let Ok(pid) = std::env::var("NEXUS_PID") {
        let pid = pid.lines().next().unwrap_or("").trim().to_owned();
        if !pid.is_empty() {
            return Some(PathBuf::from(format!("/tmp/forgiven-nexus-{pid}.sock")));
        }
    }
    // Auto-scan /tmp — pick the most recently modified socket when multiple exist.
    if let Ok(rd) = std::fs::read_dir("/tmp") {
        let mut socks: Vec<_> = rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with("forgiven-nexus-") && name.ends_with(".sock")
            })
            .filter_map(|e| {
                let modified = e.metadata().ok()?.modified().ok()?;
                Some((e.path(), modified))
            })
            .collect();
        // Most recently created socket = the live editor session.
        socks.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        if let Some((path, _)) = socks.into_iter().next() {
            return Some(path);
        }
    }
    None
}

/// Spawn the background task that reads from the Nexus UDS and emits Tauri events.
///
/// Reconnects with 500 ms back-off whenever the TUI exits/restarts.
pub fn spawn(app: AppHandle) {
    rt::spawn(async move {
        let mut attempt = 0u32;
        loop {
            let path = match socket_path() {
                Some(p) => p,
                None => {
                    attempt += 1;
                    if attempt == 1 || attempt % 10 == 0 {
                        let _ = app.emit("nexus-status", format!("searching… (attempt {attempt})"));
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                },
            };

            let path_str = path.display().to_string();
            let _ = app.emit("nexus-status", format!("connecting to {path_str}"));
            tracing::debug!("Nexus: connecting to {:?}", path);
            match UnixStream::connect(&path).await {
                Ok(stream) => {
                    tracing::info!("Nexus: connected to {:?}", path);
                    attempt = 0;
                    let _ = app.emit("nexus-status", format!("connected: {path_str}"));
                    if let Err(e) = read_loop(stream, &app).await {
                        tracing::info!("Nexus: connection closed: {e}");
                        let _ = app.emit("nexus-status", format!("disconnected: {e}"));
                    }
                },
                Err(e) => {
                    tracing::debug!("Nexus: connect error: {e}");
                    let _ = app.emit("nexus-status", format!("connect failed: {e} — {path_str}"));
                },
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}

/// Read newline-delimited NexusEvents from the stream and emit Tauri events.
async fn read_loop(stream: UnixStream, app: &AppHandle) -> anyhow::Result<()> {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<NexusEvent>(&line) {
            Ok(evt) => dispatch(evt, app),
            Err(e) => tracing::warn!("Nexus: parse error: {e} — line: {line}"),
        }
    }
    Ok(())
}

fn dispatch(evt: NexusEvent, app: &AppHandle) {
    match evt.event.as_str() {
        "buffer_update" => {
            let payload = UpdatePayload {
                content: evt.payload.unwrap_or_default(),
                content_type: evt.content_type.unwrap_or_else(|| "text".into()),
                file_path: evt.context.file_path,
                cursor_line: evt.context.cursor_line,
            };
            let _ = app.emit("nexus-update", payload);
        },
        "cursor_move" => {
            if let Some(line) = evt.context.cursor_line {
                let payload = CursorPayload { file_path: evt.context.file_path, cursor_line: line };
                let _ = app.emit("nexus-cursor", payload);
            }
        },
        "mode_change" => {
            if let Some(mode) = evt.context.mode {
                let _ = app.emit("nexus-mode", mode);
            }
        },
        "shutdown" => {
            tracing::info!("Nexus: received shutdown — exiting companion");
            std::process::exit(0);
        },
        other => {
            tracing::debug!("Nexus: unknown event type: {other}");
        },
    }
}
