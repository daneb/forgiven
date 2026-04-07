# ADR 0120 — Auto-Janitor Distinct Streaming UX

**Date:** 2026-04-07
**Status:** Accepted — Implemented

---

## Context

When the Auto-Janitor fires it submits a second API call immediately after the
normal response completes.  Both the normal response and the janitor
summarisation round stream under the same `[Copilot]` (or provider) header.
Because the janitor's summarisation output resembles the original response
(it is summarising it), users perceived the second streaming block as a
duplicate of the first — a sign of two concurrent calls or a response replay.

Observed sequence that caused confusion:

1. User submits `/speckit.analyze` + 3 uploaded files.
2. First `[Copilot]` streams the analysis.
3. Token threshold crossed → `pending_janitor = true`.
4. Next render tick → `Action::AgentJanitorCompress` → second `[Copilot]`
   header appears and streams the janitor summary.
5. On completion, history is replaced with compressed context markers.

Steps 4–5 were invisible to the user as a janitor operation.  The status bar
already showed "Janitor: compressing history…" but this was easy to miss.

## Decision

Two small changes to make the janitor round visually distinct:

### 1 — Swap the streaming header label and colour during janitor

In `src/ui/agent_panel.rs`, the streaming header is now built conditionally on
`panel.janitor_compressing`:

```rust
let (stream_label, stream_color) = if panel.janitor_compressing {
    (format!("╔ 🗜️ Auto-Janitor "), Color::Yellow)
} else {
    // existing provider-aware label + colour
};
```

While the janitor round is in flight, the header reads `╔ 🗜️ Auto-Janitor ▋`
in yellow instead of the provider name in its normal colour.

### 2 — Push a pre-compaction marker into the chat panel

In `src/editor/actions.rs`, the `Action::AgentJanitorCompress` handler pushes a
`Role::System` message into `self.agent_panel.messages` after
`compress_history()` clears the history but before `submit()` is called:

```rust
self.agent_panel.messages.push(ChatMessage {
    role: Role::System,
    content: "🗜️ Auto-Janitor: token budget reached — compressing history…".to_string(),
    images: vec![],
});
```

This message appears in the chat immediately, giving the user an in-band signal
before the second streaming response begins.  It is replaced by the compressed
context markers when the janitor round completes (the Done handler rebuilds
`self.messages` from scratch).

## Files changed

| File | Change |
|------|--------|
| `src/ui/agent_panel.rs` | Branch on `panel.janitor_compressing` for streaming header label and colour |
| `src/editor/actions.rs` | Push pre-compaction `Role::System` chat message; import `ChatMessage`, `Role` |

## Consequences

- The janitor streaming round is immediately recognisable as distinct from a
  normal AI response — different label, different colour.
- An in-chat notification appears before the second streaming block, eliminating
  the "why is Copilot re-answering?" confusion.
- No changes to the agentic loop, `compress_history()`, or `poll_stream()` logic.
- The pre-compaction chat message is ephemeral: it disappears when the janitor
  round completes and the compressed context replaces history.
