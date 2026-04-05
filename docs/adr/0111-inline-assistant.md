# ADR 0111 — Inline Assistant (Selection Transform)

**Date:** 2026-04-05
**Status:** Accepted

---

## Context

The main agent panel (ADR 0045) is optimised for multi-turn, tool-using conversations. It
carries full conversation history, supports multi-round agentic loops, and renders in a
dedicated side panel. This is the right UX for open-ended tasks.

A complementary mode is needed for quick, single-shot code transformations: "make this
function async", "add error handling", "translate to idiomatic Rust". The user selects
code, types a short directive, and the AI rewrites the selection in-place. No history,
no tools, no multi-turn back-and-forth — just fast contextual editing.

Cursor calls this Cmd+K. Zed calls it inline assistant. Windsurf has an equivalent. All
three keep it strictly separate from the main chat UX. Forgiven should do the same.

---

## Decision

### UX flow

```
1. [Visual / Normal]  SPC a i           → enter Mode::InlineAssist (Phase::Input)
2. [Phase::Input]     type prompt text  → accumulate in InlineAssistState.prompt
3. [Phase::Input]     Enter             → send to LLM → Phase::Generating
4. [Phase::Input]     Esc               → cancel, return to Normal
5. [Phase::Generating] tokens arrive    → accumulate in InlineAssistState.response
6. [Phase::Generating] Done event       → Phase::Preview
7. [Phase::Preview]   Enter             → accept: replace selection, return to Normal
8. [Phase::Preview]   Esc / q           → reject: discard response, return to Normal
```

Invocation works from Normal mode (cursor position only, no selection) as well as Visual
and VisualLine modes. Without a selection, the LLM receives the current line as context
and inserts its output below the cursor.

### State

A new `InlineAssistState` struct is held on `Editor` as `Option<InlineAssistState>`.
It is `None` outside `Mode::InlineAssist` and dropped on accept or cancel.

```rust
pub struct InlineAssistState {
    /// User-typed directive (built up during Phase::Input)
    pub prompt: String,
    /// Original selected text saved for revert
    pub original_text: String,
    /// Where the selection was (normalised start, end) — None when cursor-only
    pub original_selection: Option<(Cursor, Cursor)>,
    /// Accumulated LLM response
    pub response: String,
    pub phase: InlineAssistPhase,
    pub stream_rx: mpsc::UnboundedReceiver<StreamEvent>,
    pub abort_tx: oneshot::Sender<()>,
}

pub enum InlineAssistPhase {
    Input,       // Waiting for user to type prompt
    Generating,  // LLM streaming, response accumulating
    Preview,     // Done streaming, waiting for accept/reject
}
```

### Keymap

`SPC a i` — registered under the existing `agent` sub-tree in `build_leader_tree()`.
Works in Normal, Visual, and VisualLine modes. Ignored if no buffer is open.

New `Action` variants:

```rust
InlineAssistStart,
InlineAssistAccept,
InlineAssistCancel,
```

`InlineAssistAccept` and `InlineAssistCancel` are only dispatched from
`handle_inline_assist_mode()`; they are not mapped to physical keys directly.

### System prompt

The inline assist request uses a minimal, transformation-focused system prompt:

```
You are a code transformation engine.
The user will provide a code selection and a directive.
Reply with ONLY the transformed code — no explanation, no markdown fences,
no commentary. Preserve the original indentation of the first line.
```

No project tree, no open-file context, no tool declarations. The selection text is
sent as the user message, with the directive appended as a second paragraph.

`max_rounds = 1`. Tool calls are stripped from the tool list before the request.

### LLM integration

`InlineAssistState` is populated by calling a new `AgentPanel::start_inline_assist()`
method (analogous to `submit()`) which:

1. Builds the minimal message array (system + one user message containing selection +
   directive).
2. Spawns `tokio::spawn(agentic_loop(...))` with `max_rounds=1`, no tools.
3. Returns `(stream_rx, abort_tx)` to the caller.

This reuses the same `StreamEvent` channel and `agentic_loop` that powers the main
panel — no new LLM plumbing required.

### Streaming poll

`Editor::poll_inline_assist()` is called each frame (alongside the existing
`agent_panel.poll_stream()`). It drains up to 64 tokens per frame from
`inline_assist.stream_rx` and appends to `inline_assist.response`. On `StreamEvent::Done`
it transitions `phase` to `Preview`.

### UI

A new function `UI::render_inline_assist_overlay()` is called from the main render loop
when `editor.mode == Mode::InlineAssist`.

**Phase::Input** — a 1-line prompt bar rendered at the bottom of the buffer area:

```
╭─ Inline AI ──────────────────────────────────────────╮
│ > add error handling for the None case█              │
╰──────────────────────────────────────────────────────╯
```

The bar uses `Clear` then `Block` + `Paragraph` at a fixed 3-row rect pinned to the
bottom of the buffer view. Cursor is a block rendered inside the bar.

**Phase::Generating** — same bar, replaced with a progress indicator:

```
╭─ Inline AI ── ⚡ generating… ─────────────────────────╮
│ fn handle_value(v: Option<T>) -> Result<T, Error> {  │
╰── Esc to cancel ─────────────────────────────────────╯
```

The accumulating `response` is shown live inside the bar (scrolling last N lines).

**Phase::Preview** — bar expands to show the full response with accept/reject hint:

```
╭─ Inline AI ── ✓ ready ────────────────────────────────╮
│ fn handle_value(v: Option<T>) -> Result<T, Error> {  │
│     let inner = v.ok_or(Error::Missing)?;            │
│     Ok(inner)                                        │
│ }                                                    │
╰── Enter=accept  Esc=cancel ──────────────────────────╯
```

The overlay uses `min(response_lines + 4, area.height / 2)` rows, anchored to the
bottom of the buffer area. `Clear` is rendered beneath it to avoid bleed-through.

The selected region in the main buffer is highlighted with a dim background during the
entire interaction so the user can see what will be replaced.

### Accept / cancel

**Accept (`Enter` in Preview):**
1. If `original_selection` is `Some`, call `buffer.delete_selection()` to remove the
   original text.
2. Call `buffer.insert_text_block(&inline_assist.response)`.
3. Call `buffer.mark_modified()` (triggers LSP re-analysis and save prompt).
4. Drop `inline_assist` and set `mode = Mode::Normal`.

**Cancel (`Esc` in Input or Generating or Preview):**
1. Send on `abort_tx` to stop any in-flight LLM request.
2. Drop `inline_assist` (original text never touched).
3. Set `mode = Mode::Normal`.

---

## Implementation

### Files modified

| File | Change |
|------|--------|
| `src/keymap/mod.rs` | Add `Mode::InlineAssist`; add `Action::{InlineAssistStart, InlineAssistAccept, InlineAssistCancel}`; register `SPC a i` leaf |
| `src/agent/panel.rs` | Add `start_inline_assist()` method |
| `src/editor/mod.rs` | Add `inline_assist: Option<InlineAssistState>` field; add `InlineAssistState` + `InlineAssistPhase`; add `poll_inline_assist()` |
| `src/editor/input.rs` | Add `handle_inline_assist_mode()` |
| `src/editor/actions.rs` | Add arms for `InlineAssistStart`, `InlineAssistAccept`, `InlineAssistCancel` |
| `src/ui/mod.rs` | Call `render_inline_assist_overlay()` when mode is `InlineAssist` |
| `src/ui/popups.rs` | Add `render_inline_assist_overlay()` |

### New structs (in `src/editor/mod.rs`)

```rust
pub struct InlineAssistState {
    pub prompt: String,
    pub original_text: String,
    pub original_selection: Option<(Cursor, Cursor)>,
    pub response: String,
    pub phase: InlineAssistPhase,
    pub stream_rx: mpsc::UnboundedReceiver<StreamEvent>,
    pub abort_tx: oneshot::Sender<()>,
}

pub enum InlineAssistPhase {
    Input,
    Generating,
    Preview,
}
```

### `start_inline_assist()` signature (in `src/agent/panel.rs`)

```rust
pub fn start_inline_assist(
    &self,
    selection_text: String,
    project_root: PathBuf,
) -> (mpsc::UnboundedReceiver<StreamEvent>, oneshot::Sender<()>)
```

Returns the channels; the caller wraps them into `InlineAssistState`.

### `handle_inline_assist_mode()` outline (in `src/editor/input.rs`)

```rust
pub(super) fn handle_inline_assist_mode(&mut self, key: KeyEvent) -> Result<()> {
    let assist = match self.inline_assist.as_mut() {
        Some(a) => a,
        None => { self.mode = Mode::Normal; return Ok(()); }
    };

    match assist.phase {
        InlineAssistPhase::Input => match key.code {
            KeyCode::Enter => {
                // Capture selection text, call start_inline_assist(), phase → Generating
            }
            KeyCode::Esc => { self.execute_action(Action::InlineAssistCancel)?; }
            KeyCode::Backspace => { assist.prompt.pop(); }
            KeyCode::Char(c) => { assist.prompt.push(c); }
            _ => {}
        },
        InlineAssistPhase::Generating => match key.code {
            KeyCode::Esc => { self.execute_action(Action::InlineAssistCancel)?; }
            _ => {}
        },
        InlineAssistPhase::Preview => match key.code {
            KeyCode::Enter => { self.execute_action(Action::InlineAssistAccept)?; }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.execute_action(Action::InlineAssistCancel)?;
            }
            _ => {}
        },
    }
    Ok(())
}
```

---

## Consequences

**Positive**

- Fast, single-shot code transforms without leaving the editor or opening the agent panel.
- Reuses the entire LLM/streaming infrastructure — no new network code.
- Non-destructive until accepted: buffer is untouched until `Enter` in Preview phase.
- Works with or without a selection, covering both "rewrite this block" and
  "insert something here" patterns.
- No new crates required.

**Negative / trade-offs**

- `max_rounds=1` means the LLM cannot call tools (read other files, run shell commands).
  For transforms that require cross-file context the user should use the agent panel.
- The overlay approach (bottom-anchored bar) is simpler than a true side-by-side diff
  view. Multi-file diffs are explicitly out of scope here — see roadmap item 13.
- Streaming into a preview bar (rather than directly into the buffer) means the user
  cannot see the replacement in context until they accept. A future revision could
  render the replacement inline with the original dimmed below it.

**Future work**

- Inline diff: show original and replacement simultaneously before accept.
- Multi-round inline: allow the user to type a follow-up prompt in Preview phase.
- Selection from tree-sitter text objects: `viF` selects function, `SPC a i` transforms it.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0007](0007-vim-modal-keybindings.md) | Mode enum — `InlineAssist` is a new mode |
| [0008](0008-normal-mode-editing-operations.md) | Buffer mutation pattern — same snapshot/notify flow |
| [0045](0045-mcp-client.md) | Agent panel — `start_inline_assist()` is a sibling of `submit()` |
| [0105](0105-tree-sitter-text-objects.md) | Text objects — natural trigger for inline assist targets |
