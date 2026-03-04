# ADR 0041 — Agent Panel Copy Code Block (`c` key)

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

2. **Handle `c` (empty input) in Agent mode** in the editor key handler:
   - Call `extract_code_blocks()` on the last assistant reply.
   - If no blocks exist, show `"No code blocks in last reply"` in the status
     bar.
   - Otherwise copy block at `code_block_idx % len` to the system clipboard,
     show `"Code block N/Total copied"`, then advance the index (wrapping
     around).

3. **Keep `y` unchanged** — it continues to copy the full raw reply for users
   who want the complete markdown text.

## Consequences

- Pressing `c` once in Agent mode copies the first (and usually only) code
  block — clean, runnable text with no markdown artefacts.
- Pressing `c` again cycles to the next block, then wraps back to the first,
  letting users extract multiple blocks from a single response without a mouse.
- The index resets automatically when a new reply arrives, so the first `c`
  after each response always starts at block 1.
- No UI or rendering changes were required; the feature is entirely driven by
  the already-parsed raw message content.
