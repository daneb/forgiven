# Large File Refactor Plan

## Problem Statement

Three source files dominate the codebase and are a meaningful concern for both Rust development and AI-assisted maintenance:

| File | Lines | Size |
|------|-------|------|
| `src/editor/mod.rs` | 5,523 | 239 KB |
| `src/ui/mod.rs` | 2,942 | 128 KB |
| `src/agent/mod.rs` | 2,618 | 115 KB |
| **Total** | **11,083** | **482 KB** |

All other source files are under 1,300 lines and are not a concern.

---

## Why This Matters

### Rust-specific impact
- **rust-analyzer** performance degrades on large files — hover, completion, and diagnostics slow down as the file grows. `editor/mod.rs` at 5.5k lines is genuinely painful in practice.
- **Incremental compilation** — Rust recompiles entire modules on any change. One keymap change in `editor/mod.rs` triggers recompilation of 239 KB of code including LSP, git, AI, and rendering logic that didn't change.
- **Clippy** must hold the entire file in memory to reason about lint rules — less effective at detecting cross-function issues at this scale.
- **IDE navigation** (go-to-definition, outline view) becomes cluttered. rust-analyzer's document symbol list for `editor/mod.rs` returns 48+ methods with no grouping.

### AI-assisted maintenance impact
- **Context window exhaustion** — `editor/mod.rs` alone (239 KB) approaches or exceeds the usable context window for many AI models. An AI cannot read the full file and answer questions or make changes reliably.
- **Precision** — When a feature only touches LSP handling, an AI asked to make a change must parse through 4,000 lines of unrelated input handling, rendering, and git integration. Error rates increase.
- **Discoverability** — AI tools (and human developers) cannot predict where a feature lives without searching. A well-named `editor/lsp.rs` is self-documenting. A 5.5k-line `editor/mod.rs` is not.
- **Diff noise** — PRs touching logically unrelated features but sharing a file are harder to review.

---

## Approach: Rust Module Splitting

Rust makes this refactor low-risk. The pattern is:

1. Create `src/editor/` as a directory (already is one — `mod.rs` exists).
2. Extract cohesive groups of methods/types into new sibling files: `src/editor/lsp.rs`, `src/editor/actions.rs`, etc.
3. In `editor/mod.rs`, add `mod lsp;` declarations and re-export any public types with `pub use lsp::*;` if needed.
4. Methods that require `&mut self` stay as `impl Editor` blocks — Rust allows `impl` blocks to be split across files **within the same module** using `mod` declarations. Each sub-file gets `use super::*;` or explicit imports.

No public API changes are required. Callers in `main.rs` and elsewhere are unaffected.

---

## Priority Order

Tackle in order of pain-to-fix ratio:

1. **`editor/mod.rs`** — Highest priority. 5.5k lines, most mixed concerns, slowest to compile, hardest for AI to reason about.
2. **`agent/mod.rs`** — Second. Contains unrelated concerns: HTTP streaming, OAuth, calendar math, code extraction.
3. **`ui/mod.rs`** — Third. Already fairly well organized as a set of render functions; splitting is valuable but lower urgency.

---

## Phase 1: `src/editor/mod.rs` → 9 modules

**Target: reduce `mod.rs` to ~400 lines** (struct definition, `impl Editor` skeleton, and `mod` declarations).

### Proposed sub-modules

| File | What moves there | Est. lines |
|------|-----------------|-----------|
| `editor/input.rs` | `handle_key`, `handle_normal_mode`, `handle_insert_mode`, `handle_command_mode`, `handle_visual_mode`, `handle_visual_line_mode`, `handle_paste` | ~1,200 |
| `editor/actions.rs` | `execute_action` and all dispatch logic | ~800 |
| `editor/file_ops.rs` | `open_file`, `current_buffer*`, `with_buffer`, `scan_files`, `scan_directory`, `read_file_for_context`, `load_recents`, `save_recents` | ~400 |
| `editor/lsp.rs` | All `request_*` and `handle_*_response` methods, `handle_location_list_mode`, diagnostic nav, `notify_lsp_change`, LSP free functions (`lsp_parse_location`, `lsp_uri_to_path`, `lsp_flatten_symbol`, `lsp_symbol_kind_name`) | ~600 |
| `editor/mode_handlers.rs` | `handle_explorer_mode`, `handle_apply_diff_mode`, `handle_search_mode`, `handle_preview_mode`, `handle_rename_mode`, `handle_delete_mode`, `handle_new_folder_mode`, `do_rename`, `do_delete`, `do_create_folder` | ~600 |
| `editor/pickers.rs` | `handle_pick_buffer_mode`, `handle_pick_file_mode`, `refilter_files`, `fuzzy_score`, `open_at_picker`, `refilter_at_picker`, `handle_at_picker_key`, `is_picker_sentinel`, `recents_path` | ~350 |
| `editor/search.rs` | `handle_in_file_search_mode`, `on_search_input_changed`, `fire_search` | ~200 |
| `editor/ai.rs` | `start_commit_msg`, `handle_commit_msg_mode`, `start_release_notes`, `handle_release_notes_mode`, `trigger_release_notes_generation`, `request_inline_completion`, `open_markdown_in_browser`, `open_mermaid_in_browser`, `fix_mermaid_parens`, `strip_markdown_fence`, `open_lazygit` | ~500 |
| `editor/render_data.rs` | `render` (the main per-frame render dispatch), `render_loading`, `clear_apply_diff`, `set_status`, `set_sticky`, `sync_system_clipboard`, `check_quit` | ~500 |
| `editor/diff.rs` | `lcs_diff` free function | ~60 |

### What stays in `editor/mod.rs`
- All struct/enum definitions (`Editor`, `HighlightCache`, `MarkdownCache`, `SplitState`, `ApplyDiffState`, `CommitMsgState`, `ReleaseNotesState`, `LocationEntry`, `LocationListState`, `ClipboardType`, `DiffLine`)
- `impl Editor { pub fn new(...) }` and `impl Drop for Editor`
- `mod input; mod actions; mod file_ops; mod lsp; ...` declarations

---

## Phase 2: `src/agent/mod.rs` → 8 modules

**Target: reduce `mod.rs` to ~300 lines** (type definitions and re-exports).

| File | What moves there | Est. lines |
|------|-----------------|-----------|
| `agent/types.rs` | All structs/enums: `ChatMessage`, `Role`, `ContentSegment`, `ClipboardImage`, `AgentTask`, `AskUserState`, `SlashMenuState`, `AtPickerState`, `AgentPanel`, `ModelVersion`, `AgentStatus`, `StreamEvent`, `CopilotApiToken`, `TokenExpiredError`; free fns `split_thinking`, `message_importance` | ~350 |
| `agent/panel.rs` | `AgentPanel::new`, visibility/focus, input, model selection (`cycle_model`, `ensure_models`, `refresh_models`, `set_models`), `new_conversation`, `submit`, `try_paste_image`, slash-menu methods | ~700 |
| `agent/stream.rs` | `poll_stream`, `approve_continuation`, `deny_continuation`, `confirm_user_question`, `cancel_stream`, `cancel_user_question`, `move_question_selection`, `scroll_*` | ~300 |
| `agent/code.rs` | `extract_code_blocks`, `extract_mermaid_blocks`, `extract_first_code_block_with_path`, `get_code_to_apply`, `has_code_to_apply`, `get_apply_candidate`, `last_assistant_reply` | ~180 |
| `agent/agentic_loop.rs` | `agentic_loop`, `build_project_tree`, `tree_recursive` | ~600 |
| `agent/api.rs` | `start_chat_stream_with_tools`, `fetch_models`, `acquire_copilot_token`, `exchange_token`, `load_oauth_token`, `ensure_token` | ~500 |
| `agent/compress.rs` | `maybe_compress` and any compression constants | ~100 |
| `agent/time.rs` | `chrono_unix_from_iso`, `days_before_month` | ~50 |

`agent/tools.rs` already exists and is well-scoped — no changes needed there.

---

## Phase 3: `src/ui/mod.rs` → 6 modules

**Target: reduce `mod.rs` to ~200 lines** (type definitions and `UI::render` main dispatch).

| File | What moves there | Est. lines |
|------|-----------------|-----------|
| `ui/types.rs` | `PanelRenderCache`, `ApplyDiffView`, `ReleaseNotesView`, `DiagnosticsData`, `FileInfoData`, `RenderContext` | ~150 |
| `ui/buffer.rs` | `render_buffer`, `render_highlighted_line`, `render_line` | ~350 |
| `ui/panels.rs` | `render_agent_panel`, `render_file_explorer`, `render_file_info_popup`, `render_task_strip`, `render_welcome`, `render_status_line`, `render_diagnostics_overlay`, `render_location_list` | ~800 |
| `ui/popups.rs` | `render_buffer_picker`, `render_file_picker`, `render_which_key`, `render_apply_diff_overlay`, `render_commit_msg_popup`, `render_release_notes_popup`, `render_rename_popup`, `render_delete_popup`, `render_new_folder_popup`, `render_binary_file_popup`, `render_search_panel` | ~900 |
| `ui/dialogs.rs` | `render_slash_menu`, `render_at_picker`, `render_continuation_dialog`, `render_ask_user_dialog` | ~300 |
| `ui/format.rs` | `format_file_size`, `format_system_time`, `wrapped_line_count`, `render_message_content` | ~300 |

---

## Execution Notes

### Safe splitting pattern for `impl` methods

In Rust, `impl` blocks for a type can span multiple files as long as all files are within the same module. The standard pattern:

```rust
// editor/lsp.rs
use super::Editor;  // or `use super::*;` for convenience

impl Editor {
    pub fn request_hover(&mut self) { ... }
    pub fn handle_goto_definition_response(&mut self, value: serde_json::Value) { ... }
}
```

Then in `editor/mod.rs`:
```rust
mod lsp;
```

No re-exports needed for methods — they're just on `Editor` wherever defined.

### Do one phase at a time

Each phase should be its own PR and pass `make check` (fmt + clippy + test) before the next begins. Do not refactor logic during extraction — move code verbatim. Logic improvements are a separate concern.

### Suggested order within Phase 1

Start with the clearest seams:
1. `editor/diff.rs` — self-contained free function, no `self`
2. `editor/lsp.rs` — LSP methods are clearly scoped with `request_*`/`handle_*` naming
3. `editor/ai.rs` — commit msg, release notes, inline edit are clearly scoped
4. `editor/search.rs` — small, isolated
5. `editor/file_ops.rs` — file I/O with no mode-handling dependencies
6. `editor/pickers.rs` — fuzzy search logic
7. `editor/mode_handlers.rs` — explorer, apply-diff, rename, delete, new-folder
8. `editor/input.rs` — the five main mode handlers
9. `editor/actions.rs` — `execute_action` dispatch (depends on many of the above)
10. `editor/render_data.rs` — the `render` method (can move last)

---

## Expected Outcome

After all three phases:

| File | Before | After (est.) |
|------|--------|-------------|
| `editor/mod.rs` | 5,523 lines | ~400 lines |
| `ui/mod.rs` | 2,942 lines | ~200 lines |
| `agent/mod.rs` | 2,618 lines | ~300 lines |

No file in the codebase will exceed ~900 lines. AI tools will be able to read any single module in full. rust-analyzer performance will improve. Incremental compile times for single-feature changes will drop because only the relevant sub-module recompiles.
