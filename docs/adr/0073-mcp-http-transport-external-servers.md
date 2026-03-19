# ADR 0073 — MCP HTTP Transport & Stdio Server Environment Variables

**Date:** 2026-03-19
**Status:** Accepted

---

## Context

ADR 0072 added `Drop`-based cleanup so child MCP processes are killed when the
editor exits. This fixed container accumulation in the general case — each
editor session now spawns exactly one container and it is killed on exit.

However, when the `isokoliuk/mcp-searxng` server was investigated, two
additional problems were discovered:

1. **Missing required environment variable.** The container exited immediately
   at startup with `SEARXNG_URL not set`, causing the MCP handshake to fail
   with `MCP server 'searxng' closed its stdout`. Without `SEARXNG_URL` the
   server has no search engine to query and cannot start.

2. **Wrong transport assumption.** It was initially theorised that the
   accumulation problem could be solved by switching `isokoliuk/mcp-searxng`
   to HTTP transport — letting a persistent, user-managed container run
   externally. Investigation proved this impossible:

   | Probe | Result |
   |-------|--------|
   | `GET {url}/sse` | HTTP 404 — no `/sse` endpoint |
   | `GET {url}/` (SSE fallback) | Connection closes before `endpoint` event |
   | `POST {url}/` (Streamable HTTP) | Empty response body — `expected value at line 1 column 1` |

   `isokoliuk/mcp-searxng` is a **stdio-only** MCP server. Port 8080 (when
   exposed via `-p 8080:8080`) is the SearXNG web search UI, not an MCP
   endpoint. The server only communicates via stdin/stdout.

---

## Decision

### For `isokoliuk/mcp-searxng`: stay on stdio, add `SEARXNG_URL`

The server remains a stdio child process. The `Drop` cleanup from ADR 0072
ensures exactly one container runs per editor session and is removed on exit.

The `SEARXNG_URL` environment variable is required and is resolved from the
host environment at startup using the `$`-prefix convention (same as
`$GITHUB_PERSONAL_ACCESS_TOKEN`).

**Working config:**
```toml
[[mcp.servers]]
name    = "searxng"
command = "docker"
args    = ["run", "--rm", "-i", "-e", "SEARXNG_URL", "isokoliuk/mcp-searxng"]

[mcp.servers.env]
SEARXNG_URL = "$SEARXNG_URL"
```

**Shell environment (add to `.zshrc` / `.zprofile`):**
```sh
export SEARXNG_URL="http://searxng.searxng-mcp.orb.local"
# or any SearXNG instance: http://localhost:8080, https://searx.be, etc.
```

The user's OrbStack-managed SearXNG instance
(`http://searxng.searxng-mcp.orb.local`) is the recommended local option — it
is persistent, managed outside the editor, and has no per-session lifecycle.

### Multiple-instances question: resolved by ADR 0072 + correct config

With `--rm -i` and the `Drop` kill in ADR 0072:

| Scenario | Containers running |
|----------|--------------------|
| Editor not open | 0 |
| One editor session | 1 (killed and removed on exit) |
| Two simultaneous editor sessions | 2 (each owns one, both cleaned up) |
| Editor crashed without graceful exit | 0 (SIGKILL propagates; `--rm` removes) |

This is the correct behaviour. A single persistent SearXNG instance (OrbStack,
Docker Compose, etc.) is shared by all editor sessions; only the thin MCP
wrapper container is per-session and short-lived.

### HTTP transport: built, valid for other servers

The HTTP+SSE transport implemented in this ADR cycle (see Implementation
section) remains in the codebase. It is correct infrastructure for MCP servers
that do expose HTTP endpoints (e.g. future self-hosted servers, servers running
as system services). It simply does not apply to `isokoliuk/mcp-searxng`.

---

## Implementation

### `src/config/mod.rs`

`McpServerConfig` has an optional `url` field. When present, the editor uses
HTTP transport instead of spawning a child process:

```rust
pub struct McpServerConfig {
    pub name: String,
    /// HTTP URL for an externally-managed MCP server.
    /// When set, command/args/env are ignored — no process is spawned.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}
```

`command` is `#[serde(default)]` so a URL-only entry deserialises without
error.

### `src/mcp/mod.rs`

Two internal transport types:

- **`McpSseHandle`** — persistent `GET /sse` connection; reads the `endpoint`
  event to get the POST URL; background task forwards SSE `data:` events to a
  channel; `request()` POSTs JSON-RPC and reads from the channel until the
  matching `id` arrives.
- **`McpHandle`** enum — wraps `Stdio(McpServerHandle)` or `Sse(McpSseHandle)`.

`connect()` dispatcher routes based on `cfg.url`. `McpManager.children` only
holds stdio children; SSE connections contribute nothing to the `Vec`.

### `spawn_and_init` — env var resolution

The existing `$`-prefix resolution in `spawn_and_init` handles `SEARXNG_URL`
automatically. Values beginning with `$` are looked up via `std::env::var()`;
missing vars emit a `WARN` log (visible in `SPC d`) and the literal string is
used as a fallback.

---

## Consequences

**Positive**
- `searxng` now connects successfully; tools are available immediately on
  editor startup (confirmed: `✓ searxng N tools` in `SPC d`).
- Zero container accumulation — the `Drop` fix in ADR 0072 + `--rm` guarantees
  cleanup on every exit path.
- `SEARXNG_URL` decouples the MCP wrapper from any specific SearXNG deployment;
  the user can point it at local (OrbStack), remote, or a public instance.
- HTTP transport is available in the codebase for future MCP servers that need
  it, with no additional dependencies.

**Negative / trade-offs**
- One ephemeral `isokoliuk/mcp-searxng` container runs per editor session.
  This is acceptable — it is a thin stdio wrapper, not the SearXNG service
  itself.
- If `SEARXNG_URL` is unset or the target is unreachable, the MCP server exits
  immediately and the editor shows it as failed in `SPC d`.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0045](0045-mcp-client.md) | Introduced the MCP client and stdio transport |
| [0050](0050-mcp-env-var-secret-resolution.md) | `$`-prefix env var resolution used for `SEARXNG_URL` |
| [0053](0053-mcp-container-isolation.md) | Deferred HTTP transport — partially resolved here |
| [0072](0072-mcp-child-process-cleanup.md) | `Drop`-based child kill — prerequisite for the multiple-instances fix |
