# ADR 0053 — MCP Non-blocking Startup

**Date:** 2026-03-10
**Status:** Accepted (isolation superseded; HTTP transport delivered in [ADR 0073](0073-mcp-http-transport-external-servers.md))

---

## Context

MCP servers were started inside `setup_services()` using `tokio::join!` alongside
LSP, blocking the editor loading screen until every server completed its handshake.
With slow or containerised servers this caused 30 s+ startup drag.

A separate goal was to run each MCP server inside a Docker container for isolation.
This was explored in two forms:

1. **Built-in container config** (`McpContainerConfig` fields on `McpServerConfig`)
   — reverted: coupled security policy into the editor schema and caused the same
   30 s drag.

2. **Wrapper scripts** (`~/.local/bin/mcp-*`) — attempted but abandoned: the
   editor spawns child processes with a restricted PATH (`/usr/bin:/bin`), Docker
   lives at `/usr/local/bin/docker`, and the interaction between the restricted
   environment, npm offline cache metadata staleness, and Docker socket discovery
   produced too many failure modes to be reliable in practice.

The isolation question remains open. The editor currently offers no isolation
beyond what the OS provides to any child process.

---

## Decision

**Decouple MCP startup from the loading screen.**

`setup_services()` awaits LSP synchronously (the editor needs completions and
diagnostics immediately) and fires MCP as a background `tokio::spawn`.  A
`oneshot::Receiver<McpManager>` stored on `Editor` is polled each run-loop tick;
when all handshakes complete the manager is wired into `agent_panel`.

```
setup_services()
  ├── lsp::init_servers_parallel().await    ← blocks: editor needs LSP
  └── tokio::spawn(McpManager::from_config)
            └── oneshot::Sender<McpManager>
                      │
                      ▼
              editor.mcp_rx  ──poll each tick──▶  agent_panel.mcp_manager
```

---

## Implementation

- **`src/editor/mod.rs`**:
  - Added `mcp_rx: Option<oneshot::Receiver<McpManager>>` to `Editor`.
  - `setup_services()` splits LSP (awaited) from MCP (background `tokio::spawn`).
  - Run-loop tick polls `mcp_rx.try_recv()`; on `Ok`, wires `McpManager` into
    `self.mcp_manager` and `self.agent_panel.mcp_manager`.
  - `mcp_rx.is_some()` added to the `needs_render` guard so the agent panel
    status bar refreshes while servers are connecting.

No new dependencies.

---

## Isolation — what was learned

| Approach | Outcome |
|---|---|
| Built-in `McpContainerConfig` in editor schema | Reverted — wrong layer, same startup drag |
| Wrapper scripts in `~/.local/bin/` | Abandoned — too fragile (PATH, Docker socket, npm cache metadata) |
| Do nothing (current state) | MCP servers run as direct host processes |

The right long-term answer is **MCP over HTTP transport** (servers run as
persistent system services, editor just connects). This was delivered in
**[ADR 0073](0073-mcp-http-transport-external-servers.md)**.

---

## Consequences

- **Positive**: editor opens at LSP-ready speed regardless of MCP count or slowness.
- **Positive**: MCP connection status visible in agent panel bottom bar and `SPC d`
  while servers are still connecting in the background.
- **Negative**: MCP tools unavailable for the first few seconds after open.
- **Negative**: MCP servers have full host access — no isolation in place.
