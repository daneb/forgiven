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
| 7 | Multi-provider LLM backend | ✅ | Anthropic, OpenAI, Gemini, OpenRouter added alongside Copilot + Ollama. Zero SSE parser changes — all six providers use the same streaming code path. API keys via `$VAR` env expansion. | Zed, Cursor, Continue | ADR 0116. Six `ProviderKind` variants; `ProviderSettings` carries all HTTP params. Model discovery per provider. UI colours per provider. |
| 10 | Agent checkpoints / session undo | ✅ | `SPC a u` reverts all agent-modified files and deletes agent-created files. `session_snapshots` + `session_created_files` track all changes. Status message shows counts. | Zed, Cursor, Windsurf | ADR 0112. `revert_session()` in panel.rs. `FileCreated` StreamEvent tracks new files for deletion on revert. |

### Complexity 3 — High effort

| # | Feature | Status | Description | Who has it | Notes |
|---|---------|--------|-------------|------------|-------|
| 8 | DAP debugger integration | ❌ | Debug Adapter Protocol — set breakpoints, step through code, inspect variables, watch expressions. | Zed, VS Code, Neovim (nvim-dap), JetBrains | New subsystem: DAP client over stdio/TCP. UI: breakpoint gutter marks, variables panel, call stack panel, step controls in status bar. Zed shipped this in one quarter. |
| 9 | Integrated terminal | 🚫 | Intentionally excluded. See [ADR 0109](adr/0109-no-integrated-terminal.md). | VS Code, Zed, Cursor, Neovim | Not applicable: forgiven runs inside a terminal — the shell is already present. PTY emulator would violate `unsafe_code = "forbid"`. Agent panel streams tool output inline. |
| 12 | Agent hooks / background automation | ✅ | `on_save` and `on_test_fail` triggers. Config: `[[agent.hooks]]` + `[agent.test]`. Auto-detects test framework. Pass→fail transition fires hook with `{output}` placeholder. 30 s cooldown. Re-entry guard prevents loops. | Kiro, Windsurf | ADR 0114. `fire_hooks_for_save()` + `run_tests_if_configured()` + `fire_hooks_for_test_fail()` in editor/hooks.rs. |
| 13 | Multi-file review / change set view | ✅ | `SPC a r` opens unified multi-file diff. `y`/`n` per file, `Y`/`N` all, `Tab`/`Shift+Tab` hunk navigation, `a`/`r` per-hunk accept/reject. New files included. | Zed, Cursor | ADR 0113. `ReviewChangesState` + `apply_hunk_verdicts()` in editor/mod.rs. Partial file writes preserve accepted hunks. |

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
| Spec-driven development | OpenSpec integration (ADR 0139) — three-command workflow (`propose`/`review`/`apply`); brownfield-first, auto-clears context per phase. Replaced SpecKit (ADR 0056). |
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
9. ✅ Multi-provider LLM backend — Anthropic, OpenAI, Gemini, OpenRouter — ADR 0116
10. ✅ Agent checkpoints with session-level undo — ADR 0112
11. ✅ Multi-file review / change set view — ADR 0113

### Phase 4 — Advanced AI
12. ✅ Agent hooks (on_save + on_test_fail) — ADR 0114
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

*Last updated: 2026-04-06 by Claude (Phase 3 complete; Phase 4 item 12 complete; item 7 complete — all six providers shipped in ADR 0116)*
