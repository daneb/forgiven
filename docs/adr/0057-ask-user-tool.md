# ADR 0057 — Agent `ask_user` Tool

**Date:** 2026-03-10
**Status:** Accepted

---

## Context

The agentic loop can perform long, multi-step plans autonomously. Before this change the agent had no way to pause mid-plan and ask a clarifying question: it would either proceed with an assumption (risking destructive or incorrect work) or emit explanatory text and stop, breaking the flow.

Two gaps were identified:

1. **No structured pause mechanism** — the agent could not present the user with a choice and wait for input before continuing.
2. **Continuation dialog blocks output** — the existing `AwaitingContinuation` pattern rendered a dialog centered over the full terminal, covering the streaming plan output the user needed to read in order to answer intelligently.

---

## Decision

### New `ask_user` tool

A built-in `ask_user` tool is added to the agent's tool set. When called by the model, the agentic loop:

1. Emits a `StreamEvent::AskingUser { question, options }` event.
2. Blocks on a dedicated `question_rx: mpsc::UnboundedReceiver<String>` channel.
3. Resumes only after the user makes a selection (or cancels with Esc).

The tool definition presented to the model:

```json
{
  "name": "ask_user",
  "description": "Pause and ask the user a question before proceeding. Use when you need clarification about intent, want approval for a destructive action, or need the user to choose between meaningful alternatives.",
  "parameters": {
    "question": "string — the question to display",
    "options":  "string[] — choices (defaults to [\"Yes\", \"No\"])"
  }
}
```

The system prompt instructs the model to use `ask_user` **only** for genuinely ambiguous situations — not to confirm routine read/write operations.

### State and channels

| Field | Type | Purpose |
|---|---|---|
| `AgentPanel.asking_user` | `Option<AskUserState>` | Non-None while dialog is visible |
| `AgentPanel.question_tx` | `Option<UnboundedSender<String>>` | Sends the chosen answer back to the loop |
| `AskUserState.selected` | `usize` | Currently highlighted option |

Both `question_tx` and `asking_user` are cleared on `StreamEvent::Done` and `StreamEvent::Error`.

### Keyboard handling

While `asking_user` is `Some`, all key events are intercepted before normal mode dispatch:

| Key | Action |
|---|---|
| `↑` / `k` | Move selection up |
| `↓` / `j` | Move selection down |
| `Enter` | Confirm selection, resume loop |
| `Esc` | Cancel (sends last option, typically "No") |

### Dialog placement

The `render_ask_user_dialog` function renders within the **agent panel area** and is anchored to the **bottom** of that area, not centered over the full terminal. This keeps the streaming plan output above the dialog fully visible so the user has the context they need to answer.

```
┌─ Agent Panel ─────────────────────────┐
│  ... plan output ...                  │
│  ... plan output ...                  │
│  ... plan output ...                  │
├───────────────────────────────────────┤
│ ❓ Question                           │
│                                       │
│  ▶ Yes                                │
│    No                                 │
│                                       │
│ ↑/↓ or j/k = move  Enter = confirm   │
└───────────────────────────────────────┘
```

The selected answer is echoed into the message history as `→ **<choice>**` so it appears in the conversation record.

---

## Implementation

| File | Change |
|---|---|
| `src/agent/tools.rs` | Added `ask_user` to `tool_definitions()`; `args_summary()` shows question text |
| `src/agent/mod.rs` | `AskUserState` struct; `asking_user` + `question_tx` fields on `AgentPanel`; `confirm_user_question()`, `cancel_user_question()`, `move_question_selection()` methods; `StreamEvent::AskingUser` variant; `agentic_loop` handles `ask_user` tool call; `poll_stream()` maps event to `AgentPanel.asking_user` |
| `src/editor/mod.rs` | Key-event interception when `asking_user.is_some()` |
| `src/ui/mod.rs` | `render_ask_user_dialog()` anchored to bottom of panel area; called with panel `area` not `frame.area()` |

No new dependencies.

---

## Consequences

- **Positive**: Agent can surface genuine decision points without abandoning the plan.
- **Positive**: Dialog is anchored below the output, so the user always sees the context they need.
- **Positive**: Selected answer is echoed in chat history, providing a clear audit trail.
- **Positive**: System-prompt guidance discourages over-use; the tool is reserved for genuinely ambiguous moments.
- **Negative**: A poorly-behaved model could call `ask_user` excessively for trivial confirmations — mitigated by the system prompt constraint but not enforced structurally.
- **Negative**: 5-minute timeout on `question_rx.recv()` (inherited from the continuation pattern) silently resumes the loop if the user walks away — could produce unexpected results.
