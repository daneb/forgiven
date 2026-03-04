# ADR 0039 — Agent Status Indicator: Live Phase Tracking in the Agent Panel Title

**Status:** Accepted

---

## Context

The Copilot agent panel runs an agentic tool-calling loop in a background tokio task. The loop
alternates between three distinct phases that each introduce latency visible to the user:

1. **HTTP request / first-token wait** — after submitting a message (or after a tool completes),
   the agent makes a fresh `POST /chat/completions` call and waits for the API to begin
   streaming. This can take several seconds for large models or loaded endpoints.
2. **Token streaming** — the model streams its reply token by token. The panel already renders a
   blinking cursor (`▋`) next to the "Copilot" label during this phase.
3. **Synchronous tool execution** — between rounds the loop executes tool calls (e.g.
   `read_file`, `edit_file`) on the main thread, then constructs the next message payload before
   making the next HTTP call.

Only phase 2 produced any visible UI feedback. Phases 1 and 3 showed a frozen blinking cursor
with no additional context, making the editor appear hung. Users had no way to distinguish
"waiting for the API" from "stuck" or "silent error".

Additionally, the existing `current_round` and `max_rounds` fields on `AgentPanel` were tracked
internally and used in inline warning messages, but were never surfaced as a persistent status
visible during normal operation.

---

## Decision

### 1. Add `AgentStatus` enum (`src/agent/mod.rs`)

A new public enum tracks the exact phase of the background task:

```rust
pub enum AgentStatus {
    Idle,
    WaitingForResponse { round: usize },
    Streaming { round: usize },
    CallingTool { round: usize, name: String },
}
```

`AgentStatus` implements a `label(max_rounds: usize) -> Option<String>` method that produces
a short human-readable string for display:

| State | Label |
|---|---|
| `Idle` | *(no label)* |
| `WaitingForResponse { 2 }` | `waiting… [2/20]` |
| `Streaming { 2 }` | `streaming [2/20]` |
| `CallingTool { 2, "read_file" }` | `read_file [2/20]` |

### 2. Add `status: AgentStatus` field to `AgentPanel`

`AgentPanel::new()` initialises with `AgentStatus::Idle`. `submit()` sets
`WaitingForResponse { round: 1 }` immediately before spawning the background task, covering
the gap between pressing Enter and the first `RoundProgress` event arriving.

### 3. Wire transitions in `poll_stream()` (`src/agent/mod.rs`)

`poll_stream()` already processes `StreamEvent`s on each UI tick. Each event now also drives a
status transition:

| Event | New status |
|---|---|
| `RoundProgress { current, .. }` | `WaitingForResponse { round: current }` |
| `Token(_)` | `Streaming { round: current_round }` |
| `ToolStart { name, .. }` | `CallingTool { round: current_round, name }` |
| `ToolDone { .. }` | `WaitingForResponse { round: current_round }` |
| `Done` | `Idle` |
| `Error(_)` | `Idle` |

### 4. Render status in the panel title (`src/ui/mod.rs`)

`render_agent_panel` appends the status label (when non-empty) to the history block title,
separated by a filled dot `●`:

```
 Copilot Chat [gpt-4o]                         ← idle
 Copilot Chat [gpt-4o]  ● waiting… [1/20]      ← API call in-flight
 Copilot Chat [gpt-4o]  ● streaming [1/20]      ← tokens flowing
 Copilot Chat [gpt-4o]  ● read_file [2/20]      ← tool executing
```

The existing scroll-position suffix (`↑ scrolled (80%)`) is still appended after the status
when the user has scrolled up.

---

## Alternatives considered

**Full debug overlay / second screen**

A dedicated debug screen could show all internal state (token buffer length, HTTP timings, tool
result previews). Rejected for this iteration: the status label in the title covers the primary
pain point (distinguishing active work from a genuine hang) with minimal UI footprint. A debug
overlay remains a viable future addition if deeper diagnostics are needed.

**Spinner animation in the title**

A cycling ASCII spinner (`⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏`) would make each phase visually distinct
from a frozen state. Rejected: it would require either a monotonic frame counter on `AgentPanel`
or a separate tick-driven state, adding complexity. The static `●` dot combined with the
descriptive label is sufficient — the blinking `▋` cursor in the chat body already provides
animation cues during the streaming phase.

**Inline status message in the chat body**

Appending a `[Round 2/20 — waiting…]` line to `streaming_reply` would surface the same
information inside the message flow. Rejected: inline messages pollute the conversation history
that is committed to `self.messages` on completion. Title metadata is ephemeral and does not
survive into the stored message content.

---

## Consequences

**Positive**
- Users can always see what the agent is doing between and during rounds without reading log
  output. The "hang" ambiguity is eliminated.
- Tool names are visible in the title during synchronous execution, supplementing the inline
  `⚙ read_file(…)` entries already appended to the streaming reply.
- Round progress (`[2/20]`) is now permanently visible in the title rather than only appearing
  in the MaxRoundsWarning inline message.

**Negative / trade-offs**
- The panel title is slightly wider when active; on very narrow terminal splits the status
  suffix may be clipped by the border. This is acceptable: ratatui clips titles gracefully
  and the information is non-critical.
- `AgentStatus::CallingTool` is set on the first `ToolStart` event of a batch and only cleared
  on `ToolDone`. When multiple tool calls are batched in a single round, the title shows only
  the most recently started tool name, not all parallel calls. The current tool executor
  processes calls sequentially, so this is not a practical issue today.
