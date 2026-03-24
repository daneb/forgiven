# ADR 0088 — Automatic Tool-Result Compression via LLMLingua

**Date:** 2026-03-24
**Status:** Accepted

---

## Context

ADR 0084 introduced an optional LLMLingua MCP sidecar (`mcp_servers/llmlingua_server.py`)
that exposes a `compress_text` tool the agent can call voluntarily. It was
documented as a manual tool — the agent decides when to invoke it.

The problem with that model is that the agent rarely calls it unprompted. It
has no incentive to: it does not see its own token budget, and asking it to
compress its own tool results would require meta-awareness the current system
prompt does not provide.

Meanwhile, ADR 0087 identified that large tool results (`search_files`,
`read_files`, grep-heavy MCP responses) are the **secondary source** of context
bloat, after the open-file system prompt injection. A 50 KB grep result becomes
part of the conversation history and is re-sent on every subsequent round,
compounding across the session.

LLMLingua is already connected; the infrastructure (`mcp.call_tool()`) is
already in place. The missing piece is a transparent interception layer that
compresses eligible results before they enter the message history — without the
agent needing to ask.

---

## Decision

Add a config flag:

```toml
[agent]
auto_compress_tool_results = true
```

When this flag is set and a connected MCP server exposes a `compress_text`
tool (i.e., LLMLingua is running), the agentic loop transparently compresses
eligible tool results after execution but before they are appended to
`messages`. The model receives the compressed version and never sees the
original bulk.

### Eligibility rules

Two conditions must both be satisfied for compression to fire:

**1. The tool is not code-producing.**

The following tools are unconditionally excluded:

| Tool | Reason |
|------|--------|
| `read_file` | Returns source code — identifiers, operators, indentation must be exact |
| `get_file_outline` | Code signatures used for `edit_file` old_str matching |
| `get_symbol_context` | Code body — same reason |
| `write_file` | Returns a confirmation line ("ok, N lines") — already short |
| `edit_file` | Same |
| `list_directory` | Short by design |
| `create_task` | Short confirmation |
| `complete_task` | Short confirmation |
| `ask_user` | User-facing string — must not be altered |

All other tools — built-in (`search_files`) and MCP tools — are eligible.
This includes grep output, test runner output, web fetch results, documentation
lookups, and any future tools that produce prose or log-like content.

**2. The result exceeds 2 000 characters.**

LLMLingua's BERT model has a warm-up cost on short inputs and the per-call
latency is not worth recovering 20–40 tokens from a 100-token response. The
2 000-char threshold (~500 tokens) ensures only results with meaningful
compression potential are sent through.

### Fallback behaviour

If LLMLingua times out (>10 s), returns an error, or returns an empty string,
the original uncompressed result is used transparently. The agent never
receives an error. The timeout prevents a slow or stuck LLMLingua server from
stalling the agentic loop.

### Logging

Every compression that fires emits one `info!` line:

```
[llmlingua] search_files: 3240t → 812t  (75% reduction)
```

Timeouts and errors are logged as `warn!` (visible in `SPC d` Recent Logs):

```
[llmlingua] compress_text timed out after 10s for search_files — using original
[llmlingua] compress_text returned error for fetch_web: MCP tool error: …
```

---

## Why not compress everything?

### Source code correctness

LLMLingua uses a small BERT model to identify **low-perplexity tokens** —
tokens the model predicts are statistically predictable — and removes them.
For natural language this works well: filler words, repeated phrases, and
verbose preambles are genuinely redundant.

For source code, the same statistical logic fails. In code, tokens that look
predictable to BERT — repeated keywords (`fn`, `let`, `->`, `{`, `}`) —
are syntactically load-bearing. Removing them produces unparseable or
semantically different code. More critically, `edit_file` requires `old_str`
to match the file **verbatim**. If the model sees a compressed version of
`get_symbol_context` output, it will construct `old_str` from that compressed
version, which will not match the file on disk, causing every edit to fail.

The exclusion list is therefore a hard correctness boundary, not a
performance optimisation.

### What gets compressed in practice

The highest-value targets are:

| Tool type | Typical uncompressed | Typical compressed | Notes |
|-----------|---------------------|-------------------|-------|
| `search_files` | 3 000–15 000 t | 600–3 000 t | File:line:text structure is highly repetitive |
| Web fetch (MCP) | 5 000–30 000 t | 1 000–6 000 t | HTML/prose |
| Test output (MCP) | 2 000–10 000 t | 400–2 000 t | Framework boilerplate is low-perplexity |
| Stack traces (MCP) | 1 000–5 000 t | 200–1 000 t | Frame repetition compresses well |

---

## Understanding the metrics

### `[llmlingua]` log line

```
[llmlingua] search_files: 3240t → 812t  (75% reduction)
```

| Field | Meaning |
|-------|---------|
| `search_files` | Tool whose result was compressed |
| `3240t` | Original result size in estimated tokens (`chars / 4`) |
| `812t` | Compressed result size in estimated tokens |
| `75%` | Fraction of tokens removed |

A 75% reduction means the model receives one quarter of the original tokens
from that tool call, with the information preserved through LLMLingua's
low-perplexity removal. Over a session with many `search_files` calls, this
compounds: each compressed result is also smaller in subsequent history
re-sends, multiplying the savings every round.

### Latency cost

LLMLingua runs BERT inference on CPU. Representative timings (M1 MacBook Pro,
llmlingua-2-bert-base):

| Input tokens | Wall time |
|-------------|-----------|
| 500 t | ~80 ms |
| 1 000 t | ~160 ms |
| 3 000 t | ~450 ms |
| 10 000 t | ~1.5 s |

For a session with 10 `search_files` calls averaging 2 000 tokens each,
auto-compression adds ~2.5 seconds of total wall time while potentially
reducing per-round prompt tokens by thousands, which in turn reduces API
latency for the Copilot call itself. The net effect on total session duration
is often zero or negative (faster API calls offset the compression cost).

The 10-second timeout is a hard ceiling: no single compression call can block
the loop for more than 10 seconds regardless of input size or server state.

### Rate parameter

The default `rate = 0.5` retains 50% of tokens. This is the "moderate"
setting from ADR 0084's guidelines. It can be tuned by modifying the
`maybe_compress` helper, or in a future ADR exposed as a config value
(`compression_rate`).

---

## Implementation

### `src/config/mod.rs`

New field on `AgentConfig`:

```rust
#[serde(default)]
pub auto_compress_tool_results: bool,
```

Defaults to `false` — opt-in, not on by default even when LLMLingua is
configured, because of the latency cost.

### `src/agent/mod.rs`

**`COMPRESSION_SKIP_TOOLS` constant** — exhaustive list of tools whose
results must never be compressed.

**`maybe_compress(result, tool_name, mcp)` async function** — checks
eligibility, calls `mcp.call_tool("compress_text", ...)` with a 10-second
timeout, logs the outcome, returns original on any failure path.

**`agentic_loop()`** — new `auto_compress: bool` parameter. In the tool
dispatch loop, after `result` is computed and before it is pushed to
`messages`:

```rust
let result = if auto_compress {
    if let Some(ref mcp) = mcp_manager {
        if mcp.is_mcp_tool("compress_text") {
            maybe_compress(result, &call.name, mcp).await
        } else { result }
    } else { result }
} else { result };
```

**`submit()`** — new `auto_compress: bool` parameter, threaded from the
call site to `agentic_loop`.

### `src/editor/mod.rs`

`panel.submit(...)` call site gains `self.config.agent.auto_compress_tool_results`.

---

## Consequences

**Positive**
- Transparently reduces history size for high-verbosity tool results with no
  agent-side changes — the model does not need to know or care about compression.
- Compounds across rounds: a compressed result is also smaller in the history
  re-sends of subsequent rounds.
- Graceful fallback: a slow or crashed LLMLingua server never breaks the
  agentic loop.
- All compression activity is visible in `SPC d → Recent Logs` via the
  `[llmlingua]` prefix, consistent with the `[ctx]` and `[usage]` audit
  lines from ADR 0087.
- Code correctness is structurally guaranteed by the exclusion list — not by
  heuristics — so there is no risk of corrupted edit contexts.

**Negative / trade-offs**
- Opt-in only: users who would benefit most (heavy sessions hitting context
  limits) need to know the flag exists and set it.
- Adds 80 ms–1.5 s latency per eligible tool call when LLMLingua is running.
  Unacceptable for interactive feel if the model is calling tools in rapid
  succession. The timeout ceiling mitigates the worst case.
- LLMLingua 2 (BERT-base) is a general-purpose model. Domain-specific
  content (Rust compiler errors, test framework output) may compress less
  well than English prose. The 2 000-char minimum threshold skips short
  results but doesn't filter by content type.
- Compression is not reversible: if LLMLingua removes a token the model
  later needs, there is no way to recover it mid-session. In practice this
  affects only non-code content where semantic loss is acceptable.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Compress history during truncation (ADR 0077 phase) | History contains code from prior `read_file` calls; safe/unsafe content is not distinguishable post-hoc |
| Compress `read_file` results for files > N KB | Corrupts `edit_file` old_str matching — hard no |
| Expose compression as a model-visible tool and instruct agent to use it | Agent doesn't call it reliably; adds round-trip latency waiting for the model to decide |
| Use `rate = 0.3` for more aggressive compression | 0.5 is a safer default; aggressive rates on short inputs can remove critical context. Make rate configurable in a future ADR |
| Auto-detect content type (code vs prose) before compressing | Reliable code detection adds complexity; the tool-name exclusion list is a simpler, correct proxy |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0084](0084-llmlingua-mcp-sidecar.md) | LLMLingua MCP sidecar — the server this ADR calls |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Context audit — `[llmlingua]` log lines visible in `SPC d` alongside `[ctx]` and `[usage]` |
| [0077](0077-agent-context-window-management.md) | History truncation — compression reduces what truncation needs to discard |
| [0045](0045-mcp-client.md) | MCP client infrastructure — `mcp.call_tool()` used by `maybe_compress` |
