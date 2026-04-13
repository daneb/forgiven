# ADR 0124 — Agent Input History, Mid-line Cursor, and Label Rename

**Date:** 2026-04-13
**Status:** Accepted

---

## Context

Three small UX gaps existed in the agent panel input box:

1. **No input history** — every message had to be typed from scratch. There was no way to recall and re-use a previous submission, unlike a normal shell or chat UI.
2. **Append-only cursor** — the visual cursor (`_`) was always glued to the end of the input string. The only editing primitives were `input_char` (push) and `input_backspace` (pop). Moving to an earlier position to fix a typo required deleting everything after it.
3. **"Copilot" label** — the input box placeholder read "Ask Copilot…" / "Message Copilot…", which is provider-specific branding. The editor supports multiple LLM backends (Copilot, OpenAI, Anthropic, Ollama, OpenRouter) so the label was misleading.

---

## Decision

### 1. Input history (Up / Down)

Four fields added to `AgentPanel`:

```rust
pub input_history: Vec<String>,  // oldest-first, capped at 50
pub history_idx:   Option<usize>, // None = at live input
pub input_saved:   String,        // draft preserved while browsing
pub input_cursor:  usize,         // byte offset (shared with cursor feature)
```

On each successful `submit()` call the trimmed input is appended to `input_history`. If the vec exceeds 50 entries, the oldest is dropped.

`Up` in agent mode calls `history_up()`:
- First press: saves the current draft to `input_saved`, loads the most-recent history entry.
- Subsequent presses: walk backwards through history; no-op at the oldest entry.

`Down` calls `history_down()`:
- While browsing: walk forwards; on the final press restore `input_saved` and clear `history_idx`.
- When not browsing: falls through to the existing `scroll_down()` (message list scroll).

`Up` also falls through to `scroll_up()` when `input_history` is empty and no browsing is in progress, preserving the pre-existing scroll behaviour.

`clear_input()` and the `submit()` hot-path both reset `history_idx` and `input_saved` so a partially-typed message never gets confused with history state.

### 2. Mid-line cursor (Left / Right)

`input_cursor: usize` is a byte offset into `self.input`. All input primitives now operate at the cursor rather than the end:

```rust
pub fn input_char(&mut self, ch: char) {
    self.input.insert(self.input_cursor, ch);
    self.input_cursor += ch.len_utf8();
}

pub fn input_backspace(&mut self) {
    if self.input_cursor == 0 { return; }
    let prev = self.input[..self.input_cursor]
        .char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
    self.input.remove(prev);
    self.input_cursor = prev;
}
```

`cursor_left` and `cursor_right` walk one UTF-8 scalar at a time using `char_indices`, so multi-byte characters are handled correctly.

The render path replaces the trailing-underscore hack:

```rust
let typed = if focused {
    let cursor = panel.input_cursor.min(panel.input.len());
    format!("{}_{}",&panel.input[..cursor], &panel.input[cursor..])
} else {
    panel.input.clone()
};
```

History navigation always moves the cursor to the end of the loaded string (`self.input_cursor = self.input.len()`).

### 3. Label rename

`src/ui/agent_panel.rs` placeholder titles updated:

| Before | After |
|--------|-------|
| `" Ask Copilot… "` | `" Ask LLM… "` |
| `" Message Copilot… "` | `" Message LLM… "` |

---

## Implementation

| File | Change |
|------|--------|
| `src/agent/mod.rs` | Added `input_history`, `history_idx`, `input_saved`, `input_cursor` fields to `AgentPanel` |
| `src/agent/panel.rs` | Updated `new()` initialiser; rewrote `input_char`, `input_backspace`, `input_newline`; added `cursor_left`, `cursor_right`, `history_up`, `history_down`; updated `clear_input` and `submit` |
| `src/ui/agent_panel.rs` | Cursor rendered at `input_cursor` offset; placeholder text changed to "LLM" |
| `src/editor/input.rs` | Added `Left`/`Right` key arms; `Up`/`Down` arms check history state before falling back to scroll |

No new dependencies. No config schema changes. No breaking changes to existing keybindings.

---

## Consequences

- **Positive**: Shell-style history navigation dramatically reduces re-typing for iterative prompts.
- **Positive**: Mid-line editing eliminates the need to delete-to-cursor to fix a typo anywhere in a message.
- **Positive**: "LLM" label is provider-agnostic and accurate regardless of backend.
- **Neutral**: `Up` no longer scrolls the message list when history is non-empty. Users who relied on `Up` for scrolling must use `Up` only once (to exhaust the history) before it reverts to scroll, or use Page Up / the existing scroll bindings.
- **Neutral**: History is in-memory only and does not persist across sessions. A future ADR could address JSONL persistence alongside session metrics.
