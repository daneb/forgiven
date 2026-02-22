use anyhow::{Context, Result};
use lsp_server::{Connection, Message, Request, Response, Notification, RequestId};
use lsp_types::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, oneshot};
use tracing::{debug, error, info, warn};
use url::Url;

/// Messages sent from editor to LSP client
#[derive(Debug)]
pub enum LspRequest {
    DidOpen { uri: Uri, language_id: String, text: String },
    DidChange { uri: Uri, version: i32, text: String },
    DidSave { uri: Uri, text: Option<String> },
    Hover { uri: Uri, position: Position, response: oneshot::Sender<Option<Hover>> },
    GotoDefinition { uri: Uri, position: Position, response: oneshot::Sender<Option<GotoDefinitionResponse>> },
    Completion { uri: Uri, position: Position, response: oneshot::Sender<Option<CompletionResponse>> },
    References { uri: Uri, position: Position, response: oneshot::Sender<Option<Vec<Location>>> },
    Rename { uri: Uri, position: Position, new_name: String, response: oneshot::Sender<Option<WorkspaceEdit>> },
    DocumentSymbols { uri: Uri, response: oneshot::Sender<Option<DocumentSymbolResponse>> },
    Shutdown,
}

/// Messages sent from LSP client to editor
#[derive(Debug, Clone)]
pub enum LspNotificationMsg {
    Diagnostics { uri: Uri, diagnostics: Vec<Diagnostic> },
    Initialized,
    Error { message: String },
}

/// Stored diagnostic information
#[derive(Debug, Clone)]
pub struct DiagnosticInfo {
    pub diagnostic: Diagnostic,
    pub uri: Uri,
}

/// LSP client state for a single language server
pub struct LspClient {
    /// The language server process
    process: Option<Child>,
    
    /// LSP connection (stdin/stdout)
    connection: Connection,
    
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
    /// Spawn a new language server and initialize connection
    pub fn spawn(
        command: &str,
        args: &[&str],
        workspace_root: PathBuf,
        notification_tx: mpsc::UnboundedSender<LspNotificationMsg>,
    ) -> Result<Self> {
        info!("Spawning LSP server: {} {:?}", command, args);
        
        // Spawn the language server process
        let mut process = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn LSP server")?;
        
        // Create the LSP connection
        let (connection, io_threads) = Connection::stdio();
        
        let workspace_url = Url::from_file_path(&workspace_root)
            .map_err(|_| anyhow::anyhow!("Invalid workspace root path"))?;
        let workspace_uri = Uri::from_str(workspace_url.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to create URI: {}", e))?;
        
        Ok(Self {
            process: Some(process),
            connection,
            next_request_id: 1,
            pending_requests: HashMap::new(),
            capabilities: None,
            workspace_root: workspace_uri,
            notification_tx,
        })
    }
    
    /// Initialize the LSP server
    pub fn initialize(&mut self) -> Result<()> {
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(self.workspace_root.clone()),
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
                    references: Some(ReferenceClientCapabilities {
                        ..Default::default()
                    }),
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
        
        let request_id = self.send_request_sync::<request::Initialize>(params)?;
        
        // Wait for initialize response
        let response = self.receive_response_sync(request_id)?;
        
        if let Some(result) = response.result {
            let init_result: InitializeResult = serde_json::from_value(result)
                .context("Failed to parse initialize result")?;
            self.capabilities = Some(init_result.capabilities);
            info!("LSP server initialized successfully");
        } else {
            return Err(anyhow::anyhow!("LSP initialization failed"));
        }
        
        // Send initialized notification
        self.send_notification::<notification::Initialized>(InitializedParams {})?;
        
        let _ = self.notification_tx.send(LspNotificationMsg::Initialized);
        
        Ok(())
    }
    
    /// Send a request and get a oneshot channel for the response
    fn send_request<R>(&mut self, params: R::Params) -> Result<oneshot::Receiver<serde_json::Value>>
    where
        R: lsp_types::request::Request,
    {
        let id = self.next_request_id;
        self.next_request_id += 1;
        
        let request_id = RequestId::from(id);
        let request = Request::new(
            request_id.clone(),
            R::METHOD.to_string(),
            serde_json::to_value(params)?,
        );
        
        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(request_id, tx);
        self.connection.sender.send(Message::Request(request))?;
        
        debug!("Sent LSP request: {} (id={})", R::METHOD, id);
        Ok(rx)
    }
    
    /// Send a synchronous request (for initialization)
    fn send_request_sync<R>(&mut self, params: R::Params) -> Result<RequestId>
    where
        R: lsp_types::request::Request,
    {
        let id = self.next_request_id;
        self.next_request_id += 1;
        
        let request_id = RequestId::from(id);
        let request = Request::new(
            request_id.clone(),
            R::METHOD.to_string(),
            serde_json::to_value(params)?,
        );
        
        self.connection.sender.send(Message::Request(request))?;
        
        debug!("Sent LSP request: {} (id={})", R::METHOD, id);
        Ok(request_id)
    }
    
    /// Send a notification to the LSP server
    fn send_notification<N>(&mut self, params: N::Params) -> Result<()>
    where
        N: lsp_types::notification::Notification,
    {
        let notification = Notification::new(
            N::METHOD.to_string(),
            serde_json::to_value(params)?,
        );
        
        self.connection.sender.send(Message::Notification(notification))?;
        debug!("Sent LSP notification: {}", N::METHOD);
        Ok(())
    }
    
    /// Receive a response for a specific request ID (synchronous, for initialization only)
    fn receive_response_sync(&mut self, request_id: RequestId) -> Result<Response> {
        loop {
            match self.connection.receiver.recv()? {
                Message::Response(resp) => {
                    if resp.id == request_id {
                        return Ok(resp);
                    }
                }
                Message::Notification(notif) => {
                    self.handle_notification(notif);
                }
                Message::Request(req) => {
                    warn!("Received unexpected request from server: {:?}", req);
                }
            }
        }
    }
    
    /// Process incoming messages from the LSP server
    pub fn process_messages(&mut self) -> Result<bool> {
        // Try to receive a message without blocking
        match self.connection.receiver.try_recv() {
            Ok(Message::Response(resp)) => {
                if let Some(tx) = self.pending_requests.remove(&resp.id) {
                    if let Some(result) = resp.result {
                        let _ = tx.send(result);
                    } else if let Some(err) = resp.error {
                        error!("LSP error response: {:?}", err);
                    }
                }
                Ok(true)
            }
            Ok(Message::Notification(notif)) => {
                self.handle_notification(notif);
                Ok(true)
            }
            Ok(Message::Request(req)) => {
                warn!("Received unexpected request from server: {:?}", req);
                Ok(true)
            }
            Err(_) => Ok(false), // No message available
        }
    }
    
    /// Handle incoming notification from server
    fn handle_notification(&mut self, notif: lsp_server::Notification) {
        debug!("Received notification: {}", notif.method);
        
        match notif.method.as_str() {
            "textDocument/publishDiagnostics" => {
                if let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(notif.params) {
                    info!("Diagnostics for {:?}: {} items", params.uri, params.diagnostics.len());
                    let _ = self.notification_tx.send(LspNotificationMsg::Diagnostics {
                        uri: params.uri,
                        diagnostics: params.diagnostics,
                    });
                }
            }
            _ => {
                debug!("Unhandled notification: {}", notif.method);
            }
        }
    }
    
    /// Notify server that a document was opened
    pub fn did_open(&mut self, uri: Uri, language_id: String, text: String) -> Result<()> {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id,
                version: 0,
                text,
            },
        };
        
        self.send_notification::<notification::DidOpenTextDocument>(params)
    }
    
    /// Notify server that a document was changed
    pub fn did_change(&mut self, uri: Uri, version: i32, text: String) -> Result<()> {
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri,
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text,
            }],
        };
        
        self.send_notification::<notification::DidChangeTextDocument>(params)
    }
    
    /// Notify server that a document was saved
    pub fn did_save(&mut self, uri: Uri, text: Option<String>) -> Result<()> {
        let params = DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
            text,
        };
        
        self.send_notification::<notification::DidSaveTextDocument>(params)
    }
    
    /// Request hover information at a position
    pub fn hover(&mut self, uri: Uri, position: Position) -> Result<oneshot::Receiver<serde_json::Value>> {
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        
        self.send_request::<request::HoverRequest>(params)
    }
    
    /// Request go-to-definition
    pub fn goto_definition(&mut self, uri: Uri, position: Position) -> Result<oneshot::Receiver<serde_json::Value>> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        
        self.send_request::<request::GotoDefinition>(params)
    }
    
    /// Request completion
    pub fn completion(&mut self, uri: Uri, position: Position) -> Result<oneshot::Receiver<serde_json::Value>> {
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        };
        
        self.send_request::<request::Completion>(params)
    }
    
    /// Request references
    pub fn references(&mut self, uri: Uri, position: Position) -> Result<oneshot::Receiver<serde_json::Value>> {
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: true,
            },
        };
        
        self.send_request::<request::References>(params)
    }
    
    /// Request rename
    pub fn rename(&mut self, uri: Uri, position: Position, new_name: String) -> Result<oneshot::Receiver<serde_json::Value>> {
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            new_name,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        
        self.send_request::<request::Rename>(params)
    }
    
    /// Request document symbols
    pub fn document_symbols(&mut self, uri: Uri) -> Result<oneshot::Receiver<serde_json::Value>> {
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        
        self.send_request::<request::DocumentSymbolRequest>(params)
    }
    
    /// Shutdown the LSP server
    pub fn shutdown(&mut self) -> Result<()> {
        let _ = self.send_request_sync::<request::Shutdown>(());
        let _ = self.send_notification::<notification::Exit>(());
        
        if let Some(mut process) = self.process.take() {
            let _ = process.wait();
        }
        
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Manager for multiple LSP clients (one per language)
pub struct LspManager {
    clients: HashMap<String, LspClient>,
    diagnostics: Arc<RwLock<HashMap<Uri, Vec<Diagnostic>>>>,
    notification_rx: mpsc::UnboundedReceiver<LspNotificationMsg>,
    notification_tx: mpsc::UnboundedSender<LspNotificationMsg>,
}

impl LspManager {
    pub fn new() -> Self {
        let (notification_tx, notification_rx) = mpsc::unbounded_channel();
        
        Self {
            clients: HashMap::new(),
            diagnostics: Arc::new(RwLock::new(HashMap::new())),
            notification_rx,
            notification_tx,
        }
    }
    
    /// Add a language server for a specific language
    pub fn add_server(
        &mut self,
        language: String,
        command: &str,
        args: &[&str],
        workspace_root: PathBuf,
    ) -> Result<()> {
        let mut client = LspClient::spawn(command, args, workspace_root, self.notification_tx.clone())?;
        client.initialize()?;
        self.clients.insert(language, client);
        Ok(())
    }
    
    /// Get client for a language
    pub fn get_client(&mut self, language: &str) -> Option<&mut LspClient> {
        self.clients.get_mut(language)
    }
    
    /// Process incoming LSP messages and notifications
    pub async fn process_messages(&mut self) -> Result<()> {
        // Process notifications from all clients
        while let Ok(notif) = self.notification_rx.try_recv() {
            match notif {
                LspNotificationMsg::Diagnostics { uri, diagnostics } => {
                    let mut diags = self.diagnostics.write().await;
                    diags.insert(uri, diagnostics);
                }
                LspNotificationMsg::Initialized => {
                    info!("LSP client initialized");
                }
                LspNotificationMsg::Error { message } => {
                    error!("LSP error: {}", message);
                }
            }
        }
        
        // Process messages from all clients
        for client in self.clients.values_mut() {
            let _ = client.process_messages();
        }
        
        Ok(())
    }
    
    /// Get diagnostics for a specific file
    pub async fn get_diagnostics(&self, uri: &Uri) -> Vec<Diagnostic> {
        let diags = self.diagnostics.read().await;
        diags.get(uri).cloned().unwrap_or_default()
    }
    
    /// Get all diagnostics
    pub async fn get_all_diagnostics(&self) -> HashMap<Uri, Vec<Diagnostic>> {
        let diags = self.diagnostics.read().await;
        diags.clone()
    }
    
    /// Helper method to convert file path to URI
    pub fn path_to_uri(path: &PathBuf) -> Result<Uri> {
        let url = Url::from_file_path(path)
            .map_err(|_| anyhow::anyhow!("Invalid file path"))?;
        Uri::from_str(url.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to create URI: {}", e))
    }
    
    /// Helper method to get language ID from file extension
    pub fn language_from_path(path: &PathBuf) -> String {
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
