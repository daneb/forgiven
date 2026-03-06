# ADR 0046 — Agent Retry Visibility

**Date:** 2026-03-06
**Status:** Accepted

## Context

When the Copilot Chat API returns a retryable error (HTTP 429 rate-limit, 5xx server error, or a network failure), `start_chat_stream_with_tools` silently retries up to 5 times with exponential backoff (1 → 2 → 4 → 8 → 16 seconds, up to 31 seconds total). During this entire window:

1. No `StreamEvent` was emitted to the UI thread, so `AgentStatus` stayed frozen at `WaitingForResponse { round: 1 }` — showing `"waiting… [1/20]"` indefinitely.
2. The final error message `"Max retries reached for Copilot Chat API"` gave no indication of *why* retries were needed (HTTP status, network error, etc.).

The result: the panel appeared hung in "planning mode" with no feedback, then suddenly showed an opaque error.

## Decision

### 1. `AgentStatus::Retrying` variant

Added a new variant to `AgentStatus`:

```rust
Retrying { attempt: usize, max: usize },
```

Its `label()` renders as `"retrying (2/5)…"` in the panel title, making the retry progress visible to the user.

### 2. `StreamEvent::Retrying` variant

Added a corresponding event:

```rust
Retrying { attempt: usize, max: usize },
```

Handled in `poll_stream()` by setting `self.status = AgentStatus::Retrying { attempt, max }`.

### 3. `tx` passed into `start_chat_stream_with_tools`

The function now accepts `tx: &mpsc::UnboundedSender<StreamEvent>` and emits `StreamEvent::Retrying` after incrementing the attempt counter, before sleeping:

```rust
let _ = tx.send(StreamEvent::Retrying { attempt: retry_attempts, max: max_retries });
tokio::time::sleep(delay).await;
```

### 4. Better error message

The retry loop captures the failure reason per-iteration as a local `failure_reason: String` (computed from the match arm that triggered the retry). On exhaustion:

```
Max retries reached for Copilot Chat API (last error: HTTP 503)
Max retries reached for Copilot Chat API (last error: connection reset by peer)
```

## Key Changes

| File | Change |
|---|---|
| `src/agent/mod.rs` | Added `AgentStatus::Retrying`; added `StreamEvent::Retrying`; `start_chat_stream_with_tools` accepts `tx`, emits `Retrying` events, captures `failure_reason`; call site in `agentic_loop` passes `&tx` |

## Consequences

- Users see `"retrying (N/5)…"` in the panel title during backoff rather than a frozen `"waiting…"` state.
- Error messages now identify the root cause (HTTP status or network error string).
- No new dependencies; no behaviour change on the happy path.
