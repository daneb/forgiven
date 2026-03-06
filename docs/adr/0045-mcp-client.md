# ADR 0045 — MCP Client Integration

**Date:** 2026-03-06
**Status:** Accepted

## Context

Model Context Protocol (MCP) is an open standard for connecting AI assistants to external tools and data sources via a JSON-RPC 2.0 protocol over stdio (or HTTP/SSE). Integrating an MCP client lets the Copilot agent in forgiven call tools from any MCP server the user configures — filesystem, git, databases, web search, etc. — without hard-coding each capability.

## Decision

Implement an MCP client (`src/mcp/mod.rs`) that:

1. Spawns configured MCP server processes at startup.
2. Performs the MCP initialize handshake (initialize → notifications/initialized).
3. Fetches and caches each server's tool list via `tools/list`.
4. Exposes the tools in OpenAI function-calling format so they are automatically included in every agentic loop request.
5. Routes tool calls whose names belong to an MCP server to that server via `tools/call`.

Built-in agentic tools (`read_file`, `write_file`, `edit_file`, `list_directory`, `create_task`, `complete_task`) are unchanged — MCP tools are additive.

## Key Structures

- `McpManager` (`src/mcp/mod.rs`) — owns child processes and per-server locked handles.
- `McpServerHandle` — wraps `ChildStdin`/`BufReader<ChildStdout>` with a simple request/response loop keyed on JSON-RPC `id`.
- `McpTool` — cached tool definition (name, description, input_schema) used to build OpenAI tool defs.

## Config

```toml
# ~/.config/forgiven/config.toml

[[mcp.servers]]
name    = "filesystem"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[[mcp.servers]]
name    = "git"
command = "uvx"
args    = ["mcp-server-git"]

# optional per-server env vars
[[mcp.servers]]
name    = "my-server"
command = "/usr/local/bin/my-mcp-server"
args    = []
[mcp.servers.env]
API_KEY = "secret"
```

## Integration Points

| File | Change |
|---|---|
| `src/mcp/mod.rs` | New — McpManager, McpServer, McpServerHandle |
| `src/config/mod.rs` | Added `McpServerConfig`, `McpConfig`, `mcp: McpConfig` on `Config` |
| `src/agent/mod.rs` | `AgentPanel.mcp_manager: Option<Arc<McpManager>>`; `agentic_loop` merges MCP tool defs and dispatches MCP tool calls |
| `src/editor/mod.rs` | `Editor.mcp_manager`; `setup_mcp()` initializes and wires into agent panel |
| `src/main.rs` | Calls `editor.setup_mcp().await` between `setup_lsp()` and `run()` |

## Protocol Details

- Transport: stdio, newline-delimited JSON (one JSON object per line).
- Handshake: `initialize` → `notifications/initialized` → `tools/list`.
- Tool call: `tools/call { name, arguments }` → `{ content: [{ type: "text", text }], isError }`.
- Responses are matched by JSON-RPC `id`; notifications and unrelated responses are discarded.
- Per-server `Mutex<McpServerHandle>` serializes requests — sufficient for the sequential agentic loop.

## Consequences

- Users can now extend the agent with any MCP server by adding entries to `config.toml`.
- Servers that fail to start are skipped with a `warn!` log — the editor starts normally.
- No new crate dependencies — uses existing `tokio`, `serde_json`, and `anyhow`.
