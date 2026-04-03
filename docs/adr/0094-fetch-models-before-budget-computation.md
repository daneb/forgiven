# ADR 0094 — Fetch Models Before Context-Budget Computation

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

`AgentPanel::context_window_size()` returns the token limit for the selected
model, sourced from the Copilot `/models` API. When the model list has not yet
been fetched, it falls back to **128 000 tokens**:

```rust
pub fn context_window_size(&self) -> u32 {
    if self.available_models.is_empty() {
        return 128_000;  // fallback before models load
    }
    self.available_models[…].context_window
}
```

Before this ADR, the call sequence in `submit()` was:

1. Build `user_text`
2. Build `system` prompt
3. Call `context_window_size()` → **may return 128k fallback**
4. Compute `budget = (context_limit * 4/5) - system_tokens`
5. Run history truncation against that budget
6. `await ensure_token()`
7. `fetch_models()` (populates `available_models`)
8. Spawn agentic loop

Steps 3–5 ran *before* step 7. On the first message of any session — the most
important message, when no history exists to truncate — the budget was computed
against 128k even when the user had selected a 32k or 64k model via the config.

This made history truncation far too permissive for the first round. A model
with a 32k context window computed a budget as if it had 102k tokens available
for history, then sent a payload the API silently truncated or rejected.

ADR 0087 noted this as a secondary problem. The fallback is necessary and
correct when the `/models` call itself fails. The bug was only that models were
fetched *after* the budget was computed rather than before.

---

## Decision

Move `ensure_token()` and `fetch_models()` — along with the resulting
`model_id` assignment — to the **top of `submit()`**, immediately after
`user_text` is assembled and before any system-prompt or budget computation.

New call sequence in `submit()`:

1. Build `user_text` (message assembly, slash-command interception)
2. `await ensure_token()` ← **moved early**
3. `fetch_models()` if empty ← **moved early**
4. `let model_id = selected_model_id()` ← **moved early**
5. Build `system` prompt (project tree, tool rules, capped file context)
6. `context_window_size()` → **now returns real limit in all cases**
7. Compute `budget` and run history truncation
8. Assemble API messages, push user message to history
9. Spawn agentic loop (token and model already resolved)

The duplicate `ensure_token()` / `fetch_models()` / `model_id` block that
previously appeared after history truncation is removed.

### Why not fetch models at startup instead?

Model fetching requires an active Copilot API token, which is acquired via an
OAuth exchange that can fail or expire. Fetching at startup would require
either blocking the editor initialisation sequence or adding complex error
recovery. The current lazy approach — fetch on first submit — is correct; the
bug was only about *when within submit* the fetch happens.

### Impact on the 128k fallback

The fallback path is preserved and correct:

- If `ensure_token()` fails → `submit()` returns early with an error; no
  budget is computed; no API call is made.
- If `fetch_models()` fails → `available_models` remains empty;
  `context_window_size()` returns 128k; this is the same behaviour as before
  but now applies only when the `/models` endpoint is genuinely unavailable,
  not on every first message of a session.
- If the user submits before models load on a subsequent session restart →
  models are fetched and populated before `context_window_size()` is called;
  the 128k fallback is no longer reached.

---

## Implementation

### `src/agent/panel.rs`

**Inserted block** after `user_text` assembly, before `root_display`:

```rust
// ── Resolve token + model before computing the context budget ────────
// Fetching models first ensures context_window_size() returns the real
// limit rather than the 128k fallback, so history truncation is correct
// even on the very first message of a session.
let api_token = self.ensure_token().await?;
if self.available_models.is_empty() {
    match fetch_models(&api_token).await {
        Ok(models) if !models.is_empty() => {
            info!("Fetched {} models from Copilot API", models.len());
            self.set_models(models, preferred_model);
        },
        Ok(_) => warn!("Copilot /models returned an empty list"),
        Err(e) => warn!("Could not fetch Copilot model list: {e}"),
    }
}
let model_id = self.selected_model_id().to_string();
self.last_submit_model = model_id.clone();
```

**Removed block** — the previously-existing `ensure_token()` / `fetch_models()`
/ `model_id` block (which appeared after history truncation and user-message
push) is deleted. The `api_token` and `model_id` variables resolved at the top
are used directly when spawning `agentic_loop`.

---

## Consequences

**Positive**
- `context_window_size()` returns the real model limit on every submit,
  including the first message of a new session or after a model switch.
- History truncation is no longer over-permissive for sub-128k models on
  first submit. A 32k model no longer silently allows 100k of history through.
- No additional API calls — `fetch_models()` is still called at most once per
  session (guarded by `if self.available_models.is_empty()`).
- `last_submit_model` is set at the same point as `model_id`, so the metrics
  JSONL (ADR 0092) always records the correct model for every invocation.

**Negative / trade-offs**
- The token exchange (`ensure_token()`) now happens before the system prompt is
  built. If the token exchange fails, the user message is not pushed to history
  and no partial state is left. This is a slight improvement over the old
  behaviour (where the user message was stored before token acquisition).
- Startup of the first `submit()` call has a small additional latency for the
  `fetch_models()` round-trip (typically 200–500 ms). Subsequent submits within
  the same session hit the `!is_empty()` guard and skip the fetch.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Fetch models at editor startup | Requires a valid token before any user interaction; complicates startup sequencing |
| Use the config's `default_copilot_model` to look up a hardcoded context-window table | Brittle — the API is authoritative; a table would go stale as models are updated |
| Block `submit()` and show a spinner until models load | More complex UX; the token exchange already involves a brief wait |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Identified the 128k fallback as a secondary context-bloat source |
| [0077](0077-agent-context-window-management.md) | History truncation — the budget now uses the correct context window on first submit |
| [0040](0040-context-gauge.md) | `context_window_size()` and `last_prompt_tokens` — gauge now reflects real window immediately |
| [0069](0069-model-loading-modernisation.md) | Model loading — `fetch_models()` and `set_models()` are defined here |
