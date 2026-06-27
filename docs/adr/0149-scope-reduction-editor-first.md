# ADR 0149 — Scope Reduction: Editor-First Identity

**Date:** 2026-06-27
**Status:** Accepted

---

## Context

By mid-2026 forgiven had accumulated the following subsystems beyond its core
modal editor:

| Subsystem | LOC | Justification at the time |
|-----------|----:|---------------------------|
| Agent panel (chat + agentic loop) | ~3 500 | In-editor AI chat |
| MCP client (stdio + HTTP/SSE) | 1 059 | Tool ecosystem |
| Nexus UDS sidecar | 203 | IPC to Tauri companion |
| Tauri companion window | ~2 500 | Rich Markdown/Mermaid preview |
| Spec framework | 735 | ADR-driven feature planning |
| Insights dashboard | 1 495 | Codebase health narrative |
| Debt dashboard | 1 128 | Tech-debt metrics on welcome screen |
| Codified context | ~400 | Constitution/specialist/knowledge files |
| Intent translator | ~300 | LLM-driven keybinding suggestions |
| Session log (JSONL) | ~350 | Agent conversation persistence |
| Hooks system | ~280 | on_save / on_test_fail agent triggers |
| 6 extra LLM providers | ~600 | Anthropic, OpenAI, Gemini, OpenRouter, DeepSeek, LmStudio |

Total added surface: ~12 500 LOC across 14 subsystems.

The result: `src/agent/` (10 509 LOC) had grown larger than `src/editor/`
(9 783 LOC). The agent panel drove more of the codebase than the editor itself.

### The ecosystem argument

Terminal-first developers already have purpose-built AI tools: Claude Code,
Cursor, Zed, Continue. These tools operate on files and are invoked outside the
editor. forgiven's agent panel did not compose with them — it competed with them,
poorly. Neovim, Helix, and Kakoune deliberately avoid built-in AI panels for
this reason.

### The real gap

Helix users have requested native ghost-text completions in
[#8887](https://github.com/helix-editor/helix/issues/8887),
[#10131](https://github.com/helix-editor/helix/issues/10131), and
[#11824](https://github.com/helix-editor/helix/issues/11824). Helix has refused
on philosophical grounds. Neovim requires plugin setup. forgiven can offer this
with zero configuration.

The two features that genuinely require editor integration are:

1. **Ghost-text completions** — inline multi-token suggestions that appear as
   the user types, accepted with Tab (requires cursor position, language context,
   and per-keystroke timing that an external tool cannot replicate).
2. **Inline selection transform** (`SPC a i`) — stream an LLM rewrite of the
   selected region directly into the buffer with Accept/Reject preview.

Everything else can be done better outside the editor.

---

## Decision

Strip forgiven back to its core identity: **a fast, opinionated modal TUI editor
with native AI completions**.

### Removed

- `src/agent/panel.rs` — AgentPanel struct and all chat-panel state
- `src/agent/agentic_loop.rs` — multi-round tool-calling loop
- `src/agent/tools.rs` + `tool_dispatch.rs` — agent tool definitions
- `src/agent/conversation.rs` + `context.rs` — chat history and janitor
- `src/agent/session_log.rs` — JSONL conversation persistence
- `src/agent/stream_poll.rs` + `streaming.rs` — agent SSE polling
- `src/agent/project_tree.rs` — project tree injection into agent context
- `src/agent/token_count.rs` — per-segment token tracking
- `src/agent/codified_context.rs` — constitution/specialist/knowledge files
- `src/agent/intent.rs` — intent translator
- `src/agent/models.rs` — model-selection heuristics
- `src/agent/provider/{anthropic,openai,gemini,openrouter,deepseek,lmstudio}.rs`
  — six extra provider implementations
- `src/mcp/` — MCP client (stdio + HTTP/SSE transports)
- `src/sidecar/` — Nexus UDS IPC server
- `src/graphics/` — terminal image protocol detection stub
- `src/insights/` — codebase health dashboard
- `src/debt/` — tech-debt metrics and welcome-screen panel
- `src/spec_framework/` — ADR-driven feature planning
- `src/editor/hooks.rs` — on_save / on_test_fail agent hooks
- `src/ui/agent_panel.rs` — agent panel rendering
- `src/ui/popups.rs` — agent-panel-specific popups
- `companion/` — Tauri companion window and Nexus client (supersedes ADR 0148)

### Kept

| Feature | Module |
|---------|--------|
| Modal editing (Normal/Insert/Visual/Command) | `src/editor/` |
| LSP (rust-analyzer, copilot-ls, csharp-ls) | `src/lsp/` |
| Ghost-text completions — Copilot + Ollama | `src/editor/ai.rs` |
| Inline selection transform (`SPC a i`) | `src/editor/inline_assist.rs` |
| Tree-sitter AST + syntect highlighting | `src/treesitter/`, `src/highlight/` |
| File explorer + ripgrep search | `src/explorer/`, `src/search/` |
| Markdown preview | `src/markdown/`, `src/ui/markdown.rs` |

Provider support slimmed to **Copilot** (default, OAuth) and **Ollama** (local,
no auth) — the two that need no external account beyond what a developer already
has.

### Build / release cleanup

- `release.yml`: removed Node setup, `npm install`, and Tauri build steps; the
  macOS DMG now contains only the `forgiven` universal binary.
- `Makefile`: removed `companion` target; `install` now installs one binary.
- `Cargo.toml`: removed `resvg`, `ratatui-image`, and `tiny-skia` (graphics
  stub dependencies — fixed RUSTSEC-2026-0186, unsound `memmap2`).
- `Cargo.lock`: updated `quinn-proto` to 0.11.15 (fixed RUSTSEC-2026-0185,
  remote memory exhaustion).

---

## Consequences

**Positive**

- `src/agent/` is now a minimal shim: `StreamEvent`, Copilot auth, two-provider
  `ProviderKind`, and a standalone `start_inline_assist()` function. Under 500 LOC.
- Total codebase shrinks from ~37 000 LOC to ~24 000 LOC (~35% reduction).
- The editor binary has no runtime dependency on Node, npm, or a running Tauri
  process.
- Two active security advisories resolved as a side effect of dependency removal.
- `make check` (fmt + lint + audit + deny + 120 tests) passes clean.
- Onboarding complexity drops: there is one binary, one config file, and two
  provider choices.

**Negative / Trade-offs**

- Users who relied on the agent chat panel lose it. They should use Claude Code,
  Cursor, or another purpose-built AI tool alongside forgiven.
- The MCP ecosystem (tool calling, external context sources) is no longer
  accessible from within the editor.
- The Tauri companion window (rich Markdown/Mermaid rendering) is gone. Markdown
  preview remains, rendered in-terminal.
- Session history and conversation persistence are gone — each inline assist is
  stateless.

**Intentionally excluded** (see also ADR 0001, ADR 0042)

- Multi-cursor — still excluded.
- Integrated terminal — still excluded.
- Built-in AI chat panel — now explicitly excluded. External tools compose
  better.
