# ADR 0006 — Copilot Chat / Agent Panel

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

Beyond inline completions, Copilot Enterprise offers a conversational interface
("Copilot Chat") that can answer questions, explain code, and produce multi-line code
blocks. This needs a persistent side panel distinct from the code editor area.

## Decision

### Layout

When the agent panel is visible the terminal is split horizontally:

```
┌──────────────────────────┬───────────────────┐
│                          │  Copilot Chat     │
│   Code editor (60%)      │  history          │
│                          │                   │
│                          ├───────────────────┤
│                          │  Input box (3 ln) │
└──────────────────────────┴───────────────────┘
│ NORMAL  src/main.rs  1:1                      │
└───────────────────────────────────────────────┘
```

Implemented with `Layout::default().direction(Direction::Horizontal).constraints([60%, 40%])`.

The panel is further split vertically: `Constraint::Min(1)` for history,
`Constraint::Length(3)` for the input box.

### Mode

A new `Mode::Agent` was added to the `Mode` enum. While in this mode all keypresses are
routed to `handle_agent_mode()` instead of the normal Vim-style handler.

Keybindings in Agent mode:

| Key | Effect |
|-----|--------|
| `Esc` | Close panel, return to Normal mode |
| `Tab` | Return focus to editor, keep panel visible |
| `Enter` | Submit input; start streaming reply |
| `Backspace` | Delete last character from input |
| `↑` / `↓` | Scroll chat history |
| `a` (empty input) | Apply first code block from last reply to buffer |
| Any printable char | Append to input |

Leader key bindings (in Normal mode):

| Sequence | Effect |
|----------|--------|
| `SPC a a` | Toggle panel visible/hidden |
| `SPC a f` | Focus panel (open if hidden) |

### State: `AgentPanel`

```rust
pub struct AgentPanel {
    pub visible: bool,
    pub focused: bool,
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub scroll: usize,          // lines from bottom; 0 = pinned to newest
    token: Option<CopilotApiToken>,
    pub streaming_reply: Option<String>,
    pub stream_rx: Option<mpsc::UnboundedReceiver<StreamEvent>>,
}
```

### Streaming

Copilot Chat uses the OpenAI-compatible SSE (Server-Sent Events) streaming API:

```
POST https://api.githubcopilot.com/chat/completions
{ "model": "gpt-4o", "messages": […], "stream": true }

← data: {"choices":[{"delta":{"content":"Hello"}}]}
← data: {"choices":[{"delta":{"content":" world"}}]}
← data: [DONE]
```

A `tokio::spawn` task reads the byte stream, parses SSE lines, and sends
`StreamEvent::Token(String)` values over an `mpsc::UnboundedReceiver`. The editor
event loop calls `agent_panel.poll_stream()` each frame to drain available tokens into
`streaming_reply` — no blocking, no extra threads.

A blinking `▋` cursor in the panel header indicates a reply is in progress.

### Scroll

`scroll: usize` is defined as "number of rendered lines above the bottom":
- `0` = pinned to newest content
- Higher values = scrolled toward older content

`scroll_up()` increments; `scroll_down()` decrements (saturating at 0).

The renderer caps `scroll` at `max_scroll = total_lines - visible_height` so you cannot
scroll past the top. The panel title dynamically shows the scroll percentage and a
hint when more content is available above.

New messages auto-reset `scroll` to `0` (both on user submit and on stream `Done`).

### Code application

When the latest assistant message contains a fenced code block (` ``` … ``` `),
pressing `a` with an empty input box:

1. Calls `AgentPanel::extract_code_blocks()` to find all ` ``` ` fenced blocks
2. Takes the first block
3. Calls `Buffer::insert_text_block(&code)` which inserts it at the cursor position,
   handling multi-line content correctly (splits existing line, splices new lines in)
4. Switches focus back to the editor and shows a status message:
   `"Applied N lines from Copilot…"`

The input box title updates to show `[a] apply code to buffer` as a hint when a code
block is available and the input is empty.

### Context injection

When submitting a message, the current buffer's full content (all lines joined) is
injected as a system prompt:

```
You are a helpful coding assistant embedded in the 'forgiven' terminal editor.

Current file context:
```
<buffer contents>
```
```

Up to the last 10 exchanges (20 messages) of conversation history are also included to
maintain conversational continuity within the Copilot API's context window.

## Consequences

- The 60/40 split shrinks the editor area; on narrow terminals this can become cramped
  — a future improvement would be a configurable split ratio or an overlay mode
- `block_in_place` is used in `handle_agent_mode` to call `submit().await` from the
  synchronous event loop; this blocks one tokio thread for the duration of the HTTP
  request setup (~100–200 ms) but is acceptable given it only occurs on Enter
- Conversation history is truncated to the last 20 messages to stay within token limits;
  a smarter summarisation strategy could be added later
- The panel is intentionally stateless across sessions (no persistence to disk);
  history is lost when the editor exits
