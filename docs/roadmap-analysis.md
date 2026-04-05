# Forgiven IDE — Gap Analysis vs 2026 AI-IDE Landscape

> Generated: 2026-04-04
> Source: Cross-reference of Forgiven README (99 ADRs, v0.8.9-alpha) against Cursor, Zed, Windsurf, Kiro, Neovim, Helix, and JetBrains feature sets.
> Purpose: Living roadmap document — iterate with Claude Code to spec and implement.

---

## Status key

- ❌ Missing — not implemented
- 🟡 Partial — foundation exists but incomplete
- ✅ Done — shipped

## Complexity key

- **1** — Days (isolated change, no new subsystems)
- **2** — 1–2 weeks (new module, bounded scope)
- **3** — Weeks (new subsystem, cross-cutting)
- **4** — Month+ (new architecture, protocol work)
- **5** — Quarter+ (foundational rewrite or novel R&D)

---

## Gap table (ranked by complexity, low → high)

### Complexity 1 — Quick wins

| # | Feature | Status | Description | Who has it | Notes |
|---|---------|--------|-------------|------------|-------|
| 1 | Tree-sitter text objects | ✅ | `vif`, `vaf`, `via`, `vaa` etc. — select/delete/yank by AST node (function, class, parameter, block). Requires Tree-sitter integration replacing syntect for parsing. | Neovim, Zed, Helix | ADR 0104 (core) + ADR 0105 (text objects). Shipped in v0.8.9-alpha. |
| 2 | Sticky scroll / context header | ✅ | Pin the enclosing function/class/scope name at the top of the viewport when scrolled deep into a file. | VS Code, Neovim (treesitter-context), Zed | ADR 0107. Uses `ancestor_matching` from ADR 0105. 1-line dim overlay, zero LSP latency. |
| 3 | Code folding (AST-based) | ✅ | Collapse/expand functions, blocks, imports, comments. Tree-sitter fold queries are the standard. | All major editors | ADR 0106. `za` toggle, `zM` close all, `zR` open all. Fold stubs show `··· N lines`. |
| 4 | Surround operations | ✅ | `cs{from}{to}` change surrounding, `ds{ch}` delete surrounding, `ys{ch}` surround word. Pure keymap + buffer ops. | Neovim (nvim-surround), Helix, Zed | ADR 0110. Single-line scope. Shipped in v0.8.9-alpha. |

### Complexity 2 — Medium effort

| # | Feature | Status | Description | Who has it | Notes |
|---|---------|--------|-------------|------------|-------|
| 5 | Multi-cursor editing | 🚫 | Intentionally excluded. See [ADR 0108](adr/0108-no-multi-cursor.md). | VS Code, Zed, Cursor, Helix | Counter to AI-first philosophy; use the agent for multi-site edits. Invasive refactor with no return value in this usage model. |
| 6 | Inline assistant (selection transform) | ✅ | Select code → open mini-prompt → AI rewrites selection in-place. Different from agent panel — fast, contextual, no conversation history. | Zed, Cursor (Cmd+K), Windsurf | ADR 0111. `SPC a i`. Input → Generating → Preview phases. Accept=Enter, Cancel=Esc. Shipped. |
| 7 | Multi-provider LLM backend | 🟡 | Copilot + Ollama (ADR 0098) exist. Missing: direct Anthropic API, OpenAI API, Google Gemini, OpenRouter. | Zed, Cursor, Continue | Abstract the agent backend behind a `Provider` trait. Each provider implements `stream_chat()`. Config selects provider + model. Copilot becomes one provider among many. |
| 10 | Agent checkpoints / session undo | 🟡 | Diff overlay (Ctrl+A) exists per-block. Missing: snapshot entire project state before agent session, one-click revert of all changes. | Zed, Cursor, Windsurf | Before first tool call in a session, `git stash` or snapshot all modified buffers. Track which files the agent touched. Provide `SPC a u` to revert entire session. |

### Complexity 3 — High effort

| # | Feature | Status | Description | Who has it | Notes |
|---|---------|--------|-------------|------------|-------|
| 8 | DAP debugger integration | ❌ | Debug Adapter Protocol — set breakpoints, step through code, inspect variables, watch expressions. | Zed, VS Code, Neovim (nvim-dap), JetBrains | New subsystem: DAP client over stdio/TCP. UI: breakpoint gutter marks, variables panel, call stack panel, step controls in status bar. Zed shipped this in one quarter. |
| 9 | Integrated terminal | 🚫 | Intentionally excluded. See [ADR 0109](adr/0109-no-integrated-terminal.md). | VS Code, Zed, Cursor, Neovim | Not applicable: forgiven runs inside a terminal — the shell is already present. PTY emulator would violate `unsafe_code = "forbid"`. Agent panel streams tool output inline. |
| 12 | Agent hooks / background automation | ❌ | Trigger agents on events: file save, test failure, lint error. Auto-generate tests, docs, or fixes without explicit chat. | Kiro, Windsurf | Config-driven: `[[agent.hooks]]` with `trigger = "on_save"`, `prompt = "..."`, `glob = "*.rs"`. File watcher (already have `notify`) dispatches to agent. Needs rate limiting and user visibility. |
| 13 | Multi-file review / change set view | 🟡 | Current: Ctrl+A diff overlay targets one file. Missing: unified view of all agent edits across multiple files with per-hunk accept/reject. | Zed, Cursor | New mode: `ReviewChanges`. Collects all files modified by the agent session. Renders a scrollable multi-buffer diff with `y`/`n` per hunk and `Y`/`N` for all. |

### Complexity 4 — Very high effort

| # | Feature | Status | Description | Who has it | Notes |
|---|---------|--------|-------------|------------|-------|
| 11 | Parallel sub-agents | ❌ | Spawn multiple AI workers in parallel for independent tasks. Main agent delegates, sub-agents report back. | Zed (spawn_agent, v0.227), Cursor | Requires agent task isolation (separate conversation contexts), a dispatcher/orchestrator, and UI to show multiple concurrent agent streams. |
| 14 | Edit prediction (next-edit suggestion) | ❌ | Predict the developer's next *edit* (not just next token). If you rename one field, suggest renaming all matching fields. | Zed (Zeta2), Cursor | Requires a fine-tuned model or integration with an edit-prediction service. Fundamentally different from ghost-text completion — operates on edit deltas, not insertions. |
| 16 | ACP (Agent Client Protocol) | ❌ | Open protocol (Zed + JetBrains, Jan 2026) letting external CLI agents (Claude Code, Codex, Gemini CLI) run inside the editor natively. | Zed, JetBrains | Protocol implementation: the editor becomes an ACP host. External agents connect and use editor tools (read file, edit file, run terminal). Significant protocol + security work. |

### Complexity 5 — Extreme effort

| # | Feature | Status | Description | Who has it | Notes |
|---|---------|--------|-------------|------------|-------|
| 15 | Real-time collaboration (CRDT) | ❌ | Google Docs-style co-editing with cursor presence and conflict-free merge. | Zed | CRDT data structure for the buffer, network transport, presence UI, conflict resolution. Zed spent years on this. Only pursue if collaboration is a core goal. |

---

## What Forgiven already does well (competitive advantages)

These are areas where Forgiven is **ahead** of or **differentiated** from the competition:

| Strength | Detail |
|----------|--------|
| Context management | LLMLingua compression (ADR 0084/0088), importance-scored history (ADR 0081), per-segment token breakdowns (ADR 0099), context gauge, auto-compress tool results. More sophisticated than Cursor or Zed. |
| Spec-driven development | spec-kit integration (ADR 0056) with auto-clear context per phase (ADR 0097). Ahead of Kiro's approach in configurability. |
| Security posture | Zero telemetry, no background network calls, agent sandboxed to project root, `cargo-audit` + `cargo-deny` in CI, `unsafe` forbidden project-wide. Strongest in the space. |
| MCP ecosystem | Full MCP client (ADR 0045) with status visualisation (ADR 0048), env var secrets (ADR 0050), non-blocking startup (ADR 0053), memory server integration (ADR 0083). |
| Token awareness | Session rounds counter (ADR 0096), context bloat audit (ADR 0087), prompt caching tracking (ADR 0078), diff-only tool results (ADR 0079), tool call batching (ADR 0080). |

---

## Recommended implementation order

### Phase 1 — Foundation (Tree-sitter migration) ✅ COMPLETE
> Unlocked items 1, 2, 3. All shipped.

1. ✅ Integrate `tree-sitter` crate alongside `syntect` (ADR 0104)
2. ✅ Tree-sitter text objects `vif`, `vaf`, `via`, `daf`, etc. (ADR 0105)
3. ✅ Code folding `za`, `zM`, `zR` (ADR 0106)
4. ✅ Sticky scroll context header (ADR 0107)

### Phase 2 — Editor polish ✅ COMPLETE
5. ✅ Surround operations `ds{ch}`, `cs{from}{to}`, `ys{ch}` (ADR 0110)
6. 🚫 Multi-cursor editing — excluded by design (ADR 0108)
7. 🚫 Integrated terminal pane — excluded by design (ADR 0109)

### Phase 3 — AI interaction model
8. ✅ Inline assistant (selection → prompt → rewrite) — ADR 0111
9. Multi-provider LLM backend (Anthropic, OpenAI, Gemini direct)
10. Agent checkpoints with session-level undo
11. Multi-file review / change set view

### Phase 4 — Advanced AI
12. Agent hooks (event-driven automation)
13. Parallel sub-agents
14. ACP host implementation

### Phase 5 — Frontier (evaluate need)
15. Edit prediction
16. Real-time collaboration (CRDT)

---

## How to use this document

This file lives in the Forgiven repo root. When working with Claude Code:

1. Reference this file for context: `@ROADMAP-GAP-ANALYSIS.md`
2. Pick an item and say: "Spec out item N as an ADR"
3. Claude Code will create `docs/adr/NNNN-<feature>.md` following the existing ADR format
4. Implement, test, update the status in this file from ❌ to ✅

---

*Last updated: 2026-04-05 by Claude (Phase 3, item 8 complete — ADR 0111 inline assistant shipped)*
