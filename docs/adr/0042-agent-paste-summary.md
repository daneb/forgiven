# ADR 0042 — Agent Panel Paste Summary

**Date:** 2026-03-05
**Status:** Accepted

## Context

When a user pasted multi-line content (code, logs, stack traces) into the
agent panel input box, the full text was inserted character-by-character into
`panel.input` via `handle_paste`.  This had two problems:

1. **Visual noise** — a large paste flooded the input box, making it hard to
   see what was already typed and obscuring the panel history.
2. **Height thrash** — the dynamic input box height expanded aggressively for
   long pastes (capped at 10 lines), squashing the chat history area.

Claude Code handles this well: pasted content is acknowledged with a compact
summary pill ("Pasted 12 lines") while the actual text is kept out of the
visible input and included verbatim when the message is sent.

## Decision

1. **Add `pasted_blocks: Vec<String>`** to `AgentPanel`.  Each bracketed-paste
   event in Agent mode appends one entry; the raw text is never pushed into
   `panel.input`.

2. **`handle_paste` (editor/mod.rs)** — in Agent mode, normalise line endings
   and push to `pasted_blocks` instead of iterating characters into the input
   string.  Insert-mode paste is unchanged.

3. **`send_message` (agent/mod.rs)** — before sending:
   - Empty guard now checks `pasted_blocks.is_empty()` in addition to
     `input.trim().is_empty()`.
   - `pasted_blocks` is drained with `mem::take`, blocks joined with `\n\n`,
     and any typed input appended after a blank line.  The combined string
     becomes `user_text` — the full content Copilot receives.

4. **UI rendering (ui/mod.rs)**:
   - Input box height adds `pasted_blocks.len()` rows for summary lines.
   - Before the typed input, one `⎘  Pasted N lines` summary span (Cyan + DIM)
     is rendered per block using a ratatui `Line` / `Span`.
   - The `[a] diff+apply` hint condition also checks `pasted_blocks.is_empty()`
     so it does not show while paste content is pending.

## Consequences

- Pasting a 200-line stack trace shows `⎘  Pasted 200 lines` — one line —
  keeping the input box compact and the history visible.
- Multiple pastes accumulate as separate summary lines; all are sent together
  with the next Enter.
- The full pasted text is always sent to Copilot unchanged; nothing is lost.
- Backspace still edits the typed input only; pasted blocks cannot be
  individually edited (they must be re-pasted if wrong), which is an
  acceptable trade-off for the UX simplification.
