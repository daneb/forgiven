# ADR 0150 — Remove All AI Functionality

**Status:** Accepted  
**Date:** 2026-06-27

---

## Context

ADR 0149 performed a first scope reduction that stripped the full agent panel, agentic loop,
MCP client, Nexus sidecar, and four LLM provider backends, leaving ghost-text completions,
an inline assist overlay, AI-powered commit-message and release-notes generation, Copilot
OAuth auth, and a full 8-provider configuration system.

That half-removal left the codebase in an awkward middle state: substantial AI plumbing
(`src/agent/`, `src/editor/ai.rs`, `src/editor/inline_assist.rs`, Copilot LSP wiring,
`reqwest`/`futures-util`/`tiktoken-rs` dependencies) with a much smaller surface of features
to justify it. The editor's value is as a fast, terminal-native Vim-modal editor with solid
LSP support — not as an AI interface.

---

## Decision

Remove every remaining AI integration point:

| Removed | Detail |
|---------|--------|
| Ghost-text completions | Copilot LSP inline completion, debounce polling, ghost text rendering |
| Inline assistant | `SPC a i` streaming rewrite overlay, `InlineAssistState`, `submit.rs` |
| Commit message generation | `SPC g s` / `SPC g l`, `CommitMsgState`, `CommitMsg` mode |
| Release notes generation | `SPC g n`, `ReleaseNotesState`, `ReleaseNotes` mode |
| Copilot OAuth auth | `src/agent/auth.rs`, Copilot LSP device-flow methods |
| Provider system | All 8 provider backends + `ProviderConfig` config block |
| Agent / hooks config | `AgentConfig`, `AgentHook`, `HooksConfig`, `IntentTranslatorConfig` |

**What is preserved:**

- `SPC p b` (open markdown in browser with Mermaid.js) — not AI; `open_markdown_in_browser()`
  moved to `src/editor/file_ops.rs`
- `SPC g g` (lazygit) — not AI
- All LSP functionality for user-configured servers (rust-analyzer, etc.)

---

## Changes

**Deleted entirely:**
`src/agent/`, `src/editor/ai.rs`, `src/editor/inline_assist.rs`, `src/editor/hooks.rs`,
`src/insights/`, `src/debt/`, `src/mcp/`, `src/sidecar/`, `src/spec_framework/`,
`src/copilot/`, `src/ui/agent_panel.rs`

**Edited:**
`src/keymap/mod.rs` — removed AI `Mode` and `Action` variants, removed `SPC a` subtree,
`SPC g s/l/n`, remaining stub keybindings.  
`src/editor/mod.rs` — removed ~15 AI fields from `Editor`.  
`src/editor/event_loop.rs` — removed all AI polling blocks.  
`src/editor/actions.rs` — removed all AI action dispatch arms.  
`src/editor/input.rs` / `mode_handlers.rs` — removed AI mode handlers.  
`src/editor/render.rs` — removed AI overlay rendering.  
`src/editor/state.rs` — removed AI state types.  
`src/ui/mod.rs` / `popups.rs` — removed AI UI structs and renderers.  
`src/lsp/mod.rs` — removed Copilot auth methods, `inline_completion`, Copilot server wiring.  
`src/config/mod.rs` — removed all provider and agent config structs.  
`src/main.rs` — removed `mod agent`, Ollama warmup.  
`Cargo.toml` — removed `reqwest`, `futures-util`, `tiktoken-rs`, `image`, `base64`.

---

## Consequences

- **LOC removed:** ~17 344 lines.
- **Dependencies removed:** `reqwest`, `futures-util`, `tiktoken-rs`, `image`, `base64`.
- **Network calls:** zero. The editor makes no outbound connections of any kind.
- **Build:** `make check` passes — fmt, clippy (zero warnings), audit (zero advisories),
  deny (zero violations), 72 tests.
- **Config:** `[provider]` and `[agent]` TOML sections are no longer recognised. Existing
  config files with those sections will log a warning on startup (TOML unknown-field).
- **`copilot-language-server`:** no longer started or used, even if installed.
