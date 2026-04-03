# ADR 0101 — Auto-Janitor: Rolling History Compression (Phase 3)

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

ADR 0099 (Phase 1) made token consumption visible per segment. ADR 0100 (Phase 2)
introduced the Spec Slicer, which surgically reduces spec injection costs.

The remaining dominant cost driver is **chat history re-send**: on every `submit()`
the full conversation is re-serialised and sent to the API. After ~5–10 rounds on
a non-trivial task, `total_session_prompt_tokens` exceeds 10 000 and accelerates
with each round.

`docs/context-optimization-plan.md` Phase 3 specifies:

1. A `MemoryJanitor` struct that monitors cumulative prompt tokens.
2. A manual "Summarise & Clear" keybind as the first trigger.
3. An automated trigger when session tokens exceed a configurable threshold
   (default: 10 000).
4. Ephemeral vs. persistent message tagging (deferred — see Consequences).

The goal is to prevent unbounded history growth without losing the technical
context accumulated in a session.

---

## Decision

### 1. `compress_history()` on `AgentPanel`

A new method serialises the current non-separator conversation into a single
summarisation prompt:

```
Summarise the technical decisions, key findings, and important context from
the conversation below into a concise bulleted list. Discard chit-chat and
completed throwaway tasks. Focus on what would be expensive to re-discover.

<conversation>
**User:** …

---

**Assistant:** …
</conversation>
```

Only `Role::User` and `Role::Assistant` messages are included; system separator
lines (e.g., `── New conversation · … ──`) are stripped.

`self.messages` and `self.tasks` are cleared before `submit()` is called so the
outgoing API request carries **no prior history** — just the summarisation prompt.
This is intentional: the janitor round is cheap (one bare user message, no
context).

### 2. `janitor_compressing: bool` flag

`AgentPanel` gains two new fields:

| Field | Type | Purpose |
|---|---|---|
| `janitor_compressing` | `bool` | Marks the in-flight round as a janitor summarisation; read in `poll_stream()` on `Done`. |
| `pending_janitor` | `bool` | Set by `poll_stream()` when the threshold is exceeded; read by the editor tick loop to fire auto-compression. |

`compress_history()` sets `janitor_compressing = true`. If `history_text` is
empty (nothing to compress), the method returns early without touching state.

### 3. `poll_stream()` Done handler

When `StreamEvent::Done` arrives and `janitor_compressing` is true, the handler:

1. Extracts the summary text from the last (Assistant) message.
2. Calls `self.messages.clear()` and resets `total_session_prompt_tokens`,
   `total_session_completion_tokens`, and `session_rounds` to zero.
3. Pushes a System separator: `── Context compressed by Auto-Janitor ──`.
4. Pushes a System message: `**Session summary (Auto-Janitor):**\n\n{summary}`.
5. Skips the metrics `append_session_metric()` call (counters were just zeroed).
6. `break`s out of the poll loop normally.

The reset+reinject pattern means the next `submit()` call will see a small,
accurate history (one summary message) instead of a full conversation.

### 4. Auto-threshold trigger

`poll_stream` is extended from `fn poll_stream(&mut self) -> bool` to
`fn poll_stream(&mut self, janitor_threshold: u32) -> bool`.

When a normal (non-janitor) `Done` arrives and
`total_session_prompt_tokens >= janitor_threshold` (and `janitor_threshold > 0`),
the method sets `pending_janitor = true`.

In `editor/mod.rs`, the tick loop reads this flag immediately after
`poll_stream()` returns and fires `execute_action(Action::AgentJanitorCompress)`:

```rust
let agent_active =
    self.agent_panel.poll_stream(self.config.agent.janitor_threshold_tokens);
// …
if self.agent_panel.pending_janitor {
    self.agent_panel.pending_janitor = false;
    let _ = self.execute_action(Action::AgentJanitorCompress);
}
```

Setting threshold to `0` disables auto-trigger entirely; `SPC a j` still works.

### 5. `Action::AgentJanitorCompress` and `SPC a j` keybind

| Key | Action | Description |
|---|---|---|
| `SPC a j` | `AgentJanitorCompress` | Compress history (janitor) |

The action handler in `editor/actions.rs`:

1. Calls `agent_panel.compress_history()`.
2. If `input` is empty after the call, sets status "Janitor: nothing to compress"
   and returns.
3. Otherwise opens the agent panel, sets status "Janitor: compressing history…",
   and calls `submit()` with `max_rounds = 1`, `warning_threshold = 0`, and the
   configured cheap model (see §6).

`max_rounds = 1` prevents the agentic loop from spinning tool rounds for what is
a pure text-generation call.

### 6. Config: `janitor_threshold_tokens` and `janitor_model`

Two fields added to `AgentConfig`:

```toml
[agent]
# Tokens (cumulative, re-send cost) that trigger auto-compression.
# Set to 0 to disable. Default: 10000.
janitor_threshold_tokens = 10000

# Model for the cheap summarisation call. Falls back to active model if empty.
# Example: "claude-haiku-4-5-20251001"
janitor_model = ""
```

`janitor_model` defaults to `""` (falls back to `active_default_model()`). When
a cheap model is set, only the janitor round uses it; normal submits are
unaffected.

---

## Implementation

### Files changed

| File | Change |
|---|---|
| `src/keymap/mod.rs` | `Action::AgentJanitorCompress` variant; `SPC a j` leaf in `build_leader_tree()` |
| `src/config/mod.rs` | `janitor_threshold_tokens: u32` and `janitor_model: String` on `AgentConfig`; `default_janitor_threshold()` returns 10 000 |
| `src/agent/mod.rs` | `janitor_compressing: bool` and `pending_janitor: bool` on `AgentPanel` |
| `src/agent/panel.rs` | `compress_history()` method; janitor Done-handler branch; threshold check in Done; `poll_stream` signature extended |
| `src/editor/actions.rs` | `Action::AgentJanitorCompress` arm in `execute_action()` |
| `src/editor/mod.rs` | `poll_stream(janitor_threshold_tokens)` call; `pending_janitor` auto-trigger check; `Action` import |

---

## Consequences

**Positive**
- History cost resets to near-zero after compression; subsequent rounds are fast
  and cheap regardless of how long the session ran before.
- Manual `SPC a j` gives the user explicit control with immediate feedback;
  automated trigger requires no intervention once the threshold is hit.
- The cheap-model override (e.g. Haiku) makes the summarisation call
  significantly less expensive than a full Sonnet round.
- The summary is visible in the chat panel as a system message, so the user can
  read and verify what was retained before continuing.
- `max_rounds = 1` prevents the janitor from entering tool-calling loops.

**Negative / trade-offs**
- **Quality risk:** The summary is only as good as the cheap model's compression
  fidelity. Subtle implementation constraints or error patterns may be dropped.
  Phase 3, task 2 intentionally adds the manual keybind first so users can
  validate quality before relying on auto-trigger.
- **One missed round:** The threshold check fires *after* the round that crosses
  it, so the session may exceed the threshold by one round's tokens before
  compression runs. This is acceptable — the check is a soft ceiling, not a hard
  cap.
- **Ephemeral/persistent tagging deferred:** Plan.md Phase 3 task 4 (auto-append
  architectural decisions to `plan.md`) is not implemented. File mutation from
  within the agent's session loop is rated "medium risk" in the complexity table;
  it is tracked as a follow-on to this ADR rather than bundled here.

---

## Alternatives considered

| Alternative | Rejected because |
|---|---|
| Truncate history in-place (keep last N turns, discard older) | Loses context silently with no summary; user has no visibility into what was dropped |
| Spawn a separate `tokio::task` for the summarisation API call | Adds coordination complexity (channel setup, task handle management) for no gain over the existing `submit()` path |
| Summarise in a background thread before threshold is hit (speculative) | Premature and potentially wasteful; adds state complexity; the reactive approach is simpler |
| Run janitor as a tool inside the agentic loop | Circular — the agentic loop *is* the problem; a separate bare `submit()` with `max_rounds = 1` is cleaner |
| Reset `session_rounds` only, not token counters | `total_session_prompt_tokens` would misrepresent the effective session cost post-compression, breaking the threshold check on the next cycle |

---

## Related ADRs

| ADR | Relation |
|---|---|
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Original context bloat audit — identified history re-send as a primary cost driver |
| [0099](0099-context-breakdown-token-awareness.md) | Phase 1: per-segment token visibility that informs the threshold |
| [0100](0100-spec-slicer-virtual-context.md) | Phase 2: Spec Slicer — reduces spec injection; Phase 3 addresses the orthogonal history cost |
| [0077](0077-agent-context-window-management.md) | Importance-scored history truncation — the janitor complements (doesn't replace) this mechanism |
| [0083](0083-mcp-memory-server.md) | MCP memory server — `SPC a s` persists context to the knowledge graph; janitor compresses within the session |
