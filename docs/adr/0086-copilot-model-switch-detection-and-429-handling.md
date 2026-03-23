# ADR 0086 — Copilot Model-Switch Detection and 429 Rate-Limit Handling

**Date:** 2026-03-23
**Status:** Accepted

---

## Context

GitHub Copilot silently downgrades requests to a lower-tier model (e.g. GPT-4.1)
when a user's premium request allowance is exhausted. The editor had no awareness
of this: the panel title continued to show the originally selected model, and the
user received no in-chat notice.

Separately, when Copilot responds with HTTP 429 (Too Many Requests) the retry
loop did not read the `Retry-After` header. It applied its own exponential
backoff, which could be far shorter than the server-indicated wait and wasted
retries, or far longer than necessary for transient limits. If the quota was
exhausted (Retry-After in the hundreds of seconds) the loop would still retry 5
times before surfacing a generic "Max retries reached" error with no actionable
advice.

---

## Decision

### Model-switch detection

The SSE stream for chat completions includes a `model` field on each chunk. The
existing code already logged a mismatch between the requested model and the
actual model. This was promoted to a first-class `StreamEvent`:

```rust
StreamEvent::ModelSwitched { from: String, to: String }
```

The streaming loop emits this event on the first chunk when `actual != requested`.

`poll_stream()` handles `ModelSwitched` by:
1. Updating `self.selected_model` to the index of the new model in
   `available_models` (so the panel title reflects reality immediately).
2. Injecting a blockquote notice inline in the chat before the assistant reply:

```
> ⚠  Copilot switched model: claude-sonnet-4 → gpt-4.1 (premium quota exceeded)
```

### 429 Retry-After handling

The 429 branch in `start_chat_stream_with_tools` was refactored to:

1. **Read the `Retry-After` header** before consuming the response body (reqwest
   exposes headers independently of the body stream).
2. **Fail fast if `Retry-After > 120 s`** — a large value indicates quota
   exhaustion rather than a transient rate limit. The error message includes the
   wait duration and a hint to switch models with `Ctrl+T`.
3. **Use `Retry-After` as the sleep delay** when it is present and ≤ 120 s,
   overriding the exponential backoff value for that iteration.
4. **Continue the loop** for short-lived 429s the same as before, still subject
   to the 5-retry cap.

---

## Consequences

- Users see an in-chat warning immediately when Copilot degrades their model,
  without needing to check the status bar or logs.
- The panel title updates to show the actual model being served.
- Quota-exhaustion 429s (long `Retry-After`) surface a clear, actionable error
  immediately rather than after 5 futile retries and ~31 s of backoff.
- Transient 429s with a short `Retry-After` wait the server-indicated time,
  improving success rate on the retry.
- No new dependencies.
