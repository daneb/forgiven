# ADR 0108 — Non-Decision: Multi-Cursor Editing

**Date:** 2026-04-05
**Status:** Rejected

---

## Context

Multi-cursor editing (placing multiple cursors and editing them simultaneously) is a
standard feature of VS Code, Zed, Cursor, and Helix. It is typically used for:

- Renaming a symbol across a small scope
- Editing repetitive patterns that differ only in a small variable part
- Simultaneous structural edits to multiple similar lines

The roadmap gap analysis (item 6) rated this as Complexity 2 — approximately 1–2 weeks
to implement correctly.

---

## Decision

**Multi-cursor editing will not be implemented in forgiven.**

---

## Rationale

### 1. Counter to the AI-first editing model

Forgiven's design philosophy (see README) treats code as a black box. Editing is
performed by the agent. Multi-cursor is an optimisation for *manual* editing throughput
— the exact class of activity forgiven is designed to minimise. Adding it would serve
a workflow the editor deliberately does not optimise for.

### 2. The agent handles the same use cases, better

Every canonical multi-cursor use case is better handled by the agent:

| Multi-cursor use case | Agent equivalent |
|-----------------------|-----------------|
| Rename symbol in file | "Rename all uses of X to Y" |
| Edit N similar lines  | Select + describe the transformation |
| Fix repetitive pattern| Describe the invariant; apply everywhere |

The agent understands intent, handles edge cases, and applies changes across the whole
project — not just the visible viewport.

### 3. Invasive refactor with no return value

Implementing multi-cursor correctly requires changing `Buffer`'s single `Cursor` field
to `Vec<Cursor>`, then threading that change through every edit operation, the
renderer, selection logic, undo/redo snapshots, fold-aware row computation, and sticky
scroll offset calculations. This is a wide, cross-cutting refactor that touches the
most frequently modified paths in the codebase — all to serve a workflow the editor
philosophy explicitly deprioritises.

### 4. Lightweight posture

Each feature added to forgiven increases the binary size, the test surface, and the
cognitive overhead of future changes. Multi-cursor provides no value within forgiven's
usage model and its implementation cost is disproportionate to that value.

---

## Consequences

- The codebase keeps a single `Cursor` per buffer. All edit operations remain simple.
- Users wanting multi-site edits use the agent panel.
- This decision is recorded here so it is not revisited without a concrete,
  compelling reason that changes the underlying assumptions above.

---

## Related

| Item | Relation |
|------|----------|
| README — Design Philosophy | Foundational principles behind this decision |
| [ADR 0109](0109-no-integrated-terminal.md) | Companion non-decision |
| [Roadmap gap analysis](../../docs/roadmap-analysis.md) | Item 6 |
