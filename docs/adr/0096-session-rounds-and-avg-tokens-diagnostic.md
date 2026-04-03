# ADR 0096 — Session Rounds Counter and Average Tokens per Invocation

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

The `SPC d` diagnostics overlay (ADR 0049, 0087) shows an **Agent Session**
section with two values:

```
 Agent Session
  prompt total   15284t  (12% of 128000t window)
  completion     1240t
```

`prompt total` is `total_session_prompt_tokens` — the cumulative sum of
`prompt_tokens` from every `StreamEvent::Usage` event in the current
conversation. Because the full conversation history is re-sent to the API on
every round, this field grows **superlinearly**:

```
Round 1: 3 000 t  →  session total:  3 000 t
Round 2: 5 800 t  →  session total:  8 800 t
Round 3: 8 200 t  →  session total: 17 000 t
Round 4:10 100 t  →  session total: 27 100 t
```

The percentage `(12% of 128000t window)` divides this cumulative re-send cost
by the context window, producing a number that looks alarming (12% already in
round 4) but is not directly actionable: it does not tell the user whether
individual rounds are expensive or cheap.

The more useful diagnostic is: **how many tokens does each invocation actually
send?** This is `total_session_prompt_tokens / rounds` — the average prompt
size per round. A high average (e.g., 40 000 t on a 128k model = 31%) means
each round is burning a large share of the window. A low average (e.g., 3 000 t
= 2%) means the session is healthy.

Without a round counter, this average could not be computed.

---

## Decision

### 1. `session_rounds: u32` field on `AgentPanel`

Incremented by 1 each time `StreamEvent::Done` fires in `poll_stream()`.
Reset to 0 by `new_conversation()` (same as the token totals).

### 2. `SPC d` Agent Session section redesigned

Replace the single percentage line with three lines that together give a
complete picture:

```
 Agent Session
  invocations    4
  avg prompt     6775t  (5% of 128000t window)
  session total  27100t prompt  (21% cumulative re-send)
  completion     1240t
```

| Line | Meaning |
|------|---------|
| `invocations` | Number of completed agent invocations since `SPC a n` |
| `avg prompt` | `total_session_prompt_tokens / session_rounds` — the actual per-call cost |
| `(N% of window)` | Average as a fraction of the context window — the health signal |
| `session total` | Cumulative re-send cost, now explicitly labelled as such |
| `(N% cumulative re-send)` | Disambiguates the large-looking percentage |
| `completion` | Cumulative completion tokens (unchanged) |

The colour coding (green / yellow / red at 50% / 80%) is now applied to
**`avg_pct`** (average per-round percentage) rather than the cumulative total.
This means the colour is red when a typical round is consuming 80%+ of the
window — a direct signal that the context is under pressure — rather than when
the accumulated re-send cost crosses an arbitrary threshold.

### 3. `agent_session_tokens` tuple extended to 4 elements

`DiagnosticsData.agent_session_tokens: Option<(u32, u32, u32)>` becomes
`Option<(u32, u32, u32, u32)>` — the fourth element is `session_rounds`.

The field is `None` when `session_rounds == 0` (no completed invocations yet),
replacing the previous `total_session_prompt_tokens > 0` guard.

---

## Implementation

### `src/agent/mod.rs`

New field on `AgentPanel` (after `total_session_completion_tokens`):

```rust
/// Number of completed agent invocations in this conversation session.
/// Incremented on each StreamEvent::Done. Reset by new_conversation().
pub session_rounds: u32,
```

### `src/agent/panel.rs`

**`new()`** — `session_rounds: 0`.

**`new_conversation()`** — `self.session_rounds = 0;`

**`poll_stream()` Done arm** — increment before the metrics write:

```rust
self.session_rounds = self.session_rounds.saturating_add(1);
```

### `src/ui/mod.rs`

```rust
pub agent_session_tokens: Option<(u32, u32, u32, u32)>,
```

### `src/editor/mod.rs`

Guard changed from `total_session_prompt_tokens > 0` to `session_rounds > 0`.
Fourth tuple element added: `self.agent_panel.session_rounds`.

### `src/ui/popups.rs`

`render_diagnostics_overlay()` — destructures the 4-tuple as
`(prompt_total, completion_total, window, rounds)` and renders the four-line
layout described above. `avg_prompt = prompt_total / rounds.max(1)`;
`avg_pct = avg_prompt * 100 / window.max(1)`.

---

## Consequences

**Positive**
- The overlay now answers the actionable question: "is each round expensive?"
  rather than "how much have I sent in total?"
- The "cumulative re-send" label demystifies a number that previously looked
  alarming (12% after 4 rounds) but was not directly comparable to anything.
- The colour signal (green/yellow/red) is now tied to per-round health rather
  than the cumulative total, making it meaningful from the first invocation.
- `session_rounds` is also useful context for the JSONL metrics log (ADR 0092):
  combining the per-invocation `prompt_tokens` field with the round number
  makes growth curves easy to chart.

**Negative / trade-offs**
- The Agent Session section is now 4 lines instead of 2, making the `SPC d`
  overlay slightly taller. The popup auto-sizes to content (ADR 0049), so this
  has no layout impact unless the terminal is very short.
- `avg_prompt` uses integer division. In a 1-round session `avg_prompt ==
  prompt_total` (exact). The rounding error grows negligible as rounds
  accumulate.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Show per-round token history (sparkline or list) | Too much space in the overlay; the JSONL log is the right surface for per-round history |
| Remove the cumulative total entirely | It is still a useful "how heavy has this session been?" signal; keeping it but labelling it correctly is better |
| Use median instead of mean | Requires storing per-round values; mean is sufficient and requires only the existing cumulative field |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0049](0049-diagnostics-overlay.md) | `SPC d` overlay — Agent Session section redesigned here |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Introduced `total_session_prompt_tokens` and the Agent Session section |
| [0092](0092-persistent-session-metrics-jsonl.md) | JSONL metrics — `session_rounds` provides round-number context for per-invocation records |
| [0040](0040-context-gauge.md) | Context gauge — per-round `last_prompt_tokens` is the same data averaged here |
