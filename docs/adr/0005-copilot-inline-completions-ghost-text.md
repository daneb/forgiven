# ADR 0005 — Copilot Inline Completions and Ghost Text

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

The primary UX goal for the Copilot PoC is **ghost text** — a dim preview of the
suggested completion rendered after the cursor as the user types, accepted with Tab or
dismissed with Esc.

LSP 3.18 defines `textDocument/inlineCompletion` for this, but `lsp-types 0.97`
predates that spec. The response shape also differs from standard completions.

## Decision

### Request

A raw JSON request is used instead of the typed `send_request<R>` path:

```rust
pub fn inline_completion(&mut self, uri: &Uri, line: u32, character: u32)
    -> Result<oneshot::Receiver<serde_json::Value>>
```

Payload:
```json
{
  "textDocument": { "uri": "file:///…" },
  "position":     { "line": 0, "character": 5 },
  "context":      { "triggerKind": 2 }
}
```

`triggerKind: 2` = Automatic (triggers after a debounce, not on explicit keystroke).

### Debounce

To avoid hammering the API on every keystroke, completion requests are debounced:

```
Insert char → last_edit_instant = Some(Instant::now())
                                  ghost_text = None
                                  pending_completion = None

event loop frame:
  if no pending request && no ghost text && last_edit elapsed > 300 ms && mode == Insert:
      request_inline_completion()
      last_edit_instant = None
```

The 300 ms debounce is a compile-time constant `COMPLETION_DEBOUNCE_MS`.

### Response parsing

```rust
fn parse_first_inline_completion(value: serde_json::Value) -> Option<String>
```

Handles both `InlineCompletionList { items: [...] }` and bare `[...]` array shapes.
Extracts the first item's `insertText` field.

### Ghost text state

Three fields added to `Editor`:

```rust
ghost_text: Option<(String, usize, usize)>,  // (text, buf_row, buf_col)
pending_completion: Option<oneshot::Receiver<serde_json::Value>>,
last_edit_instant: Option<std::time::Instant>,
```

### Rendering

Ghost text is passed into the UI renderer as `Option<(&str, usize, usize)>`. In the
line-rendering loop, if the current row/col matches the ghost text anchor, the first
line of the suggestion is appended as a `DarkGray` span after the normal line content.
Multi-line suggestions display only their first line in ghost form; the full text is
inserted on Tab accept.

### Accept / dismiss

| Key | Effect |
|-----|--------|
| Tab (in Insert mode) | Insert `ghost_text` into buffer char-by-char; clear state |
| Esc (in Insert mode) | Switch to Normal; clear ghost_text + pending_completion |
| Any cursor movement  | Clear ghost_text |
| Next keystroke       | Resets debounce timer; old ghost text cleared |

### Client registration

`request_inline_completion()` looks up the language-specific client first, then falls
back to the `"copilot"` sentinel client:

```rust
let client = self.lsp_manager.get_client(&language)
    .or_else(|| self.lsp_manager.get_client("copilot"));
```

This means Copilot provides completions for all file types, not just Rust.

## Consequences

- Ghost text is non-blocking: the request fires and the result is polled each frame
- If the user types before the response arrives, the in-flight receiver is dropped
  (the channel closes) and a new request is debounced
- Only the first line of a multi-line suggestion is visible as ghost text; this is
  intentional — showing multiple ghost lines is visually disruptive in a terminal
- The `textDocument/inlineCompletion` method name is sent as a raw string; if
  `lsp-types` gains support in a future version the raw path can be replaced with the
  typed API without changing the surrounding logic
