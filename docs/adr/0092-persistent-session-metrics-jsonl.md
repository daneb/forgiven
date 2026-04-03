# ADR 0092 — Persistent Session Metrics JSONL Log

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

ADR 0087 added structured `[ctx]` and `[usage]` log lines to `/tmp/forgiven.log`.
Those lines provide per-submission and per-response token breakdowns during a
running session. Two problems remain:

1. **Ephemeral storage.** `/tmp/forgiven.log` is created with `File::create` on
   every startup, wiping the previous session's data. Historical token usage
   patterns — which models burn the most, which sessions were heaviest, how the
   system-prompt cost changes with different active files — are unanalyzable.

2. **Unstructured for tooling.** The `[ctx]` and `[usage]` log lines are
   human-readable but require custom parsing to extract fields. A `jq` query or
   a short Python script can answer questions like "what was my average
   `sys_tokens` last week?" only if the data is in a machine-readable format.

The root cause of excessive token consumption (ADR 0087: open-file system-prompt
injection) remains unfixed. Until that is addressed, users need visibility into
their own usage history so they can understand patterns, correlate heavy sessions
with specific files or models, and make informed decisions about when to start
fresh conversations.

---

## Decision

Append one JSON line to `~/.local/share/forgiven/sessions.jsonl` at the end of
every agent invocation (when `StreamEvent::Done` fires and at least one
`StreamEvent::Usage` has been received for that invocation).

### File path

`$XDG_DATA_HOME/forgiven/sessions.jsonl`, falling back to
`$HOME/.local/share/forgiven/sessions.jsonl` when `XDG_DATA_HOME` is not set.

The directory is created automatically on first write. I/O errors are swallowed
silently so a permissions problem never interrupts the agentic loop.

### Record format

One compact JSON object per line (JSONL):

```json
{"ts":1743682496,"model":"claude-sonnet-4","prompt_tokens":4821,"completion_tokens":312,"cached_tokens":2048,"ctx_window":128000,"sys_tokens":4200,"budget_for_history":98400,"session_prompt_total":14063,"session_completion_total":890,"pct":4}
```

| Field | Source | Meaning |
|-------|--------|---------|
| `ts` | `SystemTime::now()` as Unix seconds | When the invocation completed |
| `model` | `selected_model_id()` at submit time | Model that handled this invocation |
| `prompt_tokens` | `StreamEvent::Usage.prompt_tokens` | Actual billed prompt tokens for this invocation |
| `completion_tokens` | `StreamEvent::Usage.completion_tokens` | Tokens generated |
| `cached_tokens` | `StreamEvent::Usage.cached_tokens` | Tokens served from prompt cache |
| `ctx_window` | `context_window_size()` at submit time | Model's advertised context limit |
| `sys_tokens` | `system.len() / 4` at submit time | Estimated system-prompt tokens (includes open-file injection) |
| `budget_for_history` | `(ctx_window * 4/5) − sys_tokens` | Tokens available for conversation history after system prompt |
| `session_prompt_total` | cumulative `total_session_prompt_tokens` | Running prompt total for the current conversation |
| `session_completion_total` | cumulative `total_session_completion_tokens` | Running completion total |
| `pct` | `prompt_tokens * 100 / ctx_window` | Prompt tokens as % of the context window for this invocation |

### What this enables

**Ad-hoc analysis with standard tools:**

```bash
# Last 20 invocations — model, prompt tokens, pct of window
tail -20 ~/.local/share/forgiven/sessions.jsonl | jq -r '[.ts,.model,.prompt_tokens,.pct] | @tsv'

# Average sys_tokens by model (open-file injection cost)
jq -s 'group_by(.model) | map({model: .[0].model, avg_sys: (map(.sys_tokens) | add / length)})' \
   ~/.local/share/forgiven/sessions.jsonl

# Sessions where prompt exceeded 60% of the context window
jq 'select(.pct >= 60)' ~/.local/share/forgiven/sessions.jsonl

# Daily prompt token spend
jq -s 'group_by((.ts / 86400 | floor)) | map({day: .[0].ts, total_prompt: map(.prompt_tokens) | add})' \
   ~/.local/share/forgiven/sessions.jsonl
```

---

## Implementation

### `src/agent/mod.rs`

**`SubmitCtx` struct** — context-budget snapshot captured at `submit()` time:

```rust
#[derive(Debug, Clone, Copy)]
pub struct SubmitCtx {
    pub ctx_window: u32,
    pub sys_tokens: u32,
    pub budget_for_history: u32,
}
```

**`metrics_data_path()`** — resolves the JSONL file path (XDG-aware).

**`append_session_metric(record: &serde_json::Value)`** — creates the directory
on first call, appends `record.to_string() + "\n"`, swallows errors.

**`AgentPanel` struct** — two new fields:

```rust
pub last_submit_ctx: Option<SubmitCtx>,
pub last_submit_model: String,
```

Both initialised to `None`/`""` in `new()`. `last_submit_model` is set to the
empty string in `new_conversation()` via the existing zero-initialisation path.

### `src/agent/panel.rs`

**`new()`** — initialise `last_submit_ctx: None, last_submit_model: String::new()`.

**`submit()`** — immediately after computing `budget`:

```rust
self.last_submit_ctx = Some(SubmitCtx {
    ctx_window: context_limit,
    sys_tokens: system_tokens,
    budget_for_history: budget,
});
```

And after `model_id` is computed:

```rust
self.last_submit_model = model_id.clone();
```

**`poll_stream()`** — in the `StreamEvent::Done` arm, before clearing state:

```rust
if self.last_prompt_tokens > 0 {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)…as_secs();
    let (ctx_window, sys_tokens, budget) = self.last_submit_ctx.map(…).unwrap_or(…);
    let pct = self.last_prompt_tokens * 100 / ctx_window.max(1);
    append_session_metric(&serde_json::json!({ … }));
}
```

The `last_prompt_tokens > 0` guard skips the write when the invocation produced
no API response (e.g. aborted before streaming started).

---

## Consequences

**Positive**
- Persistent, machine-readable history of every agent invocation.
- Zero new dependencies: `serde_json` is already in `Cargo.toml`; `SystemTime`
  is `std`.
- XDG-compliant path mirrors the config convention from `src/config/mod.rs`.
- File grows at ~180 bytes/line. A heavy user doing 50 invocations/day produces
  ~3 MB/year — negligible. No rotation is needed.
- Silently swallows I/O errors so a write failure never impacts the editor.

**Negative / trade-offs**
- `last_prompt_tokens` reflects the last `StreamEvent::Usage` before `Done`.
  For multi-round invocations (agent called tools across several rounds), only
  the final round's token count is captured — not the per-round breakdown.
  The `session_prompt_total` field compensates: it is the cumulative sum across
  all rounds in the conversation, giving a session-level cost signal.
- `sys_tokens` is the chars/4 estimate, not the actual billed count. As noted
  in ADR 0087, code-heavy content tokenises at closer to 3 chars/token, so
  `sys_tokens` may underestimate by 15–25%. Use `prompt_tokens` (actual) as the
  authoritative cost figure.
- The file is not rotated or capped. Users who want to prune it can
  `tail -n 1000 ~/.local/share/forgiven/sessions.jsonl > /tmp/s && mv /tmp/s ~/.local/share/forgiven/sessions.jsonl`.

---

## Root cause — still not fixed

This ADR adds measurement. The primary source of token consumption — unconditional
open-file injection into the system prompt (ADR 0087) — remains unfixed.
A future ADR should cap the injected content or remove the auto-injection
entirely. The metrics added here will make the impact of that change immediately
visible: `sys_tokens` will drop from ~30 000 to ~1 000 for large-file sessions.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Context audit — adds `[ctx]`/`[usage]` log lines; this ADR persists equivalent data to a durable file |
| [0040](0040-context-gauge.md) | `last_prompt_tokens`, `context_window_size()` — the same fields written here |
| [0077](0077-agent-context-window-management.md) | History truncation — `budget_for_history` is the truncation budget |
