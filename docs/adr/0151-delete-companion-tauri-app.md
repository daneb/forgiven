# ADR 0151 — Delete Companion Tauri App

**Status:** Accepted  
**Date:** 2026-06-27

---

## Context

The Tauri companion window (`companion/`) was built to render rich Markdown and Mermaid
diagrams in a borderless floating window alongside the terminal editor. It connected to the
editor via the Nexus UDS sidecar (`src/sidecar/`), which broadcast buffer-update, cursor-move,
mode-change, and shutdown events over a Unix domain socket.

ADR 0149 removed the Nexus sidecar from the editor binary. ADR 0150 removed all remaining
AI functionality, including the `CompanionToggle` action. At that point the companion app had
no IPC layer to connect to and no purpose: it could no longer receive events from the editor
and the only meaningful rendering it performed (Mermaid diagrams) is now covered by
`SPC p b`, which opens the current markdown buffer in the system browser using the
`pulldown-cmark` + mermaid.js CDN path.

The `companion/` directory has remained on disk through two scope-reduction commits as dead
weight — ~6 000 lines of Tauri/Rust/JS that cannot be built into a useful binary.

---

## Decision

Delete `companion/` entirely. Update the `Makefile` so `make install` installs only the
`forgiven` binary. Remove companion references from `CLAUDE.md` and `.gitignore` (add
`.claude/worktrees/` and `.forgiven/` while there).

---

## Consequences

- **`make companion`** no longer exists. Anyone who has it in a script will get a "no rule
  to make target" error; update scripts to drop that step.
- **`make install`** now installs one binary (`forgiven`) instead of two.
- **`~/.local/bin/forgiven-companion`** will not be installed by future `make install` runs.
  Existing copies on disk are inert and can be deleted manually.
- `SPC p b` (open markdown in browser) remains the supported path for rendered Mermaid output.
