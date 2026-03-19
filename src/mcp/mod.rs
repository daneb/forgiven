//! MCP (Model Context Protocol) client.
//!
//! Supports two transport modes:
//!
//! **stdio** — the editor spawns the server process:
//! ```toml
//! [[mcp.servers]]
//! name    = "filesystem"
//! command = "npx"
//! args    = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
//! ```
//!
//! **HTTP** — connect to an externally-managed server (no process spawned):
//! ```toml
//! [[mcp.servers]]
//! name = "searxng"
//! url  = "http://localhost:8080"
//! ```

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::McpServerConfig;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// A single tool advertised by an MCP server.
pub struct McpTool {
    pub server_name: String,
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's input parameters (forwarded to the LLM as-is).
    pub input_schema: Value,
}

// ─────────────────────────────────────────────────────────────────────────────
// Transport abstraction
// ─────────────────────────────────────────────────────────────────────────────

/// Stdio handle — wraps a spawned child process's stdin/stdout.
struct McpServerHandle {
    name: String,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

/// HTTP+SSE handle for externally-managed MCP servers.
///
/// MCP HTTP+SSE transport flow:
/// 1. Client opens a persistent `GET /sse` connection.
/// 2. Server sends an `event: endpoint` with the POST URL in `data:`.
/// 3. For each JSON-RPC request, client POSTs to that URL.
/// 4. Server sends responses back on the SSE stream as `event: message` events.
///
/// No child process is spawned — the caller owns the server lifecycle.
struct McpSseHandle {
    name: String,
    /// Resolved URL to POST JSON-RPC messages to (extracted from the SSE endpoint event).
    post_url: String,
    client: reqwest::Client,
    next_id: u64,
    /// Receives raw `data:` payloads from the SSE stream (JSON strings).
    event_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    /// Keeps the SSE reader task alive for the lifetime of this handle.
    _sse_task: tokio::task::JoinHandle<()>,
}

impl McpSseHandle {
    /// Open an SSE connection, wait for the endpoint event,
    /// and start the background SSE reader task.
    ///
    /// Tries `{url}/sse` first; falls back to `{url}` if that returns 404
    /// (some servers host the SSE stream at the root path).
    async fn connect(name: &str, base_url: &str) -> Result<Self> {
        let client = reqwest::Client::new();
        let base = base_url.trim_end_matches('/');

        // Probe candidate SSE paths in order.
        let candidates = [format!("{base}/sse"), base.to_string()];
        let mut response = None;
        let mut used_url = String::new();

        for url in &candidates {
            let resp = client
                .get(url)
                .header("Accept", "text/event-stream")
                .send()
                .await
                .with_context(|| format!("connecting to SSE endpoint '{url}'"))?;

            let status = resp.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                continue; // try next candidate
            }
            if !status.is_success() {
                anyhow::bail!("SSE connection to '{url}' failed: HTTP {status}");
            }
            used_url = url.clone();
            response = Some(resp);
            break;
        }

        let response = response.ok_or_else(|| {
            anyhow::anyhow!("no SSE endpoint found at '{base}/sse' or '{base}' (both returned 404)")
        })?;
        let _ = &used_url; // suppress unused warning

        // Channels: endpoint_tx fires once with the POST URL; event_tx forwards all
        // subsequent message payloads to the caller's event_rx.
        let (endpoint_tx, endpoint_rx) = tokio::sync::oneshot::channel::<String>();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let name_for_task = name.to_string();
        let sse_task = tokio::spawn(async move {
            use futures_util::StreamExt as _;
            let mut stream = response.bytes_stream();
            let mut buf = String::new();
            let mut endpoint_tx = Some(endpoint_tx);

            'outer: while let Some(chunk_result) = stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("SSE stream error for '{}': {e}", name_for_task);
                        break;
                    },
                };
                buf.push_str(&String::from_utf8_lossy(&bytes));

                // SSE events are separated by blank lines (\n\n).
                while let Some(end_pos) = buf.find("\n\n") {
                    let event_block = buf[..end_pos].to_string();
                    buf = buf[end_pos + 2..].to_string();

                    let mut event_type = String::from("message");
                    let mut data = String::new();
                    for line in event_block.lines() {
                        if let Some(t) = line.strip_prefix("event: ") {
                            event_type = t.to_string();
                        } else if let Some(d) = line.strip_prefix("data: ") {
                            data = d.to_string();
                        }
                    }

                    if event_type == "endpoint" {
                        if let Some(tx) = endpoint_tx.take() {
                            let _ = tx.send(data);
                        }
                    } else if !data.is_empty() && event_tx.send(data).is_err() {
                        break 'outer; // receiver dropped
                    }
                }
            }
        });

        // Block until we receive the endpoint URL (or the stream closes).
        let endpoint_path =
            endpoint_rx.await.context("SSE connection closed before 'endpoint' event")?;

        // The path may be relative (e.g. "/message?sessionId=…") or absolute.
        let post_url = if endpoint_path.starts_with("http") {
            endpoint_path
        } else {
            format!("{}{}", base_url.trim_end_matches('/'), endpoint_path)
        };

        Ok(McpSseHandle {
            name: name.to_string(),
            post_url,
            client,
            next_id: 1,
            event_rx,
            _sse_task: sse_task,
        })
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        // POST the request (the HTTP response body is typically 202 / empty for SSE transport).
        self.client
            .post(&self.post_url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("HTTP POST to MCP server '{}'", self.name))?;

        // Read SSE events until we see the response with matching id.
        loop {
            let data =
                self.event_rx.recv().await.ok_or_else(|| {
                    anyhow::anyhow!("SSE stream closed while waiting for response")
                })?;

            let val: Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue, // skip non-JSON events
            };

            if val.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = val.get("error") {
                    anyhow::bail!("MCP server '{}' returned error: {err}", self.name);
                }
                return val
                    .get("result")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("MCP response missing 'result' field"));
            }
        }
    }

    async fn notify(&mut self, method: &str) -> Result<()> {
        let body = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        let _ = self.client.post(&self.post_url).json(&body).send().await;
        Ok(())
    }
}

/// Streamable HTTP handle (MCP 2024-11-05 "Streamable HTTP" transport).
///
/// Each request is a self-contained POST.  The server responds with either:
/// - `Content-Type: application/json` — parse the body directly, or
/// - `Content-Type: text/event-stream` — read SSE events until the matching id.
///
/// No persistent SSE connection is required.
struct McpStreamableHandle {
    name: String,
    url: String,
    client: reqwest::Client,
    next_id: u64,
}

impl McpStreamableHandle {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let resp = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("HTTP POST to MCP server '{}'", self.name))?;

        if !resp.status().is_success() {
            anyhow::bail!("MCP server '{}' returned HTTP {}", self.name, resp.status());
        }

        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if ct.contains("text/event-stream") {
            // Response is a per-request SSE stream — read events until matching id.
            use futures_util::StreamExt as _;
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.context("reading SSE response")?;
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(end_pos) = buf.find("\n\n") {
                    let event_block = buf[..end_pos].to_string();
                    buf = buf[end_pos + 2..].to_string();
                    let mut data = String::new();
                    for line in event_block.lines() {
                        if let Some(d) = line.strip_prefix("data: ") {
                            data = d.to_string();
                        }
                    }
                    if data.is_empty() {
                        continue;
                    }
                    let val: Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if val.get("id").and_then(|v| v.as_u64()) == Some(id) {
                        if let Some(err) = val.get("error") {
                            anyhow::bail!("MCP server '{}' returned error: {err}", self.name);
                        }
                        return val
                            .get("result")
                            .cloned()
                            .ok_or_else(|| anyhow::anyhow!("MCP response missing 'result' field"));
                    }
                }
            }
            anyhow::bail!("SSE stream ended without a response for request id {id}");
        } else {
            // JSON body response.
            let val: Value = resp
                .json()
                .await
                .with_context(|| format!("parsing JSON response from '{}'", self.name))?;
            if let Some(err) = val.get("error") {
                anyhow::bail!("MCP server '{}' returned error: {err}", self.name);
            }
            val.get("result")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("MCP response missing 'result' field"))
        }
    }

    async fn notify(&mut self, method: &str) -> Result<()> {
        let body = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        let _ = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .await;
        Ok(())
    }
}

/// Unified handle — stdio, HTTP+SSE, or Streamable HTTP.
enum McpHandle {
    Stdio(McpServerHandle),
    Sse(McpSseHandle),
    Streamable(McpStreamableHandle),
}

impl McpHandle {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        match self {
            McpHandle::Stdio(h) => h.request(method, params).await,
            McpHandle::Sse(h) => h.request(method, params).await,
            McpHandle::Streamable(h) => h.request(method, params).await,
        }
    }

    #[allow(dead_code)]
    async fn notify(&mut self, method: &str) -> Result<()> {
        match self {
            McpHandle::Stdio(h) => h.notify(method).await,
            McpHandle::Sse(h) => h.notify(method).await,
            McpHandle::Streamable(h) => h.notify(method).await,
        }
    }
}

impl McpServerHandle {
    /// Send a JSON-RPC request and wait for the matching response.
    /// Notifications and responses for other IDs are silently discarded.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = msg.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .with_context(|| format!("writing to MCP server '{}'", self.name))?;

        // Read lines until we see a response with our `id`.
        let mut buf = String::new();
        loop {
            buf.clear();
            let n = self
                .stdout
                .read_line(&mut buf)
                .await
                .with_context(|| format!("reading from MCP server '{}'", self.name))?;
            if n == 0 {
                anyhow::bail!("MCP server '{}' closed its stdout", self.name);
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let val: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue, // skip malformed lines
            };
            // Match by id — skip notifications (no `id` field) and unrelated responses.
            if val.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = val.get("error") {
                    anyhow::bail!("MCP server '{}' returned error: {err}", self.name);
                }
                return val
                    .get("result")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("MCP response missing 'result' field"));
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(&mut self, method: &str) -> Result<()> {
        let msg = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        let mut line = msg.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .with_context(|| format!("sending notification to MCP server '{}'", self.name))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-server wrapper (cached tool list + locked handle)
// ─────────────────────────────────────────────────────────────────────────────

struct McpServer {
    tools: Vec<McpTool>,
    handle: Arc<Mutex<McpHandle>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// McpManager — the public entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Manages all configured MCP servers and routes tool calls to the right one.
///
/// Cheap to clone (Arc-backed). Safe to pass across tokio::spawn boundaries.
pub struct McpManager {
    servers: Vec<McpServer>,
    /// Maps each tool name to the index of the server that owns it.
    tool_map: HashMap<String, usize>,
    /// Keeps the child processes alive (stdin/stdout were already extracted).
    children: Vec<Child>,
    /// Names of servers that failed to start, with the error reason.
    pub failed_servers: Vec<(String, String)>,
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.start_kill();
        }
    }
}

impl McpManager {
    /// Connect to all servers listed in `configs`, performing the MCP initialize
    /// handshake and collecting tool definitions.  Servers that fail to start are
    /// skipped with a warning — the manager is returned even if some servers fail.
    ///
    /// All servers are connected **concurrently** so startup time is bounded by
    /// the slowest single server rather than the sum of all servers.
    ///
    pub async fn from_config(configs: &[McpServerConfig]) -> Self {
        use tokio::task::JoinSet;

        // Spawn all connections concurrently, tagging each with its original index
        // so we can reassemble in config order (tool_map indices must be stable).
        // Returns (server, optional child) — HTTP servers have no child process.
        type ConnectResult = (usize, Result<(McpServer, Option<Child>)>);
        type ConnectSlot = Option<Result<(McpServer, Option<Child>)>>;
        let mut join_set: JoinSet<ConnectResult> = JoinSet::new();
        for (idx, cfg) in configs.iter().enumerate() {
            let cfg = cfg.clone();
            join_set.spawn(async move {
                let result =
                    tokio::time::timeout(tokio::time::Duration::from_secs(15), connect(&cfg))
                        .await
                        .unwrap_or_else(|_| {
                            Err(anyhow::anyhow!(
                                "timed out after 15 s — check that the server is reachable"
                            ))
                        });
                (idx, result)
            });
        }

        // Collect into a slot-per-server array to restore original ordering.
        let mut slots: Vec<ConnectSlot> = (0..configs.len()).map(|_| None).collect();
        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok((idx, result)) => slots[idx] = Some(result),
                Err(e) => warn!("MCP server task panicked: {e}"),
            }
        }

        let mut servers = Vec::new();
        let mut tool_map = HashMap::new();
        let mut children = Vec::new();
        let mut failed_servers = Vec::new();

        for (slot, cfg) in slots.into_iter().zip(configs.iter()) {
            match slot {
                Some(Ok((server, maybe_child))) => {
                    let idx = servers.len();
                    for tool in &server.tools {
                        tool_map.insert(tool.name.clone(), idx);
                    }
                    info!("MCP server '{}' connected ({} tools)", cfg.name, server.tools.len());
                    servers.push(server);
                    if let Some(child) = maybe_child {
                        children.push(child);
                    }
                },
                Some(Err(e)) => {
                    let reason = format!("{e:#}");
                    warn!("Failed to start MCP server '{}': {reason}", cfg.name);
                    failed_servers.push((cfg.name.clone(), reason));
                },
                None => {
                    warn!("MCP server '{}' task did not complete", cfg.name);
                    failed_servers.push((cfg.name.clone(), "task did not complete".to_string()));
                },
            }
        }

        McpManager { servers, tool_map, children, failed_servers }
    }

    /// Returns `true` if `name` is a tool provided by one of our MCP servers.
    pub fn is_mcp_tool(&self, name: &str) -> bool {
        self.tool_map.contains_key(name)
    }

    /// Returns all MCP tools in OpenAI function-calling format, ready to be
    /// appended to the `tools` array sent in every chat request.
    pub fn tool_definitions(&self) -> Vec<Value> {
        self.servers
            .iter()
            .flat_map(|s| s.tools.iter())
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect()
    }

    /// Returns (name, tool_count) for every successfully connected server.
    pub fn connected_servers(&self) -> Vec<(&str, usize)> {
        self.servers
            .iter()
            .map(|s| {
                let name = s.tools.first().map(|t| t.server_name.as_str()).unwrap_or("?");
                (name, s.tools.len())
            })
            .collect()
    }

    /// Returns a human-readable summary of connected servers (for status bar / logs).
    pub fn summary(&self) -> String {
        if self.servers.is_empty() {
            return "no MCP servers".to_string();
        }
        self.servers
            .iter()
            .map(|s| {
                let name = s.tools.first().map(|t| t.server_name.as_str()).unwrap_or("?");
                format!("{} ({})", name, s.tools.len())
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Execute an MCP tool call and return the result string (forwarded to the model).
    pub async fn call_tool(&self, name: &str, arguments: &str) -> String {
        let server_idx = match self.tool_map.get(name) {
            Some(idx) => *idx,
            None => return format!("unknown MCP tool: {name}"),
        };

        let args_val: Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return format!("invalid tool arguments: {e}"),
        };

        let server = &self.servers[server_idx];
        let mut handle = server.handle.lock().await;

        match handle
            .request("tools/call", serde_json::json!({ "name": name, "arguments": args_val }))
            .await
        {
            Ok(result) => extract_tool_result(&result),
            Err(e) => format!("MCP tool error: {e}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Connect to an MCP server using whichever transport the config specifies.
/// Returns `(server, Option<child>)` — HTTP servers have no child process.
async fn connect(cfg: &McpServerConfig) -> Result<(McpServer, Option<Child>)> {
    if let Some(url) = &cfg.url {
        let server = connect_http(cfg, url).await?;
        Ok((server, None))
    } else {
        let (server, child) = spawn_and_init(cfg).await?;
        Ok((server, Some(child)))
    }
}

/// HTTP transport dispatcher: tries HTTP+SSE first, then Streamable HTTP.
///
/// HTTP+SSE is the older MCP HTTP transport (persistent GET /sse stream).
/// Streamable HTTP is the newer transport (self-contained POST per request).
/// Auto-detection keeps both working without requiring explicit config.
async fn connect_http(cfg: &McpServerConfig, url: &str) -> Result<McpServer> {
    // ── 1. Try HTTP+SSE transport (3 s window for endpoint event) ────────────
    let sse_result = tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        McpSseHandle::connect(&cfg.name, url),
    )
    .await;

    match sse_result {
        Ok(Ok(handle)) => {
            // SSE transport connected — run the handshake.
            return run_http_handshake(cfg, McpHandle::Sse(handle)).await;
        },
        Ok(Err(e)) => {
            info!("'{}': HTTP+SSE transport unavailable ({e:#}), trying streamable HTTP", cfg.name);
        },
        Err(_) => {
            info!("'{}': HTTP+SSE timed out, trying streamable HTTP", cfg.name);
        },
    }

    // ── 2. Fall back to Streamable HTTP transport ─────────────────────────────
    let handle = McpStreamableHandle {
        name: cfg.name.clone(),
        url: url.to_string(),
        client: reqwest::Client::new(),
        next_id: 1,
    };
    run_http_handshake(cfg, McpHandle::Streamable(handle)).await
}

/// Perform the MCP initialize handshake and tool discovery on any HTTP handle.
async fn run_http_handshake(cfg: &McpServerConfig, mut handle: McpHandle) -> Result<McpServer> {
    handle
        .request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "forgiven", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
        .await
        .with_context(|| format!("MCP initialize for '{}'", cfg.name))?;

    handle
        .notify("notifications/initialized")
        .await
        .with_context(|| format!("MCP initialized notification for '{}'", cfg.name))?;

    let tools_result = handle
        .request("tools/list", serde_json::json!({}))
        .await
        .with_context(|| format!("MCP tools/list for '{}'", cfg.name))?;

    let tools = parse_tools(&cfg.name, &tools_result);
    Ok(McpServer { tools, handle: Arc::new(Mutex::new(handle)) })
}

/// stdio transport: spawn an MCP server process and perform the initialization handshake.
async fn spawn_and_init(cfg: &McpServerConfig) -> Result<(McpServer, Child)> {
    let mut cmd = Command::new(&cfg.command);
    cmd.args(&cfg.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());

    for (k, v) in &cfg.env {
        // Support $VAR_NAME syntax: read the value from the current process environment
        // so that secrets are never stored in config.toml.
        let resolved = if let Some(var_name) = v.strip_prefix('$') {
            std::env::var(var_name).unwrap_or_else(|_| {
                warn!(
                    "MCP server '{}': env var ${} is not set in the shell environment",
                    cfg.name, var_name
                );
                String::new()
            })
        } else {
            v.clone()
        };
        cmd.env(k, resolved);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning MCP server '{}' ({})", cfg.name, cfg.command))?;

    let stdin = child.stdin.take().context("MCP server stdin unavailable")?;
    let stdout = child.stdout.take().context("MCP server stdout unavailable")?;

    let mut handle = McpServerHandle {
        name: cfg.name.clone(),
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
    };

    // ── initialize ────────────────────────────────────────────────────────────
    handle
        .request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "forgiven", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
        .await
        .with_context(|| format!("MCP initialize for '{}'", cfg.name))?;

    handle
        .notify("notifications/initialized")
        .await
        .with_context(|| format!("MCP initialized notification for '{}'", cfg.name))?;

    // ── tools/list ───────────────────────────────────────────────────────────
    let tools_result = handle
        .request("tools/list", serde_json::json!({}))
        .await
        .with_context(|| format!("MCP tools/list for '{}'", cfg.name))?;

    let tools = parse_tools(&cfg.name, &tools_result);
    let server = McpServer { tools, handle: Arc::new(Mutex::new(McpHandle::Stdio(handle))) };
    Ok((server, child))
}

/// Parse the `tools/list` result into `McpTool` structs.
fn parse_tools(server_name: &str, result: &Value) -> Vec<McpTool> {
    let Some(arr) = result.get("tools").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|t| {
            let name = t.get("name")?.as_str()?.to_string();
            let description =
                t.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let input_schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
            Some(McpTool { server_name: server_name.to_string(), name, description, input_schema })
        })
        .collect()
}

/// Extract the text content from an MCP `tools/call` response.
///
/// MCP response shape:
/// ```json
/// { "content": [{ "type": "text", "text": "..." }], "isError": false }
/// ```
fn extract_tool_result(result: &Value) -> String {
    // Check for error flag
    if result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
        let msg = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|item| item.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return format!("error: {msg}");
    }

    let Some(content) = result.get("content").and_then(|v| v.as_array()) else {
        // Fallback: render the whole result as JSON
        return result.to_string();
    };

    let parts: Vec<&str> = content
        .iter()
        .filter_map(|item| {
            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                item.get("text").and_then(|v| v.as_str())
            } else {
                None
            }
        })
        .collect();

    if parts.is_empty() {
        "(no text content)".to_string()
    } else {
        parts.join("\n")
    }
}
