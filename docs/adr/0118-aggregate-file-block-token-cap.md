# ADR 0118 — Aggregate File-Block Token Cap

**Date:** 2026-04-07
**Status:** Accepted — Implemented

---

## Context

Files attached via the Ctrl+P picker are each truncated to `AT_PICKER_MAX_LINES`
(500 lines) before being stored as `file_blocks` on `AgentPanel`.  On `submit()`,
all file blocks are concatenated into the user message with no further limit.

With 30 attached files at the per-file cap, the user message alone can reach
~187,500 tokens.  Adding the system prompt (~5–10 K tokens) and history pushes
the total well above the 128 K Copilot Enterprise limit, producing:

```
Copilot API error (400 Bad Request):
{"error":{"message":"prompt token count of 391491 exceeds the limit of 128000",
 "code":"model_max_prompt_tokens_exceeded"}}
```

The existing `MAX_CTX_LINES = 150` cap (ADR 0087) applies only to the
currently-open editor buffer injected into the *system prompt*.  File blocks
added by the picker bypass all token-budget enforcement because they are
assembled into the current user message, which is not subject to the history
truncation algorithm.

## Decision

Enforce an **aggregate file-block token budget** before assembling `user_text`
in `submit()`.

### Budget

`max_file_tokens = context_window_size() / 2`

Using 50% of the model's actual context window (dynamic, not a hard constant)
leaves the remaining half for the system prompt, conversation history, and the
user's typed instruction.  `context_window_size()` is resolved at the top of
`submit()` — before the file assembly loop — so the budget adjusts automatically
when the user switches to a model with a larger or smaller context window.

### File-loop logic

Files are iterated in picker order (most recently added first).  For each file:

1. **Fits entirely** → included unchanged.
2. **Partially fits** → content is truncated at the last line boundary that fits
   within the remaining budget; a note is appended telling the model to call
   `read_file` for the rest.
3. **Budget exhausted** → a one-line stub replaces the file content, naming the
   file and directing the model to use `read_file`.

A `warn!` log is emitted when the budget is exhausted; it is visible in the
`SPC d` Recent Logs panel.

```rust
let max_file_tokens: usize = (context_limit as usize) / 2;
let mut used_file_tokens: usize = 0;
for (name, content, _) in &files {
    // … fit / truncate / stub logic …
}
if used_file_tokens >= max_file_tokens {
    warn!("[ctx] File block budget ({max_file_tokens}t) exhausted — …");
}
```

### Token approximation

The same `len / 4` (characters per token) approximation used by the history
truncation algorithm is used here, keeping the two budgets consistent.

## Files changed

| File | Change |
|------|--------|
| `src/agent/panel.rs` | `context_window_size()` moved to top of `submit()`; bare file-block loop replaced with budget-aware version; duplicate `context_limit` assignment removed |

## Consequences

- Submitting many files never produces a `model_max_prompt_tokens_exceeded`
  API error; the prompt is always within the model's context window.
- Files beyond the budget are not silently dropped — the model receives a stub
  entry and knows to call `read_file` for the full content.
- Normal usage (2–3 small files) is unaffected; all content passes through.
- The per-file `AT_PICKER_MAX_LINES = 500` cap is unchanged; both caps apply.
- The budget is model-aware: a model with a 200 K window gets a 100 K file
  budget without any config change.
