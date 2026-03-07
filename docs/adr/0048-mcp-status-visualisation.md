# ADR 0048: MCP Server Status Visualisation

**Date:** 2026-03-07
**Status:** Accepted

## Context

Forgiven supports multiple MCP servers configured in `~/.config/forgiven/config.toml`. Servers are connected at startup via `McpManager::from_config()`; any server that fails to start is silently skipped with a `warn!` log entry. There was no way for the user to confirm which servers were active or diagnose connection failures without reading the log file.

Additionally, the `setup_mcp()` function in `editor/mod.rs` previously only wired `mcp_manager` into the agent panel if `has_tools()` returned true — meaning a misconfigured or slow-to-initialise server would leave the panel with `mcp_manager: None` and no feedback at all.

## Decision

### `McpManager` changes (`src/mcp/mod.rs`)

- Added `pub failed_servers: Vec<(String, String)>` to `McpManager` — populated in `from_config()` when `spawn_and_init()` returns an error. The stored error string is the first line of the `anyhow` error chain (concise, single-line).
- Added `pub fn connected_servers() -> Vec<(&str, usize)>` — returns `(server_name, tool_count)` for every successfully connected server, replacing the old `summary()` string in UI code.
- Removed `has_tools()` — its only consumer was `setup_mcp()` which now unconditionally wires the manager.

### `setup_mcp()` change (`src/editor/mod.rs`)

Removed the `if manager.has_tools()` guard. The manager is now always assigned to `self.mcp_manager` and `self.agent_panel.mcp_manager` when servers are configured, so the UI always reflects the true state (even if all servers failed).

### UI change (`src/ui/mod.rs`)

`mcp_bottom` in `render_agent_panel()` switched from a single `Span` to a `Line` of multiple spans, rendered as `title_bottom` on the chat history block:

| Condition | Rendering |
|-----------|-----------|
| No `mcp_manager` | `MCP: none` (dark gray) |
| Connected servers | `MCP: github (45 tools), filesystem (10 tools)` (dim green per server) |
| Failed servers | `⚠ sequential-thinking: <first error line>` (red, appended after connected list) |
| Connected but no tools | `MCP: no tools` (dark gray) |

The scroll logic in the same function was also corrected in this session (ADR companion): the old approach sliced `lines` by logical line count, causing the bottom of the panel to be clipped when code-block lines wrapped to multiple display rows. The fix switches to `Paragraph::scroll((row_offset, 0))` where `row_offset` is computed from the sum of display rows (character count ÷ inner panel width) across all lines.

### Config additions (`~/.config/forgiven/config.toml`)

Four MCP servers added to the user config:

| Name | Package |
|------|---------|
| `filesystem` | `@modelcontextprotocol/server-filesystem` |
| `fetch` | `@modelcontextprotocol/server-fetch` |
| `sequential-thinking` | `@modelcontextprotocol/server-sequential-thinking` |
| `memory` | `@modelcontextprotocol/server-memory` |

GitHub MCP was added in the same session, using Docker (`ghcr.io/github/github-mcp-server`) with the token passed via `env`.

## Consequences

- Users immediately see which MCP servers are active and how many tools each exposes, without leaving the editor.
- Failed servers are surfaced with a one-line reason in red — no log-diving required.
- `setup_mcp()` now always reflects configured servers regardless of tool count, closing a silent-failure gap.
- The scroll fix is a correctness improvement independent of MCP: long responses containing wide code blocks now render fully to the bottom of the panel.
