# ADR 0117 — Auto-Janitor Fixes: Archive Preservation, Input Save/Restore, Status Variants, Ollama Fallback

**Date:** 2026-04-07
**Status:** Accepted — Implemented

---

## Context

The Auto-Janitor (ADR 0095) fires when `total_session_prompt_tokens` crosses a
configured threshold.  It calls `compress_history()`, which submits a
summarisation prompt to the model, then on completion replaces the live message
history with the summary.  Four bugs were identified in this flow:

1. **Message history was discarded, not archived.**  `compress_history()` called
   `self.messages.clear()` before the summarisation round.  The pre-compression
   messages were then archived in the Done handler — but that handler ran
   *after* the round completed, meaning the full conversation was missing from
   `archived_messages` for the entire duration of the janitor round.  If the
   round failed, the history was simply gone.

2. **User's in-progress typed text was silently destroyed.**  If the janitor
   fired while the user was composing a message, `self.input` was wiped by
   `compress_history()` without saving.  The user would need to retype their
   message after compression completed.

3. **No distinct status variants for the compression phase.**  While the janitor
   ran, `AgentStatus::Streaming` was displayed, which was confusing — the
   status bar showed a round counter that implied the user had submitted a
   normal request.  There was no way to distinguish a compression round from a
   real agent round.

4. **Janitor threshold could never fire on Ollama.**  Ollama does not emit
   `StreamEvent::Usage` events.  The Done handler incremented
   `total_session_prompt_tokens` only when a real usage event arrived, so the
   counter stayed at zero on Ollama and the janitor threshold was never
   reached.  The janitor was effectively disabled for local models.

## Decision

### 1 — Archive before compressing

Move the archival step from the Done handler into `compress_history()`, before
the summarisation round starts:

```rust
// compress_history() — was: self.messages.clear()
self.archived_messages.extend(std::mem::take(&mut self.messages));
```

The Done handler now discards only the janitor round itself (prompt + response),
which is a technical artifact not a real conversation turn:

```rust
// Done handler — was: self.archived_messages.extend(std::mem::take(...))
self.messages.clear(); // janitor round only
```

### 2 — Save and restore in-progress input

`compress_history()` saves whatever the user was typing into a new field
`janitor_saved_input`:

```rust
self.janitor_saved_input = std::mem::take(&mut self.input);
```

The Done handler restores it after compression completes:

```rust
self.input = std::mem::take(&mut self.janitor_saved_input);
```

### 3 — New `AgentStatus` variants

Two new variants are added to `AgentStatus`:

- `Compressing` — displayed while the janitor summarisation round is in flight;
  `status_detail()` returns `"auto-janitor: compressing…"`.
- `JanitorDone` — set when compression completes; returns
  `"auto-janitor: context compressed ✓"`.

`submit()` and the Token handler in `poll_stream()` both branch on
`self.janitor_compressing` to set `Compressing` rather than
`WaitingForResponse`/`Streaming`.

### 4 — Usage fallback for Ollama

A new boolean field `usage_received_this_round` is added to `AgentPanel`.  It
is set to `false` at submit time and to `true` when a `StreamEvent::Usage`
event arrives.

In the Done handler, if `usage_received_this_round` is still `false`, a
character-count estimate is added to `total_session_prompt_tokens` so the
janitor threshold can fire:

```rust
if !self.usage_received_this_round {
    let estimated: u32 = self.messages.iter()
        .map(|m| (m.content.len() / 4 + 4) as u32)
        .sum::<u32>().max(1);
    self.total_session_prompt_tokens =
        self.total_session_prompt_tokens.saturating_add(estimated);
}
self.usage_received_this_round = false;
```

## Files changed

| File | Change |
|------|--------|
| `src/agent/mod.rs` | Added `janitor_saved_input: String`, `usage_received_this_round: bool`, `AgentStatus::Compressing`, `AgentStatus::JanitorDone` |
| `src/agent/panel.rs` | Fixed `compress_history()`, updated Done handler, added usage fallback, status branching in `submit()` and Token handler |

## Consequences

- The user's in-progress message is always preserved across a janitor cycle.
- The pre-compression conversation is always available in `archived_messages`
  (scroll up to see it), even if the janitor round fails mid-flight.
- The status bar correctly identifies compression rounds; no false round counters.
- The janitor threshold fires reliably on Ollama and any other provider that
  omits usage events.
