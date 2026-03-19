# ADR 0072 — MCP Child Process Cleanup on Exit

**Date:** 2026-03-18
**Status:** Accepted

---

## Context

ADR 0045 introduced the MCP client (`src/mcp/mod.rs`). Each configured MCP server
is launched as a child process via `tokio::process::Command::spawn()`. For
Docker-based servers (e.g. `docker run --rm -i isokoliuk/mcp-searxng`), the
container lifecycle is tied to stdin: the container runs while stdin is open, and
`--rm` removes it when the process exits.

The child handles were stored in `_children: Vec<Child>` on `McpManager`. The
leading underscore was only a lint-suppression convention — there was no explicit
cleanup. When `McpManager` was dropped, Tokio's `Child` drop impl does **not** kill
the subprocess; it merely drops the owned handle. The stdin pipe to the Docker
container therefore remained open (held by the OS pipe buffer), and the container
continued running indefinitely.

In practice, each time forgiven started it spawned a fresh Docker container. The
old containers from previous sessions were never stopped, leading to unbounded
accumulation. After several days of regular use, 21 stale `isokoliuk/mcp-searxng`
containers were discovered still running.

---

## Decision

Implement `Drop` for `McpManager` that explicitly kills all child processes.

### Change

`_children` is renamed to `children` (the underscore was only suppressing an
"unused field" warning; with a `Drop` impl the field is actively used).

A `Drop` implementation is added immediately after the struct definition:

```rust
impl Drop for McpManager {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.start_kill();
        }
    }
}
```

`start_kill()` is the non-async variant of `Child::kill()` from
`tokio::process::Child`. It sends `SIGKILL` (Unix) / `TerminateProcess` (Windows)
without requiring an `.await`. This is appropriate in a synchronous `Drop` context.
The return value is ignored — if the process has already exited, the call is a
no-op.

When Docker receives `SIGKILL` on the `docker run` process, the container is
stopped and, because `--rm` was specified, immediately removed. This ensures at
most one running container per configured MCP server at any time.

---

## Consequences

- Docker-based MCP servers no longer accumulate across forgiven sessions — each
  exit (clean or crash) kills the child process and triggers `--rm` cleanup.
- stdio-based MCP servers (e.g. `npx mcp-remote`) are also killed on exit,
  preventing orphaned Node processes.
- `start_kill()` is fire-and-forget; it does not wait for the process to fully
  exit. For `--rm` containers this is fine: Docker handles container removal
  asynchronously after receiving the signal.
- There is no graceful MCP shutdown (i.e. no JSON-RPC `shutdown` notification is
  sent before killing). This matches the existing behaviour for LSP servers and is
  acceptable for short-lived tool-calling use.

### Files changed

| File | Change |
|------|--------|
| `src/mcp/mod.rs` | `_children` → `children`; `Drop` impl added to `McpManager` |

---

## Related ADRs

- **ADR 0045** — MCP client (introduced `McpManager` and child process spawning)
- **ADR 0053** — MCP non-blocking startup
