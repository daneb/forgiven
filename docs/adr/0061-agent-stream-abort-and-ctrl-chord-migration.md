# ADR 0061 — Agent Stream Abort and Ctrl-Chord Keybinding Migration

**Date:** 2026-03-13
**Status:** Accepted

---

## Context

Three problems were identified with the Agent panel's keyboard interface.

### 1. No way to abort a running LLM request

Pressing `Esc` while the agent was streaming only returned focus to Normal
mode; the underlying `tokio` task continued running until the model finished.
For long-running agentic loops (multi-tool calls, large file operations) there
was no way to cancel a response mid-flight.

### 2. Bare single-letter shortcuts intercepted typing

Three Agent mode shortcuts fired when the input box was empty:

| Key | Action |
|-----|--------|
| `a` | Open apply-diff overlay (`Mode::ApplyDiff`) |
| `c` | Copy first code block from last reply |
| `y` | Yank (copy) full last reply |

Because the guard was `input.is_empty()`, pressing any of these letters as the
**first character of a new message** triggered the shortcut instead of typing.
Attempts to begin a message with "add a test", "can you explain", or "yes
please" would silently fire the wrong action.

### 3. Tool-call result formatting bugs

Two rendering bugs affected how agentic tool calls appeared in the chat panel:

- **Paragraph break** — the separator token emitted between a tool-call block
  and the following LLM response was `"\n"` (a CommonMark *soft* break).
  This merged the response text into the `⚙` tool-call paragraph, causing it
  to render in dim gray instead of normal white.
- **Result summary noise** — the `result_summary` extraction rejected the
  clean `"README.md (10 lines)"` header (because it contained `(`), fell back
  to the first content line `"   1 | # Papers…"`, and used byte-based
  truncation that panics on multi-byte characters (e.g. `—`).

---

## Decision

### Stream abort (`Ctrl+C`)

A cancellation channel (`tokio::sync::oneshot`) is created in
`AgentPanel::submit()` and the sender half stored as
`abort_tx: Option<oneshot::Sender<()>>` on `AgentPanel`.

A new `cancel_stream()` method:

1. Drops `abort_tx`, which fires the oneshot receiver inside the agentic task.
2. If there is a partial streaming reply, appends `"\n\n*⏹ Stopped*"` and
   commits it to the message history so the interruption is visible.
3. Clears `stream_rx`, `continuation_tx`, `question_tx`, and resets
   `AgentStatus` to `Idle`.

Inside `agentic_loop`, both blocking await points are wrapped in
`tokio::select!` so cancellation is handled immediately without waiting for
the next network event:

```rust
let response = tokio::select! {
    _ = &mut abort_rx => { let _ = tx.send(StreamEvent::Done); return; }
    res = start_chat_stream_with_tools(...) => match res { ... }
};
```

The `ask_user` question-receive is wrapped identically so a pending tool
dialog can also be aborted.

`Ctrl+C` is handled in the Agent-mode key branch in `src/editor/mod.rs`.

### Bare-key → Ctrl-chord migration

All three empty-input bare-key shortcuts are removed. Replacement chords are
always active in Agent mode (no empty-input guard):

| Old key (empty input only) | New chord | Action |
|----------------------------|-----------|--------|
| `a` | `Ctrl+A` | Open apply-diff overlay |
| `c` | `Ctrl+K` | Copy / cycle code blocks |
| `y` | `Ctrl+Y` | Yank full reply |

The key handler falls through to `agent_panel.input_char(ch)` for all
remaining characters, so every letter is freely typeable as the first
character of a message.

ADRs 0035 and 0041 have been amended to reflect these key changes.

### Tool-call formatting fixes

**Paragraph break** — the round-separator emits `StreamEvent::Token("\n\n")`
so that the following LLM response is a new CommonMark paragraph and renders
as normal (white) text rather than merging into the dim-gray `⚙` line.

**Result summary** — the extraction logic is updated to:

- Accept lines whose only `(` occurrence is part of `" lines)"` (the
  `read_file` header format, e.g. `"README.md (10 lines)"`).
- Strip the `N | ` line-number prefix produced by the `read_file` tool.
- Truncate using `chars().take(120)` instead of a byte-index slice to avoid
  panics on multi-byte characters.

---

## Alternatives considered

**`Esc` to abort**
`Esc` already returns focus to Normal mode. Doubling it as an abort key would
silently kill the task whenever a user glanced away from the panel. A dedicated
chord is unambiguous.

**Silent drop (no "Stopped" marker)**
Dropping the partial reply silently would leave the panel looking as though
the model never responded. Appending `*⏹ Stopped*` makes the interruption
visible in the chat history.

**`Shift+letter` chords for `c` / `y` / `a`**
Uppercase letters appear in ordinary messages. `Ctrl` chords are the standard
terminal convention for editor commands and have no collision with message
content.

---

## Consequences

**Positive**
- In-flight LLM requests (including multi-round tool loops) can be cancelled
  instantly with a single chord.
- Partial replies are preserved in the chat history; the user can see what was
  generated before stopping.
- All letters (`a`–`z`, `A`–`Z`) can now be freely used as the first
  character of any Agent panel message.
- LLM response text following a tool call is no longer rendered as dim gray.
- `read_file` result summaries show the filename and line count rather than
  the first content line.

**Negative / trade-offs**
- `abort_tx: Option<oneshot::Sender<()>>` adds a small field to `AgentPanel`;
  cost is negligible.
- Users familiar with the old `c` / `y` / `a` shortcuts must learn the new
  chords. Status-bar confirmation messages display the new key names.
- `Mode::Agent` now has five `Ctrl`-chord bindings; discoverable only via the
  which-key popup or documentation.
