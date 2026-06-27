# ADR 0149 — Scope Reduction: Strip Agent Panel, MCP, Sidecar, and Peripheral Features

**Status:** Accepted  
**Date:** 2026-06-26

---

## Context

Over the course of development the editor accumulated a large surface of aspirational features:
a full agent chat panel with multi-round tool calling, an MCP client, a Nexus UDS sidecar,
a Tauri companion window, a spec framework (SpecKit → OpenSpec), an insights dashboard,
a debt dashboard, codified context loading, an intent translator, agent hooks, session
checkpoints, and four additional LLM provider backends.

These features added roughly 7 500 LOC, two open security advisories
(RUSTSEC-2026-0185 — quinn-proto memory exhaustion; RUSTSEC-2026-0186 — memmap2 unsound via
ratatui-image / resvg), and ongoing maintenance drag disproportionate to their actual use.
The editor's core value — fast, auditable, terminal-native editing with lightweight AI
assistance — was becoming obscured by the surrounding scaffolding.

---

## Decision

Remove the following features, retaining only ghost-text inline completions and the
single-turn inline assistant as the AI integration points:

| Removed feature | Key symbols |
|-----------------|-------------|
| Full agent chat panel (streaming SSE, multi-round tool loop, session log, checkpoints) | `agent/panel.rs`, `agentic_loop.rs`, `tools.rs`, `tool_dispatch.rs` |
| MCP client (stdio + HTTP/SSE transports) | `src/mcp/` |
| Nexus UDS sidecar | `sidecar/server.rs` wired into the editor |
| Tauri companion window integration | `CompanionToggle` action wired to editor |
| Spec framework (SpecKit / OpenSpec) | `src/spec_framework/` |
| Insights dashboard (collaboration analytics) | `src/insights/` |
| Debt dashboard (welcome-screen metrics) | `src/debt/` |
| Codified context three-tier loader | `agent/codified_context.rs` |
| Intent translator | `agent/intent.rs` wired into submit path |
| Agent hooks (`on_save`, `on_test_fail`) | `editor/hooks.rs` body |
| LLM providers: Anthropic, OpenAI, Gemini, OpenRouter | `agent/provider/` (bodies stripped) |
| LLMLingua tool-result compression | `auto_compress_tool_results` config key |
| Multi-file review / change-set view | `ReviewChanges` mode |
| Investigation subagent | `AgentInvestigate` action |
| Memory save to MCP knowledge graph | `MemorySave` action |
| Auto-janitor rolling compression | `AgentJanitorCompress` action |
| Agent session revert | `AgentSessionRevert` action |

**Retained AI surface:**

| Feature | Entry point |
|---------|-------------|
| Ghost-text inline completions | Copilot LSP / Ollama; `Tab` to accept |
| Inline assistant (single-turn rewrite) | `SPC a i` — selection → prompt → streamed rewrite |
| Git commit message generation | `SPC g s` / `SPC g l` — one-shot via Copilot or Ollama |
| Release notes generation | `SPC g n` — one-shot via Copilot or Ollama |

Keybindings for removed features remain defined in `src/keymap/mod.rs` and produce a
"removed in slim build" status message. They are not removed from the keymap enum to
keep the dispatch table exhaustive and avoid a large mechanical churn on types.

---

## Consequences

- **LOC**: ~7 500 lines removed.
- **Security**: RUSTSEC-2026-0185 (quinn-proto) resolved by updating to 0.11.15;
  RUSTSEC-2026-0186 (memmap2 via ratatui-image / resvg) resolved by removing the graphics
  pipeline that depended on it.
- **Build**: `make check` passes — fmt, clippy (zero warnings), audit (zero advisories),
  deny (zero violations), 120 tests.
- **Providers**: Only `copilot` and `ollama` remain. Config keys for the removed providers
  are rejected at parse time.
- **Surface area**: All remaining AI calls are single-turn, synchronous from the user's
  perspective, and require an explicit user action. No persistent network state, no
  background polling.
- **Companion directory**: `companion/` source tree is retained on disk but is not wired
  into the editor binary. `make companion` and `make install` still build it if Node is
  available, but the resulting binary does nothing useful since the Nexus IPC layer no
  longer broadcasts events.
