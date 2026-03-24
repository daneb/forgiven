# ADR 0087 — Context Bloat Audit and Session Token Instrumentation

**Date:** 2026-03-24
**Status:** Accepted

---

## Context

During a live session the context gauge (ADR 0040) showed **804% of the model's
context window** being consumed in a single submission. This is not a rounding
error — the API was receiving a payload roughly 8× the model's advertised limit.
That it was accepted at all suggests the Copilot gateway applies its own hard
cap downstream, silently truncating the tail of the conversation rather than
returning a 400.

ADR 0077 introduced token-aware history truncation with importance scoring, and
it does its job: history is culled correctly. The 804% figure came despite that
fix being live. An investigation identified the true source of bloat.

### The system prompt is the primary offender

Every `submit()` call constructs a system prompt with three components:

```
[preamble]  "You are an agentic coding assistant…"            ~20 tokens
[tree]      build_project_tree(&root, depth=2)               ~200–600 tokens
[rules]     MANDATORY PROTOCOL / FILE EDITING RULES…         ~875 tokens
[ctx]       currently open file — full content               0 – ∞ tokens
```

The first three components are bounded and modest. The fourth is not. When
`src/editor/mod.rs` (~119 KB) is the active buffer, the open-file injection
adds roughly **29,750 tokens** to every single API call — regardless of whether
the user's task has anything to do with that file.

At that scale:

| Model context window | System prompt alone | % of window consumed |
|----------------------|---------------------|----------------------|
| 128 k tokens         | ~31 k tokens        | 24%                  |
| 64 k tokens          | ~31 k tokens        | 48%                  |
| 32 k tokens          | ~31 k tokens        | 97%                  |

History truncation subtracts a `system_tokens` estimate from the budget, but it
cannot compress what has already been placed in the system prompt. The open file
rides along unconditionally.

### Secondary: fallback context window of 128 k

`context_window_size()` returns **128,000** before the `/models` response has
arrived. If the user submits before model metadata loads (e.g. the first message
of a session), the history budget is computed against 128 k even if the selected
model has a 32 k or 64 k actual limit. History truncation is therefore far too
permissive for that first round.

### Tertiary: no visibility

Before this ADR there was no way to tell, at a glance or in logs, how the
context budget was being spent. The gauge showed a percentage of the window but
not the breakdown: how many tokens came from the system prompt, how many from
the open file, how many from history, and what the cumulative cost across a
session was. Without that data, tuning the problem is guesswork.

---

## Decision

### 1. Per-submission context breakdown log (`[ctx]`)

Emit one `info!` line at the start of every `submit()` call, immediately after
the budget is computed:

```
[ctx] window=32000t  sys=30625t (rules≈875t + file≈29750t)  history_msgs=6  budget_for_history=1150t
```

Fields:

| Field | Meaning |
|-------|---------|
| `window` | `context_window_size()` — the model's advertised limit |
| `sys` | Estimated system prompt tokens (`system.len() / 4`) |
| `rules≈` | Tokens from the static rules text alone (no file) |
| `file≈` | Tokens from the injected open-file content (`ctx.len() / 4`) |
| `history_msgs` | Count of non-system messages in the conversation before truncation |
| `budget_for_history` | Tokens remaining for history after system prompt deduction |

When `budget_for_history` is small or negative (clamped to 0 by
`saturating_sub`), the history truncation keeps only `MIN_RECENT = 4` messages
regardless of their size, which is the correct safe fallback.

### 2. Oversized system prompt warning

When the system prompt alone exceeds **50% of the context window**, emit a
`warn!` that names the cause:

```
[ctx] System prompt alone (30625t) exceeds 50% of context window (32000t) —
      the open file (29750t) is the likely cause. Close the file or switch to
      a model with a larger context window.
```

This warning appears in `SPC d → Recent Logs` and in the log file at
`/tmp/forgiven.log`. The 50% threshold is chosen because at that point history
and the user message together have less than 50% of the window — the model is
already operating with severely degraded conversational memory.

### 3. Per-response actual usage log (`[usage]`)

When `StreamEvent::Usage` arrives (API response includes actual token counts),
emit a log line with the real numbers:

```
# Normal case (< 80%)
[usage] prompt=4821t (15% of 32000t window)  completion=312t  cached=2048t  session_total=9043t

# High usage (≥ 80%) — logged as warn! so it surfaces prominently
[usage] prompt=28900t (90% of 32000t window)  completion=184t  session_total=57800t
```

Fields:

| Field | Meaning |
|-------|---------|
| `prompt` | Actual prompt tokens billed for this round (from API) |
| `(N%)` | That count as a percentage of the model's context window |
| `completion` | Tokens generated in this response |
| `cached` | Tokens served from the provider's prompt cache (omitted if 0) |
| `session_total` | Cumulative prompt tokens sent this conversation |

Lines at ≥80% are emitted as `warn!` so they appear highlighted in the
diagnostics overlay's log ring buffer (which captures WARN/ERROR, per ADR 0049).
Lines below 80% are `info!`.

### 4. Session-cumulative token counters on `AgentPanel`

Two new fields accumulate across the conversation:

```rust
pub total_session_prompt_tokens: u32,
pub total_session_completion_tokens: u32,
```

Both are reset to zero by `new_conversation()` (`SPC a n`, ADR 0077), so they
reflect the current session only — not the entire process lifetime.

`saturating_add` is used to avoid overflow on pathologically long sessions
(would require ~4 billion tokens before wrapping, well beyond any practical
limit).

### 5. Agent Session section in `SPC d` diagnostics overlay

A new "Agent Session" section is added to `render_diagnostics_overlay()` when
`total_session_prompt_tokens > 0`. It shows cumulative prompt and completion
tokens for the current conversation, with the prompt total coloured to match
the context gauge convention:

| Colour | Threshold | Interpretation |
|--------|-----------|----------------|
| Green  | < 50%     | Session is within a comfortable range |
| Yellow | 50–79%    | Approaching the window; start a new conversation soon |
| Red    | ≥ 80%     | Session is deep into the context limit; context quality is degrading |

The percentage is computed as `total_session_prompt_tokens / context_window_size()`.
Note this is cumulative across all rounds in the session — it shows how much of
the model's total budget has been consumed by this conversation overall, which
is a more useful indicator of session health than per-round usage alone.

---

## Understanding the metrics

### Why chars/4 for token estimation?

The Copilot API does not expose a tokeniser. The standard approximation for
GPT-family and Claude models is **1 token ≈ 4 characters** for English prose.
For source code the ratio is typically 1:3 to 1:3.5 (code is denser — more
unique tokens per character). Using 4 is therefore a slight underestimate for
code, which means `system_tokens` may be 15–25% lower than the actual billed
count. This is intentional: the 80% budget cap in ADR 0077 absorbs this error.

The `[usage]` actual counts from the API are authoritative and should be used
to calibrate intuition. If you repeatedly see `[ctx] sys≈8000t` but `[usage]
prompt=11000t`, the ~38% gap tells you code-heavy content is tokenising at
closer to 3 chars/token.

### What is `cached` and why does it matter?

The Copilot/OpenAI API supports **prompt caching**: if the prefix of your
prompt matches a cached prefix from a recent request, those tokens are served
from cache and billed at a reduced rate (typically 50% of the normal rate). The
`cached_tokens` field in `StreamEvent::Usage` reports how many tokens were
served from cache.

A high `cached` value is good — it means:
1. Your system prompt prefix is stable enough to be reused (the static rules
   and project tree are good candidates).
2. API cost is lower for that round.

A sudden drop from high cached to zero cached typically means the system prompt
changed (e.g. you opened a different file, changing the `[ctx]` section), which
invalidates the cached prefix.

### What does `session_total` tell you?

`session_total` is the sum of `prompt_tokens` across all rounds in the current
conversation. Because the prompt includes the full conversation history (after
truncation), this grows faster than the per-round count — each round re-sends
the accumulated history.

In a typical session the growth curve looks like:

```
Round 1:   3 000 t   (system + first message)
Round 2:   5 800 t   (system + history + new message)   → session: 8 800 t
Round 3:   8 200 t   (system + growing history)         → session: 17 000 t
Round 4:  10 100 t                                      → session: 27 100 t
```

The session total is not a per-API-call cost figure — it is a diagnostic proxy
for **how much total context has been processed this session**. It answers the
question: "how heavy has this conversation been?"

If `session_total` is growing steeply round-over-round, it typically means the
history is not being truncated (budget is large relative to actual usage) or
that each round is adding a lot of new content (large tool results, pasted code,
read_file calls returning big files). The `[ctx]` log line on the *next*
submission will show whether `budget_for_history` is tightening.

### The 50% system prompt warning

The warning threshold is conservative by design. At 50% the model still has
half the window for history, user messages, and tool results. But it signals
that the open file is the dominant cost and any future growth — longer
conversations, large tool results — will compound quickly.

At 80% or above, the model has almost no room for history. The ADR 0077
truncation will keep only `MIN_RECENT = 4` messages, meaning the model
effectively cannot see anything that happened more than 4 turns ago. Responses
at this point are likely to be lower quality (repeated mistakes, forgetting
earlier decisions).

---

## Implementation

### `src/agent/mod.rs`

**`AgentPanel` struct** — two new fields after `last_cached_tokens`:

```rust
pub total_session_prompt_tokens: u32,
pub total_session_completion_tokens: u32,
```

**`AgentPanel::new()`** — initialise both to `0`.

**`AgentPanel::new_conversation()`** — reset both to `0` on session clear.

**`submit()`** — after computing `budget`, before Phase 1 truncation:

```rust
let ctx_file_tokens = context.as_ref().map(|c| c.len() / 4).unwrap_or(0);
info!(
    "[ctx] window={}t  sys={}t (rules≈{}t + file≈{}t)  history_msgs={}  budget_for_history={}t",
    context_limit, system_tokens,
    (system.len() - context.as_ref().map(|c| c.len()).unwrap_or(0)) / 4,
    ctx_file_tokens,
    self.messages.iter().filter(|m| !matches!(m.role, Role::System)).count(),
    budget,
);
if system_tokens > context_limit / 2 {
    warn!(
        "[ctx] System prompt alone ({system_tokens}t) exceeds 50% of context window \
         ({context_limit}t) — the open file ({ctx_file_tokens}t) is the likely cause."
    );
}
```

**`poll_stream()`** — `StreamEvent::Usage` arm accumulates counters and logs:

```rust
self.total_session_prompt_tokens =
    self.total_session_prompt_tokens.saturating_add(prompt_tokens);
self.total_session_completion_tokens =
    self.total_session_completion_tokens.saturating_add(completion_tokens);
// window computed inline to avoid borrow conflict with stream_rx.as_mut()
let window = /* inline context_window_size() logic */ .max(1);
let pct = prompt_tokens * 100 / window;
// warn! at ≥80%, info! otherwise
```

### `src/ui/mod.rs`

**`DiagnosticsData`** — new field:

```rust
pub agent_session_tokens: Option<(u32, u32, u32)>,  // (prompt_total, completion_total, window)
```

`None` when `total_session_prompt_tokens == 0` (no active session).

**`render_diagnostics_overlay()`** — new "Agent Session" section rendered
before "Recent Logs", coloured using the same thresholds as the context gauge
(ADR 0040).

### `src/editor/mod.rs`

**`DiagnosticsData` construction** in the `Mode::Diagnostics` render path:
populates `agent_session_tokens` from `agent_panel` fields.

---

## Consequences

**Positive**
- Root cause of excessive context usage is now immediately visible in `SPC d`
  and in `/tmp/forgiven.log` — no guesswork about why a session is heavy.
- The `[ctx]` breakdown distinguishes between the open-file cost and history
  cost, making it clear which lever to pull (close the file vs start a new
  conversation vs pick a model with a larger window).
- Warns proactively when a session is on a collision course with the context
  limit, before quality degrades.
- Zero dependency additions. Zero new UI modes. The instrumentation is 25 lines
  across three files, all routed through existing log and diagnostics
  infrastructure.

**Negative / trade-offs**
- Two `info!` log lines per submission and one per response add to the
  `/tmp/forgiven.log` volume. In a long session with many rounds this is
  perhaps 50–100 extra lines. The ring buffer in `SPC d` caps at 50 entries
  (ADR 0049), so older lines naturally drop off.
- Session totals reset on `SPC a n` (new conversation) but not on model switch
  (model switch calls `new_conversation()` internally, so this is correct —
  the reset is consistent).
- `agent_session_tokens` is `None` until the first `StreamEvent::Usage` arrives.
  The "Agent Session" section in `SPC d` is therefore absent for fresh sessions
  with no completed rounds, which is the intended behaviour.

---

## Root cause — not fixed here

This ADR adds observability. It does **not** fix the underlying issue.

The open-file injection (`ctx` in `submit()`) is the architectural source of
bloat. The model already has `read_file`, `get_file_outline`, and
`get_symbol_context` tools available — it does not need the full file pre-loaded
in the system prompt every turn. A future ADR should consider:

- Capping injected file content to the first N lines with a truncation note.
- Only injecting context when the user explicitly attaches the file (Ctrl+P
  already exists for explicit attachment via `file_blocks`).
- Removing `ctx` from the auto-submitted system prompt entirely, relying on the
  model to call `read_file` when it needs context — the same approach used by
  VS Code Copilot Edits and Cursor in agent mode.

Fixing this would reduce the system prompt from ~30 k tokens (large file open)
to ~1 k tokens for most sessions, eliminating the 804% scenario entirely.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Show token breakdown in the panel title or status bar | Panel title is already dense (ADR 0039/0040); status bar clears each render. `SPC d` is the appropriate surface for diagnostic detail |
| Add a `--verbose-tokens` flag / config option | Adds complexity for a low-cost operation; the `info!` lines are cheap and useful by default |
| Track token costs per message in `ChatMessage` | Heavier struct change; the chars/4 estimate at submit time serves the same purpose; actual data comes from API usage events not per-message counts |
| Cap open-file injection in this ADR | Separate concern — this ADR is observability only; a behavioural change to the system prompt deserves its own ADR and decision record |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0040](0040-agent-context-gauge.md) | Context gauge — `last_prompt_tokens`, `context_window_size()`, `StreamEvent::Usage` |
| [0049](0049-diagnostics-overlay.md) | `SPC d` diagnostics overlay and WARN/ERROR log ring buffer |
| [0077](0077-agent-context-window-management.md) | Token-aware history truncation — the mechanism this ADR observes |
