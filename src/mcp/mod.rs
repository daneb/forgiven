//! MCP (Model Context Protocol) client.
//!
//! Connects to one or more MCP servers over stdio (newline-delimited JSON-RPC 2.0),
//! discovers their tools, and routes tool calls from the agentic loop to the
//! appropriate server.
//!
//! Config (`~/.config/forgiven/config.toml`):
//! ```toml
//! [[mcp.servers]]
//! name    = "filesystem"
//! command = "npx"
//! args    = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
//!
//! [[mcp.servers]]
//! name    = "git"
//! command = "uvx"
//! args    = ["mcp-server-git"]
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
// Internal handle (holds the live stdio connection)
// ─────────────────────────────────────────────────────────────────────────────

struct McpServerHandle {
    name: String,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
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
    handle: Arc<Mutex<McpServerHandle>>,
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
    _children: Vec<Child>,
    /// Names of servers that failed to start, with the error reason.
    pub failed_servers: Vec<(String, String)>,
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
        let mut join_set: JoinSet<(usize, Result<(McpServer, Child)>)> = JoinSet::new();
        for (idx, cfg) in configs.iter().enumerate() {
            let cfg = cfg.clone();
            join_set.spawn(async move { (idx, spawn_and_init(&cfg).await) });
        }

        // Collect into a slot-per-server array to restore original ordering.
        let mut slots: Vec<Option<Result<(McpServer, Child)>>> =
            (0..configs.len()).map(|_| None).collect();
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
                Some(Ok((server, child))) => {
                    let idx = servers.len();
                    for tool in &server.tools {
                        tool_map.insert(tool.name.clone(), idx);
                    }
                    info!("MCP server '{}' connected ({} tools)", cfg.name, server.tools.len());
                    servers.push(server);
                    children.push(child);
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

        McpManager { servers, tool_map, _children: children, failed_servers }
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

/// Spawn an MCP server process and perform the initialization handshake.
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

    let server = McpServer { tools, handle: Arc::new(Mutex::new(handle)) };

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
