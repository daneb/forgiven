# ADR 0109 — Non-Decision: Integrated Terminal Pane

**Date:** 2026-04-05
**Status:** Rejected

---

## Context

VS Code, Zed, Cursor, and JetBrains all embed a terminal pane alongside the editor.
The primary value propositions are:

1. Run build commands and tests without leaving the editor window.
2. See command output inline next to the code being worked on.
3. Avoid context switching to a separate terminal application.

The roadmap gap analysis (item 7) rated this as Complexity 3 — weeks of effort. It
would require PTY allocation (a C-level system call), a VT100/ANSI state machine
(the `vte` crate), and a new ratatui pane capable of rendering a terminal grid.

---

## Decision

**An integrated terminal pane will not be implemented in forgiven.**

---

## Rationale

### 1. The problem is already solved

Forgiven is a TUI application that runs *inside* a terminal. The terminal is already
present. Embedding a second terminal inside the first is circular — it solves a problem
that does not exist here.

GUI editors (VS Code, Zed) need an embedded terminal because there is no shell context
around them. A terminal multiplexer is required to split screen. Forgiven users are, by
definition, already running in a terminal with a shell. Switching to a second pane
costs a single key chord in `tmux` or `zellij`.

### 2. Agent output is already inline

The agent panel streams tool output — including the results of shell commands executed
via MCP tool servers — directly into the conversation. There is no need to switch
contexts to see the result of a command the agent ran. This covers the most common
reason to want "see output next to the code".

### 3. Technical cost vs. return

A correct terminal emulator is a substantial engineering undertaking:

- PTY allocation requires `openpty(2)` — unsafe-adjacent C FFI, in direct conflict
  with forgiven's `unsafe_code = "forbid"` project lint.
- Full VT100/ANSI emulation (`vte` crate) is non-trivial to integrate correctly
  with ratatui's cell-grid renderer.
- Resize handling, scroll-back buffer, mouse passthrough, colour palette mapping
  — each a separate subsystem.

The implementation effort (weeks) is disproportionate to the value (none, given
the shell is already available outside the editor).

### 4. Lightweight posture

Adding a terminal emulator would be the single largest dependency surface added to
forgiven. It would increase binary size, compile time, and maintenance burden
permanently, for a feature that is available for free in the surrounding environment.

---

## Consequences

- No terminal pane. Users run commands in their existing shell (split with `tmux`,
  `zellij`, or a second terminal tab).
- Agent-executed commands stream output to the agent panel, covering the inline-output
  use case.
- The `unsafe_code = "forbid"` lint remains intact.
- This decision is recorded here so it is not revisited without a concrete, compelling
  reason that changes the underlying assumptions above.

---

## Related

| Item | Relation |
|------|----------|
| README — Design Philosophy | Principle 3: terminal-native, not a GUI in a terminal |
| [ADR 0108](0108-no-multi-cursor.md) | Companion non-decision |
| [ADR 0011](0011-agentic-tool-calling-loop.md) | Agent tool loop — inline command output |
| [Roadmap gap analysis](../../docs/roadmap-analysis.md) | Item 7 |
