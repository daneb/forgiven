# ADR 0093 — Cap Open-File Context Injection to 150 Lines

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

ADR 0087 identified that injecting the full content of the active buffer into
the system prompt on every `submit()` call is the primary driver of token
consumption. For `src/editor/mod.rs` (~119 KB), this single injection adds
roughly **30,000 tokens** to every API call — regardless of whether the user's
task has anything to do with that file.

ADR 0087 deliberately deferred fixing this, noting three candidate approaches:

1. Cap injected content to the first N lines with a truncation note.
2. Only inject when the user explicitly attaches the file (Ctrl+P).
3. Remove auto-injection entirely, relying on the model to call `read_file`.

ADR 0092 added a persistent JSONL metrics log, making the cost immediately
visible. The first session after that change confirmed the 30k-token figure.

The model already has `read_file`, `get_file_outline`, and `get_symbol_context`
tools. It does not need the full file pre-loaded on every round.

---

## Decision

Cap the injected context to the first **150 lines** (`MAX_CTX_LINES = 150`).

When the active buffer exceeds 150 lines, the system prompt receives only the
first 150 lines, followed by a truncation note directing the model to use
`read_file`:

```
[Showing first 150 of 3241 lines — call read_file for the full content]
```

Files under 150 lines are injected unchanged — the behaviour is identical to
before for small utility files, config files, and scripts.

### Why 150 lines and not full removal?

Full removal (option 3) is the cleanest long-term solution and matches VS Code
Copilot Edits / Cursor agent mode. However, it changes the model's first-turn
behaviour: for small files the model would need an extra tool call before it
could act, adding latency. The 150-line cap preserves the zero-latency fast path
for small files while eliminating the bloat for large ones.

Option 2 (explicit attach only) would break the existing UX where the model
can answer questions about the open file without the user needing to Ctrl+P it.

150 lines is chosen because:
- It covers the vast majority of small utility files, config files, and scripts
  completely (no truncation at all).
- For a 3 000-line file it reduces injection from ~75 000 chars (~18 750 tokens)
  to ~6 000 chars (~1 500 tokens) — a 92% reduction.
- It is large enough for the model to orient itself (see the top-of-file
  imports, module declarations, or class definitions) before calling a tool.

### Token impact

| File size | Before | After | Reduction |
|-----------|--------|-------|-----------|
| 50 lines (small) | ~600 t | ~600 t | 0% (no change) |
| 300 lines (medium) | ~3 750 t | ~1 875 t | 50% |
| 3 000 lines (large) | ~37 500 t | ~1 875 t | 95% |
| `src/editor/mod.rs` (119 KB) | ~29 750 t | ~400 t | 98.7% |

The system prompt overhead for a large-file session drops from being the
**dominant cost** to a minor overhead indistinguishable from the rules text.

---

## Implementation

### `src/agent/panel.rs`

**Constant** (top of `submit()` body):

```rust
const MAX_CTX_LINES: usize = 150;
```

**Context capping** (before `build_project_tree`):

```rust
let ctx_total_lines = context.as_ref().map(|c| c.lines().count()).unwrap_or(0);
let context_snippet: Option<String> = context.as_ref().map(|raw| {
    if ctx_total_lines > MAX_CTX_LINES {
        raw.lines().take(MAX_CTX_LINES).collect::<Vec<_>>().join("\n")
    } else {
        raw.clone()
    }
});
```

**System prompt** — now uses `context_snippet` instead of `context`. When
truncated, a `truncation_note` is appended inside the fenced block:

```rust
let truncation_note = if ctx_total_lines > MAX_CTX_LINES {
    format!("\n[Showing first {MAX_CTX_LINES} of {ctx_total_lines} lines — \
             call read_file for the full content]")
} else {
    String::new()
};
```

**`[ctx]` audit log** — `ctx_file_tokens` and the `rules≈` estimate are
updated to use `context_snippet` so the log reflects the actual injected size.
When the file was truncated a `[150/3241lines]` annotation is appended:

```
[ctx] window=128000t  sys=2275t (rules≈875t + file≈1400t [150/3241lines])  history_msgs=6  budget_for_history=100125t
```

**`last_submit_ctx`** — `sys_tokens` is computed from `system.len() / 4`
after the system prompt is built using `context_snippet`, so it automatically
reflects the capped size.

---

## Consequences

**Positive**
- The primary source of context bloat is eliminated for large files. A session
  with `src/editor/mod.rs` open no longer has 24% of a 128k window consumed
  before the first message is sent.
- No behaviour change for files under 150 lines.
- The `[ctx]` audit log now shows a `[N/M lines]` annotation whenever truncation
  fires, making it obvious in `SPC d → Recent Logs` and in
  `~/.local/share/forgiven/forgiven.log`.
- The truncation note inside the system prompt (`call read_file for the full
  content`) gives the model explicit direction — it will not attempt to work
  from incomplete content.

**Negative / trade-offs**
- For files between 151 and ~500 lines the model receives a partial view. On the
  first turn involving mid-file code, it may call `read_file` to get the full
  content, adding one tool-call round. This is the expected trade-off: one extra
  latency hit per relevant session vs tens-of-thousands of tokens burned on every
  irrelevant round.
- 150 is a hardcoded constant. A future ADR could expose it as a config key
  (`[agent] max_context_lines = 150`) or allow per-language overrides. Deferred
  to avoid speculative configurability (single concrete value is the right
  default for now).

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Remove auto-injection entirely | Breaks zero-latency fast path for small files; larger behaviour change deserving its own ADR |
| Only inject on explicit Ctrl+P attach | Changes existing UX; open-file context is genuinely useful for small files |
| Cap by character count instead of line count | Line count is easier to reason about and aligns with editor conventions |
| Cap at 50 lines | Too aggressive — misses top-of-file structure for medium files |
| Cap at 500 lines | Too conservative — still sends ~12 500 tokens for a 500-line Rust file |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Identified the open-file injection as the root cause; explicitly deferred this fix |
| [0092](0092-persistent-session-metrics-jsonl.md) | JSONL log makes the token reduction immediately measurable |
| [0077](0077-agent-context-window-management.md) | History truncation — `sys_tokens` now accurately reflects the capped injection cost |
