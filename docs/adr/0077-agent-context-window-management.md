# ADR 0077 — Agent Context Window Management

**Date:** 2026-03-20
**Status:** Accepted

---

## Context

The Copilot agent panel accumulates the full conversation history in
`AgentPanel.messages: Vec<ChatMessage>` and sends a rolling window to the API
on every turn. The prior implementation used a **hardcoded 20-message sliding
window** (`messages.len().saturating_sub(20)`) with no awareness of actual
token consumption.

This approach failed in practice: a single large tool result, pasted diff, or
code block can consume tens of thousands of tokens, so 20 messages can easily
exceed the model's context limit. The error observed:

```
[Error: Copilot Chat API error (400 Bad Request):
{"error":{"message":"prompt token count of 293360 exceeds the limit of
272000","code":"model_max_prompt_tokens_exceeded"}}]
```

Additionally, there was no user-facing way to start a fresh conversation
without switching models (which did clear history as a side-effect via
`new_conversation()`, but was not its purpose).

Two related gaps:

1. `context_window_size()` was already implemented (ADR 0040) and returned the
   model's token limit from the Copilot `/models` API — but was never used to
   gate message inclusion.
2. `new_conversation()` existed but was only called internally on model switch;
   no keybinding exposed it.

---

## Decision

### 1. Token-aware history truncation

Replace the fixed 20-message slice with a **reverse-walk budget algorithm**:

- **Budget** = 80% of `context_window_size()` minus an estimate for the system
  prompt (`system.len() / 4` tokens).
- Walk `self.messages` from **newest to oldest**, accumulating estimated token
  cost (`content.len() / 4 + 4` per message, using the standard chars/4
  approximation).
- Stop including older messages once the accumulated cost would exceed the
  budget.
- System-role divider messages (inserted by `new_conversation()` as visual
  markers) are skipped in the walk — they carry no token cost and are already
  filtered from the API payload.

The 80% headroom leaves ~20% for the user's current message, tool definitions,
tool results returned in the same round, and approximation error. The
chars/4 heuristic is intentionally conservative — GPT-family tokenizers
average slightly above 4 chars/token for code-heavy content.

### 2. `SPC a n` — explicit new conversation

Add `Action::AgentNewConversation` wired to `SPC a n` in the leader-key tree.
Dispatching the action calls `agent_panel.new_conversation(model_name)` and
sets a status bar message confirming the reset.

This gives users a deliberate, low-friction way to start a clean context
before beginning a new task — analogous to the **New Chat** button in VS Code
Copilot Chat or **New Thread** in Cursor.

---

## Implementation

### `src/keymap/mod.rs`

New `Action` variant:

```rust
    // Agent panel
    AgentToggle,
    AgentFocus,
    AgentNewConversation, // SPC a n — clear history, start fresh conversation
```

New entry in `build_leader_tree()`:

```rust
agent_node.children.insert('n', KeyNode::leaf("new conversation", Action::AgentNewConversation));
```

### `src/editor/mod.rs`

New match arm in `execute_action()`:

```rust
Action::AgentNewConversation => {
    let model_name = self.agent_panel.selected_model_display().to_string();
    self.agent_panel.new_conversation(&model_name);
    self.set_status(format!("New conversation started · {model_name}"));
},
```

### `src/agent/mod.rs`

Replaced the fixed-count slice with the token-aware walk:

```rust
// ── Token-aware history truncation ────────────────────────────────────
// Estimate tokens using the chars/4 approximation (1 token ≈ 4 chars).
// Budget is 80% of the model's context window minus an estimate for the
// system prompt, so we never approach the hard API limit.
let context_limit = self.context_window_size();
let system_tokens = (system.len() / 4) as u32;
let budget = (context_limit * 4 / 5).saturating_sub(system_tokens);

// Walk from newest to oldest to always keep the most recent messages.
let mut accumulated: u32 = 0;
let mut history_start = self.messages.len(); // default: include nothing
for (i, msg) in self.messages.iter().enumerate().rev() {
    if matches!(msg.role, Role::System) {
        continue; // display-only dividers carry no token cost
    }
    let msg_tokens = (msg.content.len() / 4) as u32 + 4; // +4 for role framing
    if accumulated + msg_tokens > budget {
        break;
    }
    accumulated += msg_tokens;
    history_start = i;
}
```

---

## Consequences

**Positive**
- Eliminates `model_max_prompt_tokens_exceeded` errors during long sessions by
  proactively staying within 80% of the context window.
- Always preserves the most recent messages; older context is silently dropped
  rather than causing a hard API failure.
- Works automatically per model: a 272k-token model gets a larger budget than a
  64k-token model without any configuration.
- `SPC a n` gives users agency over context lifetime — start fresh before a new
  task without losing the session's buffer state.
- Which-key popup (ADR 0068) picks up the new binding and shows "new
  conversation" in the `SPC a` subtree automatically.

**Negative / trade-offs**
- Chars/4 is an approximation. Code-heavy messages (e.g. large Rust files) may
  use more tokens than estimated; the 20% headroom is designed to absorb this.
  A future improvement could use the actual `last_prompt_tokens` value from
  `StreamEvent::Usage` (ADR 0040) to perform post-hoc calibration.
- Dropped messages are not summarised — abrupt context loss can cause the model
  to "forget" earlier decisions. Users should start a `SPC a n` session at
  natural task boundaries rather than relying on automatic truncation to handle
  indefinitely long conversations gracefully.
- No per-message token display in the UI yet. The context gauge in the panel
  title (ADR 0040) already shows cumulative usage; a future ADR could highlight
  when history is being trimmed.
- **No context compaction.** When the budget is exceeded, older messages are
  dropped entirely rather than summarised. A future ADR could fire a background
  Copilot call to replace dropped messages with a compact `[Context summary: …]`
  entry, preserving the semantic gist of early decisions. This was deliberately
  deferred: VS Code Copilot Chat and Cursor both truncate without summarising;
  the `SPC a n` new-session binding covers the primary use case with zero
  latency cost; and compaction introduces its own failure modes (summary
  hallucination, mid-conversation API latency). Revisit when users report losing
  critical early context despite starting fresh sessions at task boundaries.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Keep hardcoded 20-message cap | Fails on large messages as seen in production |
| Token counting via `tiktoken` / external crate | Adds a heavy dependency; chars/4 is sufficient at 80% budget |
| Summarise dropped messages via a secondary API call | Adds latency and cost on every turn; defer to a future ADR |
| Expose `SPC a c` instead of `SPC a n` | `n` aligns with "new" (VS Code, Cursor convention); `c` is ambiguous with "clear" or "copy" |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0040](0040-agent-context-gauge.md) | Context gauge — `context_window_size()` and `last_prompt_tokens` introduced here |
| [0068](0068-which-key-dynamic-height-ask-user-dialog.md) | Which-key popup picks up `SPC a n` automatically |
| [0036](0036-multi-line-agent-panel-input.md) | Agent panel input — related panel UX |
