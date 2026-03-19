# ADR 0074 — Agentic Loop Mid-Session Token Refresh

**Date:** 2026-03-19
**Status:** Accepted

---

## Context

The Copilot Chat API uses a **short-lived API token** (typically ~30 minutes)
that is obtained by exchanging the user's long-lived OAuth token (stored in
`~/.config/github-copilot/apps.json`). The existing `ensure_token()` method on
`AgentPanel` guards this exchange with a 60-second early-expiry buffer, so
tokens are proactively refreshed at session start.

However, once `submit()` spawns `agentic_loop()`, the token is passed in as a
plain `String`. The loop has no path back to `self.token` and no mechanism to
refresh mid-run. On long agentic sessions — many tool-call rounds, slow tools,
or a session left idle before sending — the token can expire inside the loop.
Every subsequent API call then returns:

```
Error: Copilot Chat API error (401 Unauthorized): IDE token expired: unauthorized: token expired
```

The retry logic introduced in ADR 0046 does not help here: 4xx errors (except
429) were treated as permanent failures and returned immediately without retry.

### Why `Arc<Mutex<>>` was not chosen

A shared token handle was considered. It would work — the lock is never held
across an `.await`, so `std::sync::Mutex` is safe and performant — but it adds
shared state complexity with no additional benefit. The loop runs serially (one
API call at a time) and only ever needs one token refresh path. The simpler
inline approach is a complete solution.

---

## Decision

Detect a 401 response specifically in `start_chat_stream_with_tools`, surface it
via a typed sentinel error (`TokenExpiredError`), and handle it in
`agentic_loop` by refreshing the token inline and retrying the current round
once before surfacing any error to the user.

---

## Implementation

### `src/agent/mod.rs`

**`TokenExpiredError` sentinel type**

A minimal `std::error::Error` impl placed near `CopilotApiToken`:

```rust
#[derive(Debug)]
struct TokenExpiredError;

impl std::fmt::Display for TokenExpiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Copilot API token expired")
    }
}

impl std::error::Error for TokenExpiredError {}
```

Using a concrete type (rather than a string check) lets `anyhow`'s
`e.is::<TokenExpiredError>()` downcast cleanly without coupling the call site
to error message wording.

**`start_chat_stream_with_tools` — 401 separated from other 4xx**

```rust
// Before: all 4xx except 429 → permanent generic Err
if status.is_client_error() && status.as_u16() != 429 {
    return Err(anyhow::anyhow!("Copilot Chat API error ({status}): {body}"));
}

// After: 401 → typed sentinel; other 4xx still permanent
if status.as_u16() == 401 {
    return Err(anyhow::Error::new(TokenExpiredError));
}
if status.is_client_error() && status.as_u16() != 429 {
    return Err(anyhow::anyhow!("Copilot Chat API error ({status}): {body}"));
}
```

**`agentic_loop` — inline refresh-and-retry**

`api_token` is made `mut`. After the `tokio::select!` call, a second match
intercepts `TokenExpiredError`, exchanges a fresh token, and retries the same
API call once:

```rust
// On token expiry: refresh the API token once and retry the call.  A second
// 401 after a fresh token means a genuine auth failure — surface it as an error.
let api_result = match api_result {
    Err(ref e) if e.is::<TokenExpiredError>() => {
        warn!("API token expired mid-session — refreshing and retrying this round");
        match load_oauth_token() {
            Ok(oauth) => match exchange_token(&oauth).await {
                Ok(new_tok) => {
                    info!("Token refreshed successfully");
                    api_token = new_tok.token;
                    start_chat_stream_with_tools(
                        api_token.clone(), messages.clone(), tool_defs.clone(), &model_id, &tx,
                    )
                    .await
                },
                Err(e) => Err(anyhow::anyhow!("Token refresh failed: {e}")),
            },
            Err(e) => Err(anyhow::anyhow!("Token refresh failed: {e}")),
        }
    },
    other => other,
};
```

Because `api_token` is a local `mut String`, the updated value persists for all
subsequent rounds of the same session — no further refreshes are needed until
the new token also expires (~30 minutes later).

The retry does not re-enter the `tokio::select!` abort check. A `Ctrl+C` that
arrives during the single retry HTTP call will be caught at the top of the next
round's `select!`. This is the correct trade-off: the abort path is designed to
be non-blocking and cooperative, not instantaneous.

---

## Consequences

**Positive**
- Long agentic sessions no longer hard-fail when the API token expires
  mid-run; they refresh transparently and continue.
- The updated token is reused for all remaining rounds — no per-round overhead.
- A second consecutive 401 (genuine auth failure) is still surfaced as a clear
  error message rather than silently retrying.
- Zero new dependencies; `load_oauth_token` and `exchange_token` were already
  free functions.
- `cargo clippy` clean — no new warnings.

**Negative / trade-offs**
- A single retry window (~one HTTP round-trip) exists where a `Ctrl+C` is not
  honoured immediately. This is indistinguishable from any other slow API call
  and is acceptable.
- If the user's OAuth token has itself expired (e.g. never re-authenticated with
  `gh auth login`), `load_oauth_token` or `exchange_token` will fail and the
  error is shown clearly in the panel.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0004](0004-copilot-authentication.md) | Original Copilot OAuth + API token exchange |
| [0011](0011-agentic-tool-calling-loop.md) | `agentic_loop` architecture |
| [0046](0046-agent-retry-visibility.md) | HTTP retry + `StreamEvent::Retrying`; 401 was previously a non-retried permanent error |
