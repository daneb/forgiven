# ADR 0041 — Agent Panel Copy Code Block (`Ctrl+K`) and Yank Reply (`Ctrl+Y`)

**Date:** 2026-03-04
**Status:** Accepted

## Context

When the Copilot agent panel returns a response containing one or more fenced
code blocks, the only way to copy content to the system clipboard was the `y`
key, which copies the entire last assistant reply as raw markdown.  The raw
reply includes prose, thinking blocks, markdown fences, and gutter characters
(`│`), making it unusable for directly running code in a terminal.

Code blocks are already parsed by `AgentPanel::extract_code_blocks()`, which
strips fences and returns clean `Vec<String>` of block bodies.  The system
clipboard (`arboard`) is already integrated.  No new infrastructure was needed.

## Decision

1. **Add `code_block_idx: usize`** to `AgentPanel` (initialized to `0`, reset
   to `0` on `StreamEvent::Done` so the index always starts fresh for each new
   reply).

2. **Handle `Ctrl+K` in Agent mode** in the editor key handler:
   - Call `extract_code_blocks()` on the last assistant reply.
   - If no blocks exist, show `"No code blocks in last reply"` in the status
     bar.
   - Otherwise copy block at `code_block_idx % len` to the system clipboard,
     show `"Code block N/Total copied  (Ctrl+K for next)"`, then advance the
     index (wrapping around).

3. **Handle `Ctrl+Y` in Agent mode** — copies the full raw last reply
   (prose, fences, and all) for users who want the complete markdown text.

## Consequences

- Pressing `Ctrl+K` once in Agent mode copies the first (and usually only)
  code block — clean, runnable text with no markdown artefacts.
- Pressing `Ctrl+K` again cycles to the next block, then wraps back to the
  first, letting users extract multiple blocks from a single response without
  a mouse.
- The index resets automatically when a new reply arrives, so the first
  `Ctrl+K` after each response always starts at block 1.
- No UI or rendering changes were required; the feature is entirely driven by
  the already-parsed raw message content.

---

## Amendment — 2026-03-13

The trigger keys were changed from bare single-letter shortcuts to
`Ctrl`-modifier chords:

- `c` (empty input only) → **`Ctrl+K`** (copy / cycle code blocks)
- `y` (empty input only) → **`Ctrl+Y`** (yank full reply)

**Reason:** bare single-letter shortcuts that only fire on an empty input box
intercept the first character of any new message starting with that letter
(e.g. "copy this to a file", "can you explain…").  Moving to `Ctrl+K` /
`Ctrl+Y` eliminates the accidental-trigger class of bugs while keeping
single-chord shortcuts.  `Ctrl+A` (apply diff, ADR 0035) was migrated for the
same reason.
