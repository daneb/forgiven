# ADR 0128 — Investigation Subagent (`SPC a v`)

**Date:** 2026-04-15  
**Status:** Implemented (keybind fix applied this session)

---

## Context

During an agentic session, the user often wants a quick read-only answer before or
alongside a main prompt: "which files handle X?", "what does this call chain look like?",
"is Y already implemented?". Asking the main agent these questions costs context budget
— the answer plus all tool calls get appended to history and re-sent every subsequent
round.

The existing options were:

- **Ask the main agent directly** — answer and all tool output enter the main conversation
  history and accumulate re-send cost indefinitely.
- **Open a second terminal session** — works but breaks flow; findings aren't visible in
  the panel and can't be referenced in the next prompt.
- **`SPC a j` (janitor compress)** — compresses history after the fact; does nothing to
  prevent the bloat from the investigation itself.

---

## Decision

Add a lightweight **investigation subagent**: a single-round, read-only agentic loop that
runs concurrently with or prior to the main session, explores the codebase, then injects a
compact summary (≤ 200 words) as a System message into the main conversation history.

### Design principles

1. **Single round, read-only.** `max_rounds = 1` and the system prompt forbids edits. The
   model may call `get_file_outline`, `get_symbol_context`, `search_files`, and `read_file`
   but not `write_file`, `create_task`, or any mutating tool.

2. **Separate stream, separate buffer.** Uses `investigation_rx` / `investigation_buf`
   fields on `AgentPanel` — completely independent of the main `stream_rx`. Both can be
   active simultaneously without interference. `poll_stream()` drains both independently
   each frame.

3. **Result injected as a System message.** On `StreamEvent::Done`, the collected summary
   is pushed as `Role::System` with a `🔍 Investigation result:` prefix. It appears in the
   chat panel and is included in the history sent on the next main-agent round, so the
   model can reference it without the full tool-call chain.

4. **Zero residual cost.** Tool calls made by the investigation subagent are never added
   to `self.messages`. Only the final summary text enters history. Subsequent rounds
   re-send a single short System message rather than the full raw tool payloads.

### Keybinding

`SPC a v` — available from Normal mode and (after the fix in this session) from Agent
mode when the input box is empty.

**Workflow:**

1. Open the agent panel and ensure the input box is empty (or press `Ctrl+Bksp` to clear).
2. Type an investigation query (e.g. "where is the token budget computed and which fields
   feed it?").
3. Press `SPC a v`.
4. Status bar shows `Investigation running…`. The investigation runs concurrently — the
   main agent session is unaffected.
5. On completion, a `🔍 Investigation result:` System message appears in the chat panel.
6. Type the main prompt and press `Enter`. The model sees the investigation result in
   context automatically.

### System prompt

```
You are a code investigator for the 'forgiven' terminal editor.
Project root: {root}

INVESTIGATION RULES:
- Explore the codebase using get_file_outline, get_symbol_context, search_files,
  and read_file as needed.
- Make NO edits — this is read-only exploration.
- After exploring, output a COMPACT SUMMARY (max 200 words) covering:
  * Which files/functions are involved
  * Key call paths or data flow
  * Any non-obvious facts the developer should know
- No preamble, no pleasantries. Start directly with the findings.
```

---

## Keybind fix (2026-04-15)

The original implementation placed `SPC a v` in the Normal-mode leader tree but
`handle_agent_mode` had no leader-key forwarding. Pressing `Space` in Agent mode hit
the `KeyCode::Char(ch)` arm, which typed a literal space into the input box. Then `a`
and `v` typed those characters. The query was silently corrupted — the misfire the user
observed.

Visual mode and VisualLine mode both forward leader sequences:

```rust
if key.code == KeyCode::Char(' ') || self.key_handler.leader_active() {
    let action = self.key_handler.handle_normal(key);
    ...
}
```

Agent mode now has equivalent forwarding, conditioned on `input.is_empty()` for the
initial Space press so that a space in a typed query is never consumed as a leader:

```rust
if (key.code == KeyCode::Char(' ') && input_empty) || self.key_handler.leader_active() {
    let action = self.key_handler.handle_normal(key);
    if !matches!(action, Action::Noop) {
        return self.execute_action(action);
    }
    if self.key_handler.leader_active() {
        return Ok(()); // sequence still in-flight — don't fall through to input_char
    }
}
```

Once a leader is in progress `self.key_handler.leader_active()` is true, so `a` and `v`
are forwarded even though the input check is no longer involved.

---

## Known limitations

### Concurrent use during a running prompt

When the main agent is running, the input box is empty (cleared on submit). The user can
type a new query into the box while streaming and press `SPC a v` — but the empty-input
guard fires first, starting the leader with the input still empty. By the time `v` is
pressed the input has content, but `leader_active()` carries the forwarding forward
correctly. In practice this works, but requires the user to know the input will be read
at the moment `v` is pressed, not earlier.

A cleaner solution is a dedicated modifier chord that reads input regardless of whether
it is empty — see the "Future improvements" section.

### Status indicator during concurrent runs

When both `stream_rx` and `investigation_rx` are active, `investigation_rx` drain sets
`self.status = AgentStatus::Idle` on `Done`, which can briefly overwrite the main stream's
`Streaming` status. This is cosmetic but noticeable. The fix is to only set Idle in the
investigation drain when `stream_rx` is `None`.

---

## Alternatives considered

### Inject query directly into main conversation

The simplest option — just ask the agent directly. Rejected because investigation tool
calls (often 3–8 `read_file` / `search_files` calls totalling 10–40 K tokens) enter
history and are re-sent every subsequent round. On a 128 K context window this is a
meaningful budget hit for what is often a one-off orientation question.

### Separate full session (new conversation)

Preserves the main session but findings are siloed. The user must manually copy text
back. Rejected — too much friction for routine use.

### Slash command (`/investigate`)

Would need special parsing and dispatch through the slash menu. No advantage over a
dedicated keybind given the workflow already lives in the agent panel. May be added as
an alias if discoverability proves to be an issue.

---

## Future improvements

### F1 — Dedicated modifier chord for concurrent use

Add `Ctrl+I` (or another unambiguous chord) to `handle_agent_mode` that triggers
`AgentInvestigate` regardless of whether a leader sequence is in-flight and regardless of
input content. This removes the empty-input constraint and makes concurrent
investigation during a running agent unambiguous.

```rust
KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    return self.execute_action(Action::AgentInvestigate);
},
```

Note: `Ctrl+I` maps to `\t` (Tab, 0x09) in many terminal emulators and may not be
distinguishable from `Tab` depending on the terminal. Verify against the target
terminal or choose `Alt+i` instead, which is reliably distinct.

### F2 — Fix Idle status overwrite during concurrent runs

In the investigation drain in `poll_stream()`, only reset status to Idle when the main
stream is not active:

```rust
Ok(StreamEvent::Done) => {
    ...
    if self.stream_rx.is_none() {
        self.status = AgentStatus::Idle;
    }
    ...
}
```

### F3 — Investigation scope control

Allow the user to scope the investigation to a specific file or symbol by passing the
currently open buffer or a visual selection as additional context. The system prompt
already includes the project root; add the buffer path and a selection excerpt when
`AgentInvestigate` fires from a non-empty buffer context. Mirrors how the main submit
injects the current buffer at [editor/input.rs — the `context` variable in the Enter
handler].

### F4 — Persist investigation results to disk

Investigation results are injected as System messages and therefore survive until the
next `new_conversation()`. They are not persisted to the session JSONL log planned in
ADR 0123. Once ADR 0123 Phase 1 (disk persistence) is implemented, investigation System
messages should be written to the log alongside regular messages so they survive across
sessions.

### F5 — Token budget for the investigation subagent

The investigation subagent currently inherits the same `context_window` budget as the
main session. Because it is single-round, the practical ceiling is low, but there is no
explicit guard. Add an `investigation_max_tokens: usize` config key (default: 4096 output
tokens) to cap the model's reply. This also makes the 200-word summary constraint
enforceable at the API level rather than relying solely on the system prompt instruction.

---

## Files changed

| File | Change |
|------|--------|
| `src/agent/panel.rs` | `start_investigation_agent()`, `investigation_rx` drain in `poll_stream()` |
| `src/agent/mod.rs` | `investigation_rx`, `investigation_buf`, `AgentStatus::Investigating` fields |
| `src/keymap/mod.rs` | `Action::AgentInvestigate`, `SPC a v` leader node |
| `src/editor/actions.rs` | `Action::AgentInvestigate` dispatch |
| `src/editor/input.rs` | Leader-key forwarding in `handle_agent_mode` (keybind fix, 2026-04-15) |
