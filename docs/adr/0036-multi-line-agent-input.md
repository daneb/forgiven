# ADR 0036 — Multi-line Agent Panel Input

**Status:** Accepted

---

## Context

The Copilot agent panel input stored the prompt as a single `String` with no newline
support. The ratatui `Paragraph` widget with `Wrap { trim: false }` already
word-wraps long text visually, but there was no way to deliberately insert a line
break. This made it impossible to write structured prompts with:

- Bullet-pointed task lists
- Numbered steps
- Multi-paragraph questions that benefit from clear separation

A secondary issue was that paste-from-clipboard collapsed `\n` characters into
spaces, so users who composed a multi-line prompt in their editor and then pasted it
into the panel would lose all formatting. The collapse was originally added to
prevent an accidental newline mid-paste from triggering submission, but with a
dedicated newline key (`Alt+Enter`) that motivation disappears.

A third issue was that the input box height calculation used a flat character-count
divided by panel width, which ignored explicit newlines. Any `\n` already in the
string (reachable via paste) would display as an actual line break in the `Paragraph`
widget but the height calculation would not account for it, causing the input box to
be shorter than the text it contained.

---

## Decision

### `Alt+Enter` inserts a newline; plain `Enter` still submits

A new arm is added to the `handle_agent_mode` match block, placed **before** the
existing `KeyCode::Enter` arm so Rust's match ordering consumes it first:

```rust
KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
    self.agent_panel.input_newline();
}
```

Plain `Enter` behaviour (submit) is unchanged.

### `AgentPanel::input_newline`

A single-line method added alongside `input_char` and `input_backspace`:

```rust
pub fn input_newline(&mut self) { self.input.push('\n'); }
```

The cursor always sits at the end of the string, so appending is correct. Backspace
(which pops the last character) already handles removing a trailing newline — no
special case required.

### Paste preserves newlines in Agent mode

`handle_paste` previously collapsed `\n` and `\r` to spaces:

```rust
// before
let single_line = text.replace("\r\n", " ").replace('\r', " ").replace('\n', " ");
```

This is replaced with the same normalisation used in Insert mode:

```rust
// after
let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
for ch in normalised.chars() {
    self.agent_panel.input_char(ch);
}
```

Multi-line clipboard content is now preserved. Submission still requires an explicit
`Enter` keypress; paste alone cannot trigger a submit.

### Height calculation accounts for explicit newlines

The old formula treated the input as a single logical line:

```
wrapped_lines = ceil(char_count / content_width)
```

The new formula splits on `\n` first, computes wrapped display rows per logical line
(with a minimum of 1 row per line even when empty), then sums:

```rust
let explicit_lines: Vec<&str> = panel.input.split('\n').collect();
let total_wrapped: usize = explicit_lines.iter().enumerate().map(|(i, line)| {
    let len = line.chars().count()
        + if i == explicit_lines.len() - 1 { 1 } else { 0 }; // cursor on last line
    if content_width > 0 { ((len + content_width - 1) / content_width).max(1) } else { 1 }
}).sum();
```

The per-line `.max(1)` ensures that an empty line (produced by two consecutive
`Alt+Enter` presses) still occupies one display row.

### Max height raised from 5 → 10 lines

The original cap of 5 text lines was chosen to keep chat history visible for
single-sentence prompts. Structured multi-line prompts need more room; 10 lines
(12 rows including borders) is a reasonable upper bound that still leaves the chat
history area visible at typical terminal heights (≥ 40 rows).

### Hint text updated

The input box border title is updated to surface the new key:

```
" Ask Copilot… (Enter=send, Alt+Enter=newline, Ctrl+T=model, Tab=back) "
```

---

## Alternatives considered

**`Ctrl+Enter` instead of `Alt+Enter`**

`Ctrl+Enter` is not reliably distinguishable from plain `Enter` in most terminals
(both produce byte `0x0D`). `Alt+Enter` is sent as `ESC` + `CR` (or as an
xterm-style `\e[27;3;13~` sequence in Kitty), which crossterm correctly maps to
`KeyCode::Enter` with `KeyModifiers::ALT`. macOS Terminal, iTerm2, WezTerm, and
Kitty all support it.

**Full cursor-position tracking (left/right navigation, multi-line editing)**

A proper multi-line editor inside the input box would require tracking a 2-D cursor
position, adjusting backspace to delete the character at the cursor rather than the
last character, and remapping Up/Down arrows (which currently scroll chat history).
This is significant complexity. The append-only model — type to the right, backspace
from the right — is sufficient for prompt composition and is already well understood
by users of search bars and command lines.

**`\n` sentinel replaced by a `Vec<String>` input buffer**

Storing lines as a vector avoids scanning for `\n` at render time. However, it
complicates every call site that currently reads `panel.input` as a plain `&str`
(submit context assembly, the hint-visibility check `panel.input.is_empty()`, the
apply-diff trigger). A single `String` with embedded newlines keeps the diff minimal
and preserves the existing API surface.

---

## Consequences

**Positive**
- Users can write structured, multi-paragraph prompts directly in the panel.
- Paste from a text editor or terminal now preserves formatting.
- The input box height tracks the actual displayed content correctly, even after
  paste operations that introduce `\n`.
- Backspace at the start of a newly-inserted line removes the newline and merges
  back to the previous line — no special handling needed.
- No new mode or state machine entry is required.

**Negative / trade-offs**
- `Alt+Enter` is not available in all terminals (notably some SSH clients and older
  `xterm` configurations). Users in those environments must rely on paste to insert
  newlines.
- The input box can now grow up to 12 rows, reducing visible chat history in short
  terminals. The 10-line cap limits the impact.
- The height calculation is now O(n) in the number of explicit lines rather than O(1),
  but n is capped at 10 and the string is always short, so this is negligible.
