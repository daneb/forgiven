use std::path::PathBuf;

use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};

use super::protocol::NexusEvent;

/// Handle to the Nexus UDS server running in a background task.
///
/// Dropping this value signals the background task to exit and removes the
/// socket file from the filesystem.
pub struct SidecarServer {
    event_tx: mpsc::UnboundedSender<NexusEvent>,
    /// `take()`-ed on drop to trigger clean shutdown.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Fires once each time a new sidecar client connects.
    /// The editor polls this to push an immediate state snapshot on connect.
    pub new_client_rx: mpsc::UnboundedReceiver<()>,
}

impl SidecarServer {
    /// Canonical path for the UDS socket, scoped to the current PID so that
    /// multiple editor instances do not collide.
    pub fn socket_path() -> PathBuf {
        let pid = std::process::id();
        PathBuf::from(format!("/tmp/forgiven-nexus-{pid}.sock"))
    }

    /// Bind the UDS listener and spawn the background accept-loop task.
    ///
    /// Any stale socket file from a prior crash is removed before binding.
    pub async fn bind(socket_path: &std::path::Path) -> anyhow::Result<Self> {
        // Remove a stale socket left by a prior crash.
        let _ = std::fs::remove_file(socket_path);

        let listener = UnixListener::bind(socket_path)?;
        let (event_tx, event_rx) = mpsc::unbounded_channel::<NexusEvent>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (new_client_tx, new_client_rx) = mpsc::unbounded_channel::<()>();

        tokio::spawn(accept_loop(listener, event_rx, shutdown_rx, new_client_tx));

        Ok(Self { event_tx, shutdown_tx: Some(shutdown_tx), new_client_rx })
    }

    /// Send an event to the sidecar. Fire-and-forget: drops silently when no
    /// client is connected or the background task has exited.
    pub fn send(&self, event: NexusEvent) {
        let _ = self.event_tx.send(event);
    }
}

impl Drop for SidecarServer {
    fn drop(&mut self) {
        // Signal the background task to exit.
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Remove the socket file so reconnection attempts fail fast.
        let _ = std::fs::remove_file(Self::socket_path());
    }
}

/// Background task: accept one client at a time, forward events as
/// newline-delimited JSON. On client disconnect, resume listening.
async fn accept_loop(
    listener: UnixListener,
    mut event_rx: mpsc::UnboundedReceiver<NexusEvent>,
    mut shutdown_rx: oneshot::Receiver<()>,
    new_client_tx: mpsc::UnboundedSender<()>,
) {
    let mut current_client: Option<tokio::net::UnixStream> = None;

    loop {
        tokio::select! {
            // Shutdown signal from SidecarServer::drop().
            _ = &mut shutdown_rx => break,

            // New client connection (only when no client is active).
            accept = listener.accept(), if current_client.is_none() => {
                match accept {
                    Ok((stream, _addr)) => {
                        tracing::debug!("Nexus: sidecar client connected");
                        current_client = Some(stream);
                        // Signal the editor to push an immediate state snapshot.
                        let _ = new_client_tx.send(());
                    }
                    Err(e) => {
                        tracing::warn!("Nexus: accept error: {e}");
                    }
                }
            }

            // Event from the editor — forward to the connected client.
            event = event_rx.recv() => {
                let Some(evt) = event else {
                    // Sender dropped: editor is shutting down.
                    break;
                };
                if let Some(ref mut stream) = current_client {
                    match serde_json::to_string(&evt) {
                        Ok(mut line) => {
                            line.push('\n');
                            if stream.write_all(line.as_bytes()).await.is_err() {
                                tracing::debug!("Nexus: client disconnected");
                                current_client = None;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Nexus: serialisation error: {e}");
                        }
                    }
                }
            }
        }
    }

    tracing::debug!("Nexus: accept-loop exiting");
}
