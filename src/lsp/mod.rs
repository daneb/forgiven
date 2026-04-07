use anyhow::{Context, Result};
use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::*;
use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};
use url::Url;

/// Messages sent from LSP client to editor
#[derive(Debug, Clone)]
pub enum LspNotificationMsg {
    Diagnostics {
        uri: Uri,
        diagnostics: Vec<Diagnostic>,
    },
    Initialized,
    #[allow(dead_code)]
    Error {
        message: String,
    },
    /// Human-readable message from the server (e.g. Copilot auth instructions).
    ShowMessage {
        message: String,
    },
}

/// Stored diagnostic information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiagnosticInfo {
    pub diagnostic: Diagnostic,
    pub uri: Uri,
}

/// LSP client state for a single language server.
///
/// I/O is handled by two background threads:
///  - writer thread: receives Messages from `writer_tx` and writes them to child stdin
///  - reader thread: reads Messages from child stdout and sends them via `reader_rx`
///
/// This avoids using `Connection::stdio()` from lsp_server (which connects to the
/// *current* process's stdin/stdout instead of the child's, stealing keyboard input).
pub struct LspClient {
    /// The language server process
    process: Option<Child>,
    /// PID of the child, kept separately so we can kill the whole process
    /// group (child + any grandchildren like rust-analyzer-proc-macro-srv)
    /// even after `process` has been taken in shutdown().
    child_pid: Option<u32>,

    /// Send messages to the LSP server (via background writer thread)
    writer_tx: std_mpsc::Sender<Message>,

    /// Receive messages from the LSP server (from background reader thread)
    reader_rx: mpsc::UnboundedReceiver<Message>,

    /// Request ID counter
    next_request_id: i32,

    /// Pending requests waiting for responses
    pending_requests: HashMap<RequestId, oneshot::Sender<serde_json::Value>>,

    /// Server capabilities after initialization
    capabilities: Option<ServerCapabilities>,

    /// Root URI of the workspace
    workspace_root: Uri,

    /// Channel to send notifications to editor
    notification_tx: mpsc::UnboundedSender<LspNotificationMsg>,
}

impl LspClient {
    /// Spawn a new language server and set up background I/O threads.
    pub fn spawn(
        command: &str,
        args: &[&str],
        workspace_root: PathBuf,
        notification_tx: mpsc::UnboundedSender<LspNotificationMsg>,
        extra_env: &std::collections::HashMap<String, String>,
    ) -> Result<Self> {
        info!("Spawning LSP server: {} {:?}", command, args);

        // Ensure common tool directories are on PATH so language servers installed
        // via rustup, npm, pip etc. can be found even when launched from a minimal env.
        //
        // ORDERING MATTERS: rustup/cargo dirs must come BEFORE Homebrew so that
        // `rustc`, `rust-analyzer` etc. resolve to the rustup-managed binaries.
        // We always strip-and-prepend these preferred dirs even if they're already
        // present in PATH, to guarantee correct precedence regardless of how the
        // user's shell is configured.
        let path_env = std::env::var("PATH").unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        // Preferred dirs — prepended unconditionally (rustup must beat Homebrew).
        let preferred_dirs: Vec<String> =
            vec![format!("{}/.cargo/bin", home), format!("{}/.local/bin", home)];
        // Fallback dirs — only added if not already present.
        let mut extra_dirs: Vec<String> = vec![
            "/usr/local/bin".to_string(),
            // Homebrew on Apple Silicon and Intel — intentionally after rustup
            "/opt/homebrew/bin".to_string(),
            "/opt/homebrew/opt/node/bin".to_string(),
            "/usr/local/opt/node/bin".to_string(),
        ];
        // Resolve the active nvm node version from ~/.nvm/alias/default
        // e.g. "~/.nvm/alias/default" contains "20.11.0" -> add ~/.nvm/versions/node/v20.11.0/bin
        let nvm_default = format!("{}/.nvm/alias/default", home);
        if let Ok(version) = std::fs::read_to_string(&nvm_default) {
            let version = version.trim().trim_start_matches('v');
            if !version.is_empty() {
                extra_dirs.push(format!("{}/.nvm/versions/node/v{}/bin", home, version));
            }
        }
        // Also do a best-effort glob of ~/.nvm/versions/node/*/bin so any installed version works
        if let Ok(entries) = std::fs::read_dir(format!("{}/.nvm/versions/node", home)) {
            for entry in entries.flatten() {
                let bin = entry.path().join("bin");
                if bin.is_dir() {
                    if let Some(s) = bin.to_str() {
                        extra_dirs.push(s.to_string());
                    }
                }
            }
        }

        // Build the final PATH:
        //   1. preferred_dirs  (always prepended — rustup beats Homebrew)
        //   2. extra fallback dirs not yet in PATH
        //   3. original PATH
        // Strip preferred dirs from the original PATH first so they don't appear twice.
        let stripped_path: String = path_env
            .split(':')
            .filter(|seg| !preferred_dirs.iter().any(|p| p == seg))
            .collect::<Vec<_>>()
            .join(":");
        let mut path_parts: Vec<String> = preferred_dirs;
        for d in &extra_dirs {
            if !d.is_empty() && !stripped_path.contains(d.as_str()) {
                path_parts.push(d.clone());
            }
        }
        path_parts.push(stripped_path);
        let augmented_path = path_parts.join(":");

        // Resolve user-supplied env vars (strip leading `$` and look up from host env).
        let mut cmd = Command::new(command);
        cmd.args(args)
            .env("PATH", &augmented_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()); // capture so we can log it (null would block the child)
        for (key, val) in extra_env {
            let resolved = if let Some(var_name) = val.strip_prefix('$') {
                std::env::var(var_name).unwrap_or_else(|_| {
                    warn!("LSP env var ${} not set, leaving empty", var_name);
                    String::new()
                })
            } else {
                val.clone()
            };
            cmd.env(key, resolved);
        }
        // Put the child in its own process group so that shutdown() can kill
        // the entire group (including grandchildren like rust-analyzer-proc-macro-srv)
        // with a single signal, preventing process leaks across editor restarts.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut process = cmd.spawn().with_context(|| {
            format!("Failed to spawn LSP server '{}' (PATH={})", command, augmented_path)
        })?;
        let child_pid = process.id();

        let child_stdin =
            process.stdin.take().ok_or_else(|| anyhow::anyhow!("Failed to get child stdin"))?;
        let child_stdout =
            process.stdout.take().ok_or_else(|| anyhow::anyhow!("Failed to get child stdout"))?;
        let child_stderr =
            process.stderr.take().ok_or_else(|| anyhow::anyhow!("Failed to get child stderr"))?;

        // Stderr logger thread — drain the child's stderr so it never blocks,
        // and forward each line to tracing so it shows up in forgiven.log.
        let server_name = command.to_string();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = BufReader::new(child_stderr);
            for line in reader.lines().map_while(Result::ok) {
                warn!("[lsp stderr] {}: {}", server_name, line);
            }
        });

        // Writer thread: pull Messages from the channel and write them to the child's stdin.
        let (writer_tx, writer_rx) = std_mpsc::channel::<Message>();
        std::thread::spawn(move || {
            let mut writer = BufWriter::new(child_stdin);
            while let Ok(msg) = writer_rx.recv() {
                if let Err(e) = msg.write(&mut writer) {
                    error!("LSP write error: {}", e);
                    break;
                }
                if let Err(e) = writer.flush() {
                    error!("LSP flush error: {}", e);
                    break;
                }
            }
            debug!("LSP writer thread exiting");
        });

        // Reader thread: read Messages from the child's stdout and forward them.
        let (reader_tx, reader_rx) = mpsc::unbounded_channel::<Message>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(child_stdout);
            loop {
                match Message::read(&mut reader) {
                    Ok(Some(msg)) => {
                        if reader_tx.send(msg).is_err() {
                            break; // main thread dropped the receiver
                        }
                    },
                    Ok(None) => {
                        info!("LSP server closed connection (EOF)");
                        break;
                    },
                    Err(e) => {
                        error!("LSP read error: {}", e);
                        break;
                    },
                }
            }
            debug!("LSP reader thread exiting");
        });

        // Canonicalize to resolve symlinks / relative components — Url::from_file_path
        // requires a true absolute path (e.g. /private/... on macOS, not /tmp/...).
        let canonical_root = match workspace_root.canonicalize() {
            Ok(p) => {
                info!("Workspace root (canonical): {:?}", p);
                p
            },
            Err(e) => {
                warn!("canonicalize({:?}) failed: {} — using raw path", workspace_root, e);
                workspace_root.clone()
            },
        };
        let workspace_url = Url::from_file_path(&canonical_root).map_err(|_| {
            anyhow::anyhow!(
                "Url::from_file_path failed for {:?} (is_absolute={})",
                canonical_root,
                canonical_root.is_absolute()
            )
        })?;
        let workspace_uri = Uri::from_str(workspace_url.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to create URI: {}", e))?;

        Ok(Self {
            process: Some(process),
            child_pid: Some(child_pid),
            writer_tx,
            reader_rx,
            next_request_id: 1,
            pending_requests: HashMap::new(),
            capabilities: None,
            workspace_root: workspace_uri,
            notification_tx,
        })
    }

    /// Initialize the LSP server (async, with a 10-second timeout).
    ///
    /// Pass `initialization_options` for servers that require custom options
    /// (e.g. Copilot needs `editorInfo` and `editorPluginInfo`).
    pub async fn initialize(
        &mut self,
        initialization_options: Option<serde_json::Value>,
    ) -> Result<()> {
        #[allow(deprecated)] // root_uri kept for servers that predate workspace_folders
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(self.workspace_root.clone()),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: self.workspace_root.clone(),
                name: "project".to_string(),
            }]),
            initialization_options,
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(TextDocumentSyncClientCapabilities {
                        did_save: Some(true),
                        will_save: Some(true),
                        will_save_wait_until: Some(false),
                        dynamic_registration: Some(false),
                    }),
                    completion: Some(CompletionClientCapabilities {
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    hover: Some(HoverClientCapabilities {
                        content_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    definition: Some(GotoCapability {
                        link_support: Some(false),
                        ..Default::default()
                    }),
                    references: Some(ReferenceClientCapabilities { ..Default::default() }),
                    rename: Some(RenameClientCapabilities {
                        prepare_support: Some(false),
                        ..Default::default()
                    }),
                    document_symbol: Some(DocumentSymbolClientCapabilities {
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                workspace: Some(WorkspaceClientCapabilities {
                    apply_edit: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let request_id = self.send_request_id::<request::Initialize>(params)?;

        // Await the response with a timeout so a slow/crashed server can't hang the editor.
        let response =
            tokio::time::timeout(Duration::from_secs(10), self.wait_for_response(request_id))
                .await
                .context("LSP initialization timed out after 10 seconds")?
                .context("LSP initialization failed")?;

        if let Some(result) = response.result {
            let init_result: InitializeResult =
                serde_json::from_value(result).context("Failed to parse InitializeResult")?;
            self.capabilities = Some(init_result.capabilities);
            info!("LSP server initialized successfully");
        } else if let Some(err) = response.error {
            return Err(anyhow::anyhow!("LSP initialization error: {:?}", err));
        }

        self.notify::<notification::Initialized>(InitializedParams {})?;
        let _ = self.notification_tx.send(LspNotificationMsg::Initialized);

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Internal send helpers
    // -------------------------------------------------------------------------

    /// Send a request, returning its ID (used when we want to await the response directly).
    fn send_request_id<R>(&mut self, params: R::Params) -> Result<RequestId>
    where
        R: lsp_types::request::Request,
    {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request_id = RequestId::from(id);

        let request =
            Request::new(request_id.clone(), R::METHOD.to_string(), serde_json::to_value(params)?);
        self.writer_tx
            .send(Message::Request(request))
            .map_err(|_| anyhow::anyhow!("LSP writer channel closed"))?;

        debug!("Sent LSP request: {} (id={})", R::METHOD, id);
        Ok(request_id)
    }

    /// Send a request and return a oneshot receiver for the async response.
    #[allow(dead_code)]
    fn send_request<R>(&mut self, params: R::Params) -> Result<oneshot::Receiver<serde_json::Value>>
    where
        R: lsp_types::request::Request,
    {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request_id = RequestId::from(id);

        let request =
            Request::new(request_id.clone(), R::METHOD.to_string(), serde_json::to_value(params)?);

        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(request_id, tx);

        self.writer_tx
            .send(Message::Request(request))
            .map_err(|_| anyhow::anyhow!("LSP writer channel closed"))?;

        debug!("Sent LSP request: {} (id={})", R::METHOD, id);
        Ok(rx)
    }

    /// Send a notification to the LSP server.
    fn notify<N>(&mut self, params: N::Params) -> Result<()>
    where
        N: lsp_types::notification::Notification,
    {
        let notification = Notification::new(N::METHOD.to_string(), serde_json::to_value(params)?);
        self.writer_tx
            .send(Message::Notification(notification))
            .map_err(|_| anyhow::anyhow!("LSP writer channel closed"))?;
        debug!("Sent LSP notification: {}", N::METHOD);
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Async response waiting (used only during initialize)
    // -------------------------------------------------------------------------

    /// Drain the reader channel until the response for `target_id` arrives.
    /// Other messages are processed/routed while waiting.
    async fn wait_for_response(&mut self, target_id: RequestId) -> Result<Response> {
        loop {
            let msg = self
                .reader_rx
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("LSP server disconnected (channel closed)"))?;

            match msg {
                Message::Response(resp) if resp.id == target_id => return Ok(resp),
                Message::Response(resp) => {
                    // Route any other responses that arrived while we were waiting.
                    if let Some(tx) = self.pending_requests.remove(&resp.id) {
                        if let Some(result) = resp.result {
                            let _ = tx.send(result);
                        } else if let Some(err) = resp.error {
                            error!("LSP error for pending request {:?}: {:?}", resp.id, err);
                            let _ = tx.send(serde_json::Value::Null);
                        }
                    }
                },
                Message::Notification(notif) => self.handle_notification(notif),
                Message::Request(req) => {
                    debug!("Server-initiated request during init: {}", req.method);
                    let response = Response::new_ok(req.id, serde_json::Value::Null);
                    let _ = self.writer_tx.send(Message::Response(response));
                },
            }
        }
    }

    // -------------------------------------------------------------------------
    // Non-blocking message processing (called every frame in the editor loop)
    // -------------------------------------------------------------------------

    /// Drain up to `MAX_MESSAGES_PER_FRAME` messages from the reader channel without blocking.
    /// Capped to keep frame time predictable even when a server sends bursts of notifications.
    /// Returns the number of messages processed.
    pub fn process_messages(&mut self) -> Result<usize> {
        const MAX_MESSAGES_PER_FRAME: usize = 32;
        let mut count = 0;
        while count < MAX_MESSAGES_PER_FRAME {
            match self.reader_rx.try_recv() {
                Ok(Message::Response(resp)) => {
                    if let Some(tx) = self.pending_requests.remove(&resp.id) {
                        if let Some(result) = resp.result {
                            let _ = tx.send(result);
                        } else if let Some(err) = resp.error {
                            error!("LSP error response: {:?}", err);
                            // Send Null so the receiver resolves immediately rather
                            // than hanging until tx is dropped.
                            let _ = tx.send(serde_json::Value::Null);
                        }
                    }
                },
                Ok(Message::Notification(notif)) => self.handle_notification(notif),
                Ok(Message::Request(req)) => {
                    // Server-initiated requests (e.g. window/workDoneProgress/create,
                    // workspace/configuration) require a response or the server stalls.
                    // Send a null success response for all of them.
                    debug!("Server-initiated request: {}", req.method);
                    let response = Response::new_ok(req.id, serde_json::Value::Null);
                    let _ = self.writer_tx.send(Message::Response(response));
                },
                Err(_) => break, // channel empty
            }
            count += 1;
        }
        Ok(count)
    }

    // -------------------------------------------------------------------------
    // Notification handling
    // -------------------------------------------------------------------------

    fn handle_notification(&mut self, notif: lsp_server::Notification) {
        match notif.method.as_str() {
            "textDocument/publishDiagnostics" => {
                if let Ok(params) =
                    serde_json::from_value::<PublishDiagnosticsParams>(notif.params)
                {
                    info!(
                        "Diagnostics for {:?}: {} items",
                        params.uri,
                        params.diagnostics.len()
                    );
                    let _ = self.notification_tx.send(LspNotificationMsg::Diagnostics {
                        uri: params.uri,
                        diagnostics: params.diagnostics,
                    });
                }
            }
            // Auth / info messages — surface to the user via status line.
            "window/showMessage" | "window/showMessageRequest" => {
                if let Some(msg) = notif.params
                    .get("message")
                    .and_then(|v| v.as_str())
                {
                    info!("LSP window/showMessage: {}", msg);
                    let _ = self.notification_tx.send(LspNotificationMsg::ShowMessage {
                        message: msg.to_string(),
                    });
                }
            }
            // High-frequency server chatter — log at trace so normal debug runs stay clean.
            "$/progress"
            | "window/logMessage"
            | "window/workDoneProgress/create"
            | "telemetry/event"
            | "$/copilot/openURL"       // Copilot browser-open requests
            | "$/copilot/didChangeStatus" => {
                debug!("LSP [{}]", notif.method);
            }
            _ => {
                debug!("Unhandled LSP notification: {}", notif.method);
            }
        }
    }

    // -------------------------------------------------------------------------
    // Public document notification methods
    // -------------------------------------------------------------------------

    pub fn did_open(&mut self, uri: Uri, language_id: String, text: String) -> Result<()> {
        self.notify::<notification::DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem { uri, language_id, version: 0, text },
        })
    }

    pub fn did_change(&mut self, uri: Uri, version: i32, text: String) -> Result<()> {
        self.notify::<notification::DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text,
            }],
        })
    }

    pub fn did_save(&mut self, uri: Uri, text: Option<String>) -> Result<()> {
        self.notify::<notification::DidSaveTextDocument>(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
            text,
        })
    }

    // -------------------------------------------------------------------------
    // Public request methods (return oneshot receivers)
    // TODO: wire these up from editor/mod.rs request_hover / request_goto_definition etc.
    // -------------------------------------------------------------------------

    #[allow(dead_code)]
    pub fn hover(
        &mut self,
        uri: Uri,
        position: Position,
    ) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.send_request::<request::HoverRequest>(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
    }

    pub fn goto_definition(
        &mut self,
        uri: Uri,
        position: Position,
    ) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.send_request::<request::GotoDefinition>(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
    }

    #[allow(dead_code)]
    pub fn completion(
        &mut self,
        uri: Uri,
        position: Position,
    ) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.send_request::<request::Completion>(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
    }

    pub fn references(
        &mut self,
        uri: Uri,
        position: Position,
    ) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.send_request::<request::References>(ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext { include_declaration: true },
        })
    }

    #[allow(dead_code)]
    pub fn rename(
        &mut self,
        uri: Uri,
        position: Position,
        new_name: String,
    ) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.send_request::<request::Rename>(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            new_name,
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
    }

    pub fn document_symbols(&mut self, uri: Uri) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.send_request::<request::DocumentSymbolRequest>(DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
    }

    // -------------------------------------------------------------------------
    // Copilot custom auth methods (non-standard JSON-RPC)
    // -------------------------------------------------------------------------

    /// Send a raw JSON-RPC request with an arbitrary method name.
    /// Used for Copilot's custom `checkStatus` / `signInInitiate` protocol.
    fn copilot_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<oneshot::Receiver<serde_json::Value>> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request_id = RequestId::from(id);

        let request = Request::new(request_id.clone(), method.to_string(), params);

        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(request_id, tx);

        self.writer_tx
            .send(Message::Request(request))
            .map_err(|_| anyhow::anyhow!("LSP writer channel closed"))?;

        debug!("Sent copilot request '{}' (id={})", method, id);
        Ok(rx)
    }

    /// Ask the Copilot server whether the user is signed in.
    /// Response JSON: `{"status": "OK", "user": "…"}` or `{"status": "NotSignedIn"}`
    pub fn copilot_check_status(&mut self) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.copilot_request("checkStatus", serde_json::json!({ "options": {} }))
    }

    /// Start the GitHub device auth flow.
    /// Response JSON: `{"status": "PromptUserDeviceFlow", "verificationUri": "…", "userCode": "XXXX-XXXX"}`
    /// or `{"status": "AlreadySignedIn", "user": "…"}` if already authenticated.
    pub fn copilot_sign_in_initiate(&mut self) -> Result<oneshot::Receiver<serde_json::Value>> {
        self.copilot_request("signInInitiate", serde_json::json!({ "options": {} }))
    }

    // -------------------------------------------------------------------------
    // Inline completion (LSP 3.18 / Copilot)
    // -------------------------------------------------------------------------

    /// Request inline completions at the given position.
    ///
    /// Uses raw JSON to avoid dependency on lsp-types 3.18 types.
    /// The response is a raw `serde_json::Value` — use
    /// `parse_first_inline_completion()` to extract the first suggestion.
    pub fn inline_completion(
        &mut self,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> Result<oneshot::Receiver<serde_json::Value>> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri.to_string() },
            "position": { "line": line, "character": character },
            "context": { "triggerKind": 2 }   // Automatic = 2
        });

        let id = self.next_request_id;
        self.next_request_id += 1;
        let request_id = RequestId::from(id);

        let request =
            Request::new(request_id.clone(), "textDocument/inlineCompletion".to_string(), params);

        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(request_id, tx);

        self.writer_tx
            .send(Message::Request(request))
            .map_err(|_| anyhow::anyhow!("LSP writer channel closed"))?;

        debug!("Sent inlineCompletion request (id={})", id);
        Ok(rx)
    }

    // -------------------------------------------------------------------------
    // Shutdown
    // -------------------------------------------------------------------------

    /// Shut down the LSP server. Non-blocking — just kills the process.
    pub fn shutdown(&mut self) {
        // Best-effort: send exit notification before killing.
        let notif = Notification::new(
            "exit".to_string(), // lsp_types::notification::Exit::METHOD
            serde_json::Value::Null,
        );
        let _ = self.writer_tx.send(Message::Notification(notif));

        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            // Do NOT wait() here — we might be called from Drop in an async context.
            // The OS will reap the process eventually.
        }

        // Kill the entire process group (child + grandchildren like proc-macro-srv).
        // We do this AFTER killing the direct child so the group kill is a belt-and-
        // suspenders backstop rather than the primary signal.
        // process_group(0) on spawn ensured PGID == child PID, so `-pid` targets
        // the whole group without needing libc or unsafe code.
        #[cfg(unix)]
        if let Some(pid) = self.child_pid.take() {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &format!("-{pid}")])
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // writer_tx and reader_rx are dropped automatically after this,
        // which signals the background threads to exit.
        self.shutdown();
    }
}

// =============================================================================
// LspManager
// =============================================================================

/// Manager for multiple LSP clients (one per language).
pub struct LspManager {
    clients: HashMap<String, LspClient>,
    /// Diagnostics keyed by file URI (updated from LSP notifications).
    diagnostics: HashMap<Uri, Vec<Diagnostic>>,
    notification_rx: mpsc::UnboundedReceiver<LspNotificationMsg>,
    notification_tx: mpsc::UnboundedSender<LspNotificationMsg>,
    /// Human-readable messages (e.g. Copilot auth instructions) for the editor to display.
    pending_messages: Vec<String>,
}

impl LspManager {
    pub fn new() -> Self {
        let (notification_tx, notification_rx) = mpsc::unbounded_channel();
        Self {
            clients: HashMap::new(),
            diagnostics: HashMap::new(),
            notification_rx,
            notification_tx,
            pending_messages: Vec::new(),
        }
    }

    /// Clone the notification sender so callers can spin up clients independently.
    pub fn notification_tx(&self) -> mpsc::UnboundedSender<LspNotificationMsg> {
        self.notification_tx.clone()
    }

    /// Insert an already-initialized client (used after parallel startup).
    pub fn insert_client(&mut self, language: String, client: LspClient) {
        self.clients.insert(language, client);
    }

    /// Drain any human-readable messages collected since the last call.
    /// The editor should call this each frame and display the last one.
    pub fn drain_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_messages)
    }

    /// Get a mutable reference to the client for a language.
    pub fn get_client(&mut self, language: &str) -> Option<&mut LspClient> {
        self.clients.get_mut(language)
    }

    /// Process pending LSP notifications and client messages.
    /// Call this every frame — it is non-blocking and capped per frame.
    /// Process pending LSP notifications and client wire messages.
    /// Returns `true` if anything was processed (so callers can decide whether to re-render).
    pub fn process_messages(&mut self) -> Result<bool> {
        const MAX_NOTIFS_PER_FRAME: usize = 32;
        let mut count = 0;

        // Drain high-level notifications (diagnostics etc.) forwarded from client threads.
        while count < MAX_NOTIFS_PER_FRAME {
            match self.notification_rx.try_recv() {
                Ok(LspNotificationMsg::Diagnostics { uri, diagnostics }) => {
                    self.diagnostics.insert(uri, diagnostics);
                },
                Ok(LspNotificationMsg::Initialized) => {
                    info!("LSP client initialized");
                },
                Ok(LspNotificationMsg::Error { message }) => {
                    error!("LSP error: {}", message);
                },
                Ok(LspNotificationMsg::ShowMessage { message }) => {
                    info!("LSP message: {}", message);
                    self.pending_messages.push(message);
                },
                Err(_) => break,
            }
            count += 1;
        }

        // Drain raw wire messages from each client (routes responses to pending requests).
        let mut client_count = 0usize;
        for client in self.clients.values_mut() {
            client_count += client.process_messages().unwrap_or(0);
        }

        Ok(count > 0 || client_count > 0)
    }

    /// Get current diagnostics for a file (returns an empty vec if none).
    pub fn get_diagnostics(&self, uri: &Uri) -> Vec<Diagnostic> {
        self.diagnostics.get(uri).cloned().unwrap_or_default()
    }

    /// Get all diagnostics across all files.
    #[allow(dead_code, clippy::mutable_key_type)]
    pub fn get_all_diagnostics(&self) -> &HashMap<Uri, Vec<Diagnostic>> {
        &self.diagnostics
    }

    /// Remove diagnostics for a file that has been closed, preventing unbounded
    /// accumulation across long sessions with many file opens/closes.
    pub fn clear_diagnostics_for_uri(&mut self, uri: &Uri) {
        self.diagnostics.remove(uri);
    }

    // -------------------------------------------------------------------------
    // Utility helpers
    // -------------------------------------------------------------------------

    pub fn path_to_uri(path: &std::path::Path) -> Result<Uri> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let url = Url::from_file_path(&canonical)
            .map_err(|_| anyhow::anyhow!("Invalid file path: {:?}", canonical))?;
        Uri::from_str(url.as_str()).map_err(|e| anyhow::anyhow!("Failed to create URI: {}", e))
    }

    pub fn language_from_path(path: &std::path::Path) -> String {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| match ext {
                "rs" => "rust",
                "py" => "python",
                "js" => "javascript",
                "ts" => "typescript",
                "go" => "go",
                "c" => "c",
                "cpp" | "cc" | "cxx" => "cpp",

                "cs" => "csharp",
                "java" => "java",
                "rb" => "ruby",
                "sh" => "sh",
                "md" => "markdown",
                "json" => "json",
                "yaml" | "yml" => "yaml",
                "toml" => "toml",
                _ => "plaintext",
            })
            .unwrap_or("plaintext")
            .to_string()
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Inline completion helpers
// =============================================================================

// =============================================================================
// Parallel startup helper
// =============================================================================

/// Return only the LSP server configs that are relevant for `workspace_root`.
///
/// For well-known languages we require a language-specific indicator file to
/// exist in the workspace root before attempting to spawn the server.  This
/// prevents slow timeout-based failures (e.g. `rust-analyzer` on a TypeScript
/// project) from blocking editor startup.
///
/// * `copilot` is always included — it is a cross-language completion engine.
/// * Languages not in the known list are always included (opt-out rather than
///   opt-in, so a user-configured server is never silently dropped).
pub fn filter_servers_for_workspace(
    servers: &[crate::config::LspServerConfig],
    workspace_root: &std::path::Path,
) -> Vec<crate::config::LspServerConfig> {
    servers.iter().filter(|s| server_relevant_for_workspace(s, workspace_root)).cloned().collect()
}

fn server_relevant_for_workspace(
    server: &crate::config::LspServerConfig,
    workspace_root: &std::path::Path,
) -> bool {
    // Copilot is a cross-language completion engine — always start it.
    if server.language == "copilot" {
        return true;
    }

    // For well-known languages, require an indicator file in the workspace root.
    let indicators: &[&str] = match server.language.as_str() {
        "rust" => &["Cargo.toml"],
        "python" => &["pyproject.toml", "setup.py", "requirements.txt"],
        "typescript" | "javascript" => &["tsconfig.json", "package.json"],
        "go" => &["go.mod"],
        "c" | "cpp" | "c++" => &["CMakeLists.txt", "compile_commands.json"],
        "java" => &["pom.xml", "build.gradle", "build.gradle.kts"],
        "ruby" => &["Gemfile"],
        "php" => &["composer.json"],
        // C# uses *.csproj — handled separately below.
        // C#: accept .sln or .csproj at the workspace root, or .csproj one
        // level deep (solution-style repos where projects live in src/).
        "csharp" => {
            let has_cs_indicator = |dir: &std::path::Path| {
                dir.read_dir()
                    .ok()
                    .map(|entries| {
                        entries.filter_map(|e| e.ok()).any(|e| {
                            matches!(
                                e.path().extension().and_then(|x| x.to_str()),
                                Some("csproj" | "sln")
                            )
                        })
                    })
                    .unwrap_or(false)
            };
            // Check root first.
            if has_cs_indicator(workspace_root) {
                return true;
            }
            // Then check immediate subdirectories (src/, etc.).
            return workspace_root
                .read_dir()
                .ok()
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .any(|e| has_cs_indicator(&e.path()))
                })
                .unwrap_or(false);
        },
        // Unknown language — don't block it.
        _ => return true,
    };

    let found = indicators.iter().any(|name| workspace_root.join(name).exists());
    if !found {
        info!("LSP '{}': no indicator file found in workspace, skipping", server.language);
    }
    found
}

/// Spawn and initialize all LSP servers concurrently.
///
/// Returns one `(language, Result<LspClient>)` per configured server in the
/// same order as `servers`.  Callers apply the results to `LspManager` via
/// `insert_client` and handle per-server errors individually.
pub async fn init_servers_parallel(
    servers: &[crate::config::LspServerConfig],
    workspace_root: std::path::PathBuf,
    notification_tx: mpsc::UnboundedSender<LspNotificationMsg>,
) -> Vec<(String, Result<LspClient>)> {
    use tokio::task::JoinSet;

    let mut join_set: JoinSet<(usize, String, Result<LspClient>)> = JoinSet::new();

    for (idx, server) in servers.iter().enumerate() {
        let language = server.language.clone();
        let command = server.command.clone();
        let args: Vec<String> = server.args.clone();
        let env = server.env.clone();
        let user_init_options = server.initialization_options.clone();
        let root = workspace_root.clone();
        let tx = notification_tx.clone();

        join_set.spawn(async move {
            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let result = async {
                let mut client = LspClient::spawn(&command, &args_ref, root, tx, &env)?;

                // Build built-in defaults for servers that need special initialization.
                let builtin: Option<serde_json::Value> = if language == "copilot" {
                    Some(serde_json::json!({
                        "editorInfo":       { "name": "forgiven", "version": "0.1.0" },
                        "editorPluginInfo": { "name": "forgiven-copilot", "version": "0.1.0" }
                    }))
                } else {
                    None
                };

                // Merge user-supplied initialization_options over the built-in defaults.
                // User values take precedence at the top level; nested merging is not
                // performed (a user key fully replaces the corresponding built-in key).
                let init_options = match (builtin, user_init_options) {
                    (Some(mut base), Some(overrides)) => {
                        if let Ok(overrides_json) = serde_json::to_value(&overrides) {
                            if let (Some(base_obj), Some(override_obj)) =
                                (base.as_object_mut(), overrides_json.as_object())
                            {
                                for (k, v) in override_obj {
                                    base_obj.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        Some(base)
                    },
                    (Some(base), None) => Some(base),
                    (None, Some(overrides)) => serde_json::to_value(&overrides).ok(),
                    (None, None) => None,
                };

                client.initialize(init_options).await?;
                Ok::<LspClient, anyhow::Error>(client)
            }
            .await;
            (idx, language, result)
        });
    }

    // Collect into index-ordered slots to restore config ordering.
    let mut slots: Vec<Option<(String, Result<LspClient>)>> =
        (0..servers.len()).map(|_| None).collect();
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok((idx, lang, result)) => slots[idx] = Some((lang, result)),
            Err(e) => warn!("LSP init task panicked: {e}"),
        }
    }

    slots.into_iter().flatten().collect()
}

// =============================================================================

/// Extract the first suggestion text from a raw `textDocument/inlineCompletion` response.
///
/// The spec allows either `InlineCompletionList { items: [...] }` or a bare array `[...]`.
/// Each item has an `insertText` field (string or `{ value: string }`).
pub fn parse_first_inline_completion(value: serde_json::Value) -> Option<String> {
    let items = value.get("items").and_then(|v| v.as_array()).or_else(|| value.as_array())?;

    let item = items.first()?;

    // insertText may be a plain string or { value: "..." }
    item.get("insertText").and_then(|v| {
        if let Some(s) = v.as_str() {
            Some(s.to_string())
        } else {
            v.get("value").and_then(|s| s.as_str()).map(|s| s.to_string())
        }
    })
}
