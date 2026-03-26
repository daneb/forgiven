# ADR 0089 — Large File Split: editor, agent, and ui modules

**Date:** 2026-03-25
**Status:** Accepted

---

## Context

Three source files had grown to sizes that made navigation and maintenance painful:

| File | Lines before |
|------|-------------|
| `src/editor/mod.rs` | 5 523 |
| `src/agent/mod.rs` | 2 613 |
| `src/ui/mod.rs` | 2 939 |

All three are central to the editor: `editor/mod.rs` owns the entire event loop and
editor state, `agent/mod.rs` owns the Copilot agent panel and all agent data types,
`ui/mod.rs` owns all terminal rendering. At these sizes, adding a feature required
scrolling thousands of lines to find the right section; `grep` output for a function
name was the primary navigation tool.

Rust's module system allows `impl` blocks for a single struct to be split across
multiple files (via submodules), making it possible to decompose these files without
changing any public APIs or struct definitions.

---

## Decision

Split all three files into submodules. Struct definitions and top-level types remain
in `mod.rs`; `impl` blocks and free functions move into named submodules. Each
submodule uses `use super::*;` so that all types from the parent are in scope without
repetitive imports.

### Phase 1 — `src/editor/mod.rs` (5 523 → ~1 600 lines)

Nine submodules extracted:

| Submodule | Contents |
|-----------|----------|
| `actions.rs` | `execute_action()` — the main action dispatch function |
| `ai.rs` | AI/Copilot integration (inline edit, commit message, release notes) |
| `diff.rs` | Apply-diff overlay logic |
| `file_ops.rs` | File open, save, reload, watcher integration |
| `input.rs` | Raw key input handling, insert mode, paste |
| `lsp.rs` | LSP event polling, diagnostics, hover |
| `mode_handlers.rs` | Per-mode key handling (Normal, Visual, Command, …) |
| `pickers.rs` | Buffer picker, file picker, at-picker |
| `search.rs` | In-file search and project-wide search |

### Phase 2 — `src/agent/mod.rs` (2 613 → ~390 lines)

Four submodules extracted (pre-existing files refactored):

| Submodule | Contents |
|-----------|----------|
| `auth.rs` | OAuth token loading, Copilot API token exchange, `one_shot_complete` |
| `models.rs` | `fetch_models()` — GET /models endpoint |
| `agentic_loop.rs` | Tool-calling loop, `start_chat_stream_with_tools`, compression helpers |
| `panel.rs` | Full `impl AgentPanel` block (~1 156 lines), `build_project_tree` |

`mod.rs` retains all type definitions: `ChatMessage`, `AgentPanel` struct,
`ModelVersion`, `AgentStatus`, `StreamEvent`, `Role`, `ContentSegment`, etc.

### Phase 3 — `src/ui/mod.rs` (2 939 → ~490 lines)

Seven submodules extracted:

| Submodule | Contents |
|-----------|----------|
| `agent_panel.rs` | `render_agent_panel`, `render_slash_menu`, `render_at_picker`, `render_continuation_dialog`, `render_ask_user_dialog`, `render_file_explorer`, `render_task_strip` |
| `buffer_view.rs` | `render_buffer`, `render_welcome`, `render_highlighted_line`, `render_line` |
| `pickers.rs` | `render_which_key`, `render_buffer_picker`, `render_file_picker` |
| `search_lsp.rs` | `render_search_panel`, `render_location_list` |
| `status.rs` | `render_status_line` |
| `popups.rs` | All modal popup renders, `format_file_size`, `format_system_time`, `render_file_info_popup` |
| `markdown.rs` | `render_message_content` — markdown + `<think>` block renderer |

`mod.rs` retains all type definitions (`RenderContext`, `UI`, `PanelRenderCache`,
`ApplyDiffView`, `ReleaseNotesView`, `DiagnosticsData`, `FileInfoData`) and the
top-level `impl UI { render() }` entry point.

---

## Implementation notes

### Visibility

Methods inside `impl UI` or `impl AgentPanel` blocks in submodule files are declared
`pub(super)` so they are callable from `mod.rs` (the parent) but not from external
crates or unrelated modules. Free functions such as `render_message_content` are
similarly `pub(super)`.

### Cross-sibling access

`render_message_content` (in `markdown.rs`) is called from `agent_panel.rs`. Since
sibling submodules cannot see each other's `pub(super)` items directly via
`use super::*;`, `agent_panel.rs` imports it explicitly:

```rust
use super::markdown::render_message_content;
```

### `RenderContext` field addition

During the split, a new field `in_file_search_query: Option<&'a str>` was added to
`RenderContext` to carry the in-file search buffer into the render path (set by
`editor/mod.rs`, consumed by the status line renderer for mode label display).

---

## Consequences

**Positive**
- All three files are now navigable without grep-driven scrolling.
- Feature work is localised: adding a new popup only touches `popups.rs`; a new
  editor action only touches `actions.rs` or the relevant mode handler file.
- No public API changes — all struct definitions, public types, and the public
  `UI::render()` entry point remain in `mod.rs`.
- `cargo clippy -D warnings` and all 11 unit tests continue to pass.

**Negative / trade-offs**
- `use super::*;` in each submodule is a blanket import — it does not document which
  types a submodule actually uses. This is intentional (reduces noise in files that
  use many types) but means the compiler does not flag accidental removal of a type
  from `mod.rs`.
- Cross-sibling imports (e.g. `use super::markdown::render_message_content;`) are
  an additional coupling point that must be maintained if functions move between
  submodules.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0045](0045-mcp-client.md) | MCP client lives in `src/mcp/` — the same submodule pattern used here |
| [0077](0077-agent-context-window-management.md) | Context window management implemented in `agent/agentic_loop.rs` |
