# ADR 0062 — Offline Resilience: Request Timeouts, MCP Startup Bound, and Error Visibility

**Date:** 2026-03-14
**Status:** Accepted

---

## Context

Three gaps were identified when the editor starts with no internet connection or
when the network drops mid-session:

### 1. No per-request HTTP timeout

Every `reqwest::Client` was built with `Client::new()`, which inherits
reqwest's default of **no timeout**. On a hung or dropped TCP connection a
single `send().await` can block indefinitely. The retry loops in
`exchange_token`, `fetch_models`, `start_chat_stream_with_tools`, and
`one_shot_complete` all assume each attempt eventually completes; without a
timeout this assumption breaks silently, stalling the background task for an
unbounded time.

### 2. MCP `spawn_and_init` had no timeout

`McpManager::from_config` runs one `JoinSet` task per configured MCP server,
collecting results with `join_next().await`. Each task calls `spawn_and_init`,
which performs three sequential async operations (process spawn, `initialize`
handshake, `tools/list`). None of these operations had a deadline.

MCP servers that proxy remote endpoints (e.g. `mcp-remote` →
`mcp.atlassian.com`) can stall indefinitely if the upstream is unreachable.
Because the editor's MCP task runs in the background (ADR 0053), the editor
remains interactive — but the task leaks and the server never appears as
"failed" in the diagnostics overlay.

### 3. Network errors were silent to the user

Two error paths produced no user-visible output:

- **`submit()` failure** — if `ensure_token()` failed (missing OAuth file,
  token exchange network error), the error was caught in `editor/mod.rs` and
  only forwarded to `tracing::warn!()`. The status bar was never updated; the
  agent panel showed nothing.
- **`StreamEvent::Error`** — stream errors were already appended to the agent
  message history as `[Error: …]`, but if the agent panel was not in view the
  user had no indication that anything had failed.

---

## Decision

### Reqwest 15-second per-request timeout

All four `reqwest::Client` constructions in `src/agent/mod.rs` are replaced
with:

```rust
reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(15))
    .build()
    .unwrap_or_default()
```

15 seconds gives enough headroom for slow or congested connections while
bounding the worst-case hang. The existing retry loops (3 retries for token
exchange, 5 retries for model fetch and chat stream) remain unchanged; each
individual attempt is now time-bounded.

The 60-second SSE stream-stall timeout (added in ADR 0046) is unaffected —
it covers the streaming phase where per-chunk delays are expected.

### MCP 15-second spawn timeout

Each `JoinSet` task in `McpManager::from_config` is wrapped with
`tokio::time::timeout`:

```rust
join_set.spawn(async move {
    let result = tokio::time::timeout(
        tokio::time::Duration::from_secs(15),
        spawn_and_init(&cfg),
    )
    .await
    .unwrap_or_else(|_| Err(anyhow::anyhow!(
        "timed out after 15 s — check that the server is reachable"
    )));
    (idx, result)
});
```

On timeout, the `Err` falls into the existing `Some(Err(e))` arm in
`from_config`, which logs a warning and pushes the server into `failed_servers`.
The failed entry then appears in the `SPC d` diagnostics overlay (ADR 0049)
with the timeout message, and in the agent panel bottom bar (ADR 0048) as a
red `⚠` indicator.

No changes to `spawn_and_init` itself were needed.

### Error visibility — two paths

**Path A — `submit()` failure**

In `src/editor/mod.rs`, the `block_in_place` closure that drives
`agent_panel.submit()` now returns `Option<String>`:

```rust
let submit_err = tokio::task::block_in_place(|| {
    tokio::runtime::Handle::current().block_on(async {
        match fut.await {
            Ok(()) => None,
            Err(e) => { tracing::warn!("Agent submit error: {e}"); Some(e.to_string()) }
        }
    })
});
if let Some(e) = submit_err {
    self.set_status(format!("Agent error: {e}"));
}
```

This covers pre-stream failures: missing OAuth token, failed token exchange,
and any other error returned by `submit()` before the stream channel is created.

**Path B — `StreamEvent::Error`**

A new field `pub last_error: Option<String>` is added to `AgentPanel`.
The `StreamEvent::Error` arm in `poll_stream()` sets it:

```rust
self.last_error = Some(e);
```

The editor run-loop reads and clears it immediately after calling
`poll_stream()`:

```rust
let agent_active = self.agent_panel.poll_stream();
if let Some(err) = self.agent_panel.last_error.take() {
    self.set_status(format!("Agent error: {err}"));
}
```

The error still appears in the agent message history as `[Error: …]` (existing
behaviour); the status bar message is additive and ensures visibility regardless
of which panel is in focus.

---

## Behaviour when internet returns

| Component | Recovery |
|-----------|----------|
| **Agent / Copilot** | Automatic — `ensure_token()` re-exchanges on the next submit; `ensure_models()` retries lazily |
| **LSP** | Manual restart required — no reconnection logic for copilot-ls |
| **MCP** | Manual restart required — timed-out servers remain in `failed_servers` for the session |

---

## Alternatives considered

**Longer timeout (e.g. 30 s)**
More forgiving for very slow connections, but doubles the worst-case hang time
per retry attempt. 15 s is the same threshold used by the LSP `initialize`
phase (ADR 0003) and keeps behaviour consistent across subsystems.

**Surfacing errors as a modal popup**
A blocking popup would interrupt editing. The status bar is non-blocking and
consistent with all other transient messages in the editor.

**MCP reconnection on network recovery**
Automatically re-running `from_config` when internet becomes available would
require a network-state monitor and careful state management (tool-map indices
must remain stable). Deferred to a future ADR.

---

## Consequences

**Positive**
- A stalled HTTP connection is now detected within 15 seconds across all
  Copilot API calls.
- MCP servers that are unreachable at startup fail within 15 seconds and
  appear immediately in the diagnostics overlay.
- Auth failures and stream errors are shown in the status bar, giving the user
  actionable feedback without requiring `SPC d`.

**Negative / trade-offs**
- On a genuinely slow connection a single request might legitimately take
  more than 15 seconds. In practice Copilot API responses start within
  1–2 seconds on any working connection; 15 s is conservative.
- `last_error: Option<String>` adds a field to `AgentPanel`; cost is
  negligible.
