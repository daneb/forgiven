# ADR 0081: Importance-Scored History Retention

**Date:** 2026-03-23
**Status:** Accepted

## Context

ADR 0077 introduced token-aware history truncation: walk messages newest-to-oldest, accumulate token estimates, stop when the budget is exceeded. This is purely recency-based — the oldest messages are always the first to be dropped.

In practice, some old messages are highly valuable (user instructions that define the task, error messages from failed tool calls) while some recent messages are low-value (large file reads the model has already acted on). Pure FIFO truncation loses the former before the latter.

## Decision

The truncation algorithm is replaced with a two-pass importance-scored approach:

**Phase 1 — Recency guarantee:** The 4 most recent non-system messages are always included unconditionally. This preserves immediate conversational context.

**Phase 2 — Importance-scored older messages:** For messages older than the recency window, each is scored by `message_importance()`:

| Condition | Score delta |
|-----------|-------------|
| User message (base) | +3 |
| Assistant message (base) | +2 |
| Contains "error" / "Error" / "failed" / "panic" | +3 |
| Large (>2 KB) line-numbered file read or batch result | -2 |

Candidates are sorted by score descending and greedily included into the remaining token budget (total budget minus recency tokens). This means a short old error message can survive pruning while a large old file-read is dropped.

Messages are re-emitted to the API in their original order so conversation coherence is preserved.

## Consequences

- Error context from earlier in a session survives longer than bulk file reads.
- User task-definition messages are the last to be dropped.
- The algorithm is O(n log n) vs the previous O(n) reverse walk, but n is bounded by the context window and the difference is negligible in practice.
- The MIN_RECENT=4 constant can be tuned without changing the algorithm.
