# Editor & Agent: Safe Incremental Refactor

**Goal:** Break `editor/mod.rs` (2,388 lines) and `agent/panel.rs` / `agent/agentic_loop.rs` into smaller, single-concern modules — safely, using the Rust type checker + tests as the regression net.

**See also:** [module-analysis.md](module-analysis.md) for the size breakdown and performance issues.

---

## Rules of the Road

1. **Tests before moves** — Write characterization tests that pin current behaviour before touching any structure.
2. **Structural moves are logic-free** — Copy code verbatim. Zero logic changes during a move. Only add `use` statements.
3. **Performance fixes are separate** — Each perf fix is its own commit, strictly after all structural phases are done.
4. **Gate between phases:**
   ```bash
   cargo fmt --all
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
   All three must pass before moving to the next phase.
5. **Rust module mechanics** — Sub-files in `src/editor/` contain `impl Editor { ... }` blocks. The compiler merges them into a single module. Callers never reference file names, so no public API changes are needed anywhere.

---

## Phase 0 — Write Docs

- [x] Create `docs/module-analysis.md` — size table, extraction targets, perf issues, test baseline
- [x] Create `docs/editor-agent-refactor.md` — this file

---

## Phase 1 — Characterization Tests

**Risk:** None (additive only — no changes to non-test code, except one helper extraction in 1e).  
**Target:** ~38 tests → ~60 tests, all green.

### 1a. `src/buffer/buffer.rs` — expand to 10+ tests

Add to the existing `#[cfg(test)] mod tests` block:

| Test name | What it checks |
|-----------|---------------|
| `test_move_cursor_right_at_eol` | cursor at EOL stays put |
| `test_move_cursor_left_at_bol` | col 0 on row > 0 wraps to prev line end |
| `test_delete_char_at_empty_line` | backspace on empty line merges with previous |
| `test_visual_rows_wrap` | `visual_rows_for_len(160, 80)` == 2 |
| `test_yank_selection_multiline` | yank across 3 lines returns correct string |
| `test_undo_roundtrip` | insert → snapshot → undo → original content |

Pattern: `Buffer::new("test_content")` — no `Editor`, no terminal, no async.

### 1b. `src/editor/hooks.rs` — expand to 10+ tests

Add alongside existing 5 glob tests:

| Test name | Pattern | Input | Expect |
|-----------|---------|-------|--------|
| `no_separator_deep_path` | `"*.rs"` | `"a/b/c/foo.rs"` | false (filename match only) |
| `anchored_prefix` | `"src/*.rs"` | `"src/a/b.rs"` | false |
| `double_star_root` | `"**"` | any path | true |
| `empty_pattern` | `""` | `""` | true |
| `empty_pattern_non_empty` | `""` | `"foo"` | false |

### 1c. `src/editor/actions.rs` — test pure surround helpers

Add `#[cfg(test)] mod tests` at the bottom of `actions.rs`. The functions `surround_pair(ch)` and `find_surround_on_line(chars, col, open, close)` are already pure functions (no `self`). Test them directly:

| Test name | Input | Expected |
|-----------|-------|---------|
| `surround_pair_parens` | `'('` | `('(', ')')` |
| `surround_pair_braces` | `'{'` | `('{', '}')` |
| `surround_pair_symmetric` | `'"'` | `('"', '"')` |
| `find_surround_basic` | `"(hello)"` col 3 | Some((0, 6)) |
| `find_surround_none` | `"hello"` col 2 | None |
| `find_surround_cursor_at_open` | `"(hi)"` col 0 | Some((0, 3)) |

### 1d. `src/agent/tools.rs` — test `safe_path` and `execute_tool`

Add `#[cfg(test)] mod tests` at the bottom of `tools.rs`. `safe_path` is a pure function. Use `std::env::temp_dir()` for I/O tests:

| Test name | What it checks |
|-----------|---------------|
| `safe_path_traversal_rejected` | `safe_path(root, "../etc/passwd")` returns Err |
| `safe_path_valid` | `safe_path(root, "src/main.rs")` returns Ok |
| `execute_tool_unknown` | unknown tool name returns "unknown tool: bogus" string |
| `execute_tool_read_missing` | read_file on nonexistent path returns error string |
| `execute_tool_write_then_read` | write to temp file, then read_file returns same content |

### 1e. `src/agent/agentic_loop.rs` — extract + test SSE line parser

This is the one small logic extraction needed before Phase 6. Extract:

```rust
pub(super) enum SseLine {
    Done,
    Token(String),
    ToolDelta { index: usize, id: Option<String>, name: Option<String>, args_fragment: String },
    Skip,
}

pub(super) fn parse_sse_line(line: &str) -> SseLine { ... }
```

Then add tests:

| Test name | Input | Expected |
|-----------|-------|---------|
| `sse_done` | `"data: [DONE]"` | `SseLine::Done` |
| `sse_token` | `r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#` | `SseLine::Token("hi")` |
| `sse_keepalive` | `": keepalive"` | `SseLine::Skip` |
| `sse_empty` | `""` | `SseLine::Skip` |

**Checkpoint:** `cargo test` → all ~60 tests green.

---

## Phase 2 — Extract `editor/state.rs`

**Risk:** Low — pure data types, no method logic.  
**Result:** `mod.rs` shrinks by ~400 lines.

### What moves

Everything in `mod.rs` lines 42–443 (all the state types before the `Editor` struct):

- `ClipboardType`
- `HighlightCache`, `StickyScrollCache`, `MarkdownCache`
- `LocationEntry`, `LocationListState`
- `InlineAssistPhase`, `InlineAssistState`
- `HoverPopupState`
- `DiffLine`, `Verdict`, `FileDiff` + impl
- `ReviewChangesState` + impl
- Free functions: `review_compute_offsets`, `review_diff_lines`, `apply_hunk_verdicts`
- `SplitState`, `CommitMsgState`, `ReleaseNotesState`

### Steps

1. Create `src/editor/state.rs`
2. Copy all `use` statements referenced by these types
3. Cut the blocks from `mod.rs`, paste verbatim into `state.rs`
4. In `mod.rs`, add at the top of the module declarations block:
   ```rust
   mod state;
   use state::*;
   ```

### Visibility

| Type | Visibility in state.rs |
|------|----------------------|
| `SplitState`, `CommitMsgState`, `ReleaseNotesState` | `pub(super)` — only used within `editor/` |
| `review_compute_offsets`, `review_diff_lines`, `apply_hunk_verdicts` | `pub(super)` |
| Everything else that was `pub` | `pub` |

**Checkpoint:** `cargo build` first (fast). Fix any missing `use` statements. Then `cargo test`.

---

## Phase 3 — Extract `editor/folding.rs` + `editor/inline_assist.rs`

**Risk:** Low — small, self-contained impl blocks.

### `src/editor/folding.rs`

Move from `mod.rs` lines 1,097–1,219:

```rust
use super::Editor;

impl Editor {
    pub(crate) fn fold_toggle(&mut self) { ... }
    pub(crate) fn fold_close_all(&mut self) { ... }
    pub(crate) fn fold_open_all(&mut self) { ... }
}
```

Add `mod folding;` to `mod.rs` module declarations.

### `src/editor/inline_assist.rs`

Move from `mod.rs`:
- `poll_inline_assist(&mut self) -> bool`
- `strip_assist_fence(s: &str) -> String` (free function)

Move from `input.rs`:
- `handle_inline_assist_mode(&mut self, key: KeyEvent) -> Result<()>`

```rust
use anyhow::Result;
use crossterm::event::KeyEvent;
use super::Editor;
use super::state::{InlineAssistPhase, InlineAssistState};
use crate::agent::StreamEvent;

impl Editor {
    pub(super) fn poll_inline_assist(&mut self) -> bool { ... }
    pub(super) fn handle_inline_assist_mode(&mut self, key: KeyEvent) -> Result<()> { ... }
}

pub(super) fn strip_assist_fence(s: &str) -> String { ... }
```

Add `mod inline_assist;` to `mod.rs`. Remove the method body from `input.rs`.

**Checkpoint:** `cargo clippy --all-targets -- -D warnings && cargo test`.

---

## Phase 4 — Extract `editor/render.rs` + `editor/event_loop.rs`

**Risk:** Medium — large bodies with many field references. Easy to miss a `use`.  
**Result:** `mod.rs` reaches ~450 lines. This is the biggest editor phase.

### `src/editor/render.rs`

Move `render()` (~520 lines) verbatim. Add `#[allow(clippy::too_many_lines)]`.

Key imports needed:
```rust
use anyhow::Result;
use ratatui::text::Span;
use super::Editor;
use super::state::{HighlightCache, MarkdownCache, StickyScrollCache, InlineAssistPhase};
use crate::keymap::Mode;
use crate::ui::{RenderContext, UI};
```

### `src/editor/event_loop.rs`

Move `run()` (~1,060 lines) verbatim. Add `#[allow(clippy::too_many_lines)]`.

Key imports needed:
```rust
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use tokio::signal::unix::{signal, SignalKind};
use super::Editor;
use crate::keymap::Mode;
```

All `self.foo()` calls to sibling modules (`actions`, `input`, `lsp`, `ai`, etc.) dispatch automatically — they are all `impl Editor` methods. No extra imports for method calls.

### What stays in `mod.rs` after Phase 4

- Module declarations (`mod state; mod render; mod event_loop; mod folding; mod inline_assist; mod actions; mod ai; mod file_ops; mod hooks; mod input; mod lsp; mod mode_handlers; mod pickers; mod search;`)
- All `use` imports
- `Editor` struct definition (120+ fields)
- `impl Editor { pub fn new() ... }` constructor
- `open_file()`, `current_buffer()`, `current_buffer_mut()`, `ts_tree_for_current_buffer()`, `with_buffer()`
- `render_loading()`, `setup_services()`
- Small helpers: `check_quit()`, `set_status()`, `set_sticky()`, `sync_system_clipboard()`, `cleanup()`
- `impl Drop for Editor`

**Checkpoint:** `cargo build --release && cargo test`. Release build catches dead_code that debug misses.

---

## Phase 5 — Extract `editor/surround.rs` + `editor/text_objects.rs`

**Risk:** Low — pure logic, no state complexity.

### `src/editor/surround.rs`

Move from `actions.rs` lines 1,040–1,134:

```rust
use super::Editor;

pub(super) fn surround_pair(ch: char) -> (char, char) { ... }
pub(super) fn find_surround_on_line(
    chars: &[char], cursor_col: usize, open: char, close: char
) -> Option<(usize, usize)> { ... }

impl Editor {
    pub(super) fn apply_surround_delete(&mut self, ch: char) { ... }
    pub(super) fn apply_surround_change(&mut self, from: char, to: char) { ... }
    pub(super) fn apply_surround_add(&mut self, ch: char) { ... }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Move Phase 1c tests here
}
```

In `actions.rs`, add:
```rust
mod surround;
use surround::{surround_pair, find_surround_on_line};
```

### `src/editor/text_objects.rs`

Move from `actions.rs` lines 1,131–1,215:

```rust
use super::Editor;
use super::state::ClipboardType;
use crate::keymap::{Mode, TextObjectKind};

impl Editor {
    pub(super) fn text_object_range(&mut self, ...) -> Option<(...)> { ... }
    pub(super) fn set_selection_range(&mut self, ...) { ... }
    pub(super) fn apply_text_object_select(&mut self, ...) { ... }
    pub(super) fn apply_text_object_delete(&mut self, ...) { ... }
    pub(super) fn apply_text_object_yank(&mut self, ...) { ... }
    pub(super) fn apply_text_object_change(&mut self, ...) { ... }
}
```

**Checkpoint:** `cargo clippy -- -D warnings && cargo test`.

---

## Phase 6 — Extract Agent Sub-Modules

**Risk:** Medium-high — async boundaries and type visibility are trickier than editor moves.  
**Do in this order (lowest risk first).**

### 6a. `src/agent/context.rs`

Move from `mod.rs`:
- `ContextBreakdown` struct + impl (`total()`, `used_pct()`)
- `SubmitCtx` struct
- `message_importance(msg: &ChatMessage) -> u32`

Move from `panel.rs`:
- `compress_history(&mut self)` as `impl AgentPanel` block

In `mod.rs`:
```rust
pub mod context;
pub use context::{ContextBreakdown, SubmitCtx, message_importance};
```

### 6b. `src/agent/session.rs`

Move from `mod.rs`:
- `metrics_data_path() -> Option<PathBuf>`
- `history_file_path(session_start_secs: u64) -> Option<PathBuf>`
- `append_session_metric(record: &serde_json::Value)`

Move from `panel.rs`:
- `has_checkpoint(&self) -> bool`
- `revert_session(&mut self, project_root: &Path) -> (Vec<String>, Vec<String>)`

In `mod.rs`:
```rust
pub mod session;
pub use session::{metrics_data_path, history_file_path, append_session_metric};
```

### 6c. `src/agent/streaming.rs`

Extract from `agentic_loop.rs` lines 237–455 into:

```rust
pub(super) struct ParsedRound {
    pub text_buf: String,
    pub partial_tools: HashMap<usize, PartialToolCall>,
}

pub(super) async fn parse_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    model_id: &str,
) -> ParsedRound { ... }
```

Move Phase 1e SSE unit tests here into `#[cfg(test)] mod tests`.

Call site in `agentic_loop.rs` becomes:
```rust
let parsed = streaming::parse_sse_stream(response, &tx, &model_id).await;
```

### 6d. `src/agent/tool_dispatch.rs`

Extract from `agentic_loop.rs` lines 466–661 (the `for (_, partial) in sorted` tool execution loop).

If lifetime complexity is high, use a plain `async fn`:
```rust
pub(super) async fn dispatch_tools(
    sorted: Vec<(usize, PartialToolCall)>,
    messages: &mut Vec<serde_json::Value>,
    text_buf: String,
    project_root: &Path,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    snapshotted: &mut HashSet<String>,
    mcp_manager: Option<Arc<McpManager>>,
    auto_compress: bool,
) { ... }
```

Move verbatim first. Refine the interface in a follow-up commit.

**Checkpoint (per sub-step):** `cargo build` after each. `cargo test` after all four.

---

## Phase 7 — Performance Fixes

**Risk:** Medium-High — these are logic changes, not structural.  
**One fix per commit** so each is independently bisectable.

### Fix 1: Blocking I/O in `agent/tools.rs`

Replace all `std::fs::read_to_string` and `std::fs::write` calls with `tokio::fs` equivalents. Make `execute_tool` async:

```rust
// Before:
pub fn execute_tool(call: &ToolCall, root: &Path) -> String

// After:
pub async fn execute_tool(call: &ToolCall, root: &Path) -> String
```

Update the one call site in `agentic_loop.rs`.

### Fix 2: SSE buffer drain in `agent/streaming.rs`

Replace per-line `drain(..=pos)` with an index cursor:

```rust
let mut cursor = 0usize;
while let Some(rel) = sse_buf[cursor..].find('\n') {
    let line = sse_buf[cursor..cursor + rel].trim();
    // process line...
    cursor += rel + 1;
}
sse_buf.drain(..cursor);  // single O(n) drain per chunk, not per line
```

### Fix 3: `tool_defs` clone in `agent/agentic_loop.rs`

```rust
// Before (per round):
let tool_defs = build_tool_defs(...);
start_chat_stream_with_tools(..., tool_defs.clone(), ...).await

// After:
let tool_defs = Arc::new(build_tool_defs(...));
start_chat_stream_with_tools(..., Arc::clone(&tool_defs), ...).await
```

Update `start_chat_stream_with_tools` signature to accept `Arc<serde_json::Value>`.

### Fix 4: `messages` clone in `agent/agentic_loop.rs`

```rust
// Before:
start_chat_stream_with_tools(..., messages.clone(), ...)

// After:
start_chat_stream_with_tools(..., &messages, ...)
```

Change signature to `messages: &[serde_json::Value]` and update the `reqwest::json!` serialization call inside.

### Fix 5: Bounded stream channel in `agent/panel.rs` + `agent/agentic_loop.rs`

```rust
// Before:
let (tx, rx) = mpsc::unbounded_channel::<StreamEvent>();

// After:
let (tx, rx) = mpsc::channel::<StreamEvent>(128);
```

Update all `.send(event)` → `.send(event).await` in `agentic_loop.rs`. Handle `SendError` (stream consumer gone) as a clean exit condition.

**Do this as a single atomic commit** — it touches both files simultaneously.

**Checkpoint:** `cargo test` + manual: stream a long agent response, confirm render loop stays responsive.

---

## Phase 8 — Targeted Unit Tests for Newly Isolated Units

Now that units have clean API boundaries, write proper tests.

### `editor/surround.rs` — extend to 10+ tests
Move Phase 1c tests here. Add:
- `find_surround_cursor_at_close` — cursor on `)` char
- `find_surround_multichar_line` — long line with multiple pairs, finds innermost

### `agent/streaming.rs` — extend SSE tests
Move Phase 1e tests here. Add:
- `sse_tool_call_delta` — partial tool_call chunk accumulates arguments correctly
- `sse_model_switched` — `model` field different from request emits `ModelSwitched`
- `sse_usage_event` — usage block emits `Usage` event

### `agent/session.rs` — path computation tests
```rust
test_metrics_data_path_xdg      // with XDG_DATA_HOME set, correct path
test_metrics_data_path_home     // fallback to ~/.local/share
test_history_file_path_nonzero  // correct filename from timestamp
```

### `agent/context.rs` — scoring and breakdown tests
```rust
test_message_importance_user_error   // "error" in user message → score 6
test_message_importance_plain_assistant  // plain assistant → score 2
test_context_breakdown_total         // sum of all four fields
test_context_breakdown_used_pct      // correct percentage calculation
```

**Final checkpoint:**
```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test   # target: 80+ tests, all green
cargo build --release
```

---

## Dependency & Sequencing

```
Phase 1 (characterization tests)
    └── Phase 2 (editor/state.rs)
         └── Phase 3 (folding + inline_assist)
              └── Phase 4 (render + event_loop)   ← mod.rs reaches ~450 lines
                   └── Phase 5 (surround + text_objects)
                        └── Phase 6 (agent sub-modules)  ← do 6a→6b→6c→6d
                             └── Phase 7 (perf fixes)    ← one commit per fix
                                  └── Phase 8 (targeted tests)
```

Phases 2–5 are editor-only. Phase 6 is agent-only. Phases 7 and 8 are independent — can be interleaved with Phase 6 sub-steps if desired.

---

## Risk Summary

| Phase | Risk | Key concern |
|-------|------|-------------|
| 1 — Tests | None | Additive only |
| 2 — state.rs | Low | Missing `use` statements |
| 3 — folding/inline_assist | Low-Med | `handle_inline_assist_mode` is split across two files |
| 4 — render/event_loop | Medium | Large bodies; many field references |
| 5 — surround/text_objects | Low | Pure logic, no state |
| 6 — agent sub-modules | Med-High | Async ownership; `messages` lifetime; channel types |
| 7 — perf fixes | High | Logic changes; bounded channel semantics change error handling |
| 8 — new tests | None | Additive |

---

## Conventions to Follow

- **`pub(super)`** for anything only used within the same module directory
- **Specific imports** in sub-files (`use super::{Editor, ClipboardType}`), not `use super::*`
- **`mod foo;`** declarations go at the top of `mod.rs`, before `use` blocks
- **Tests** go in `#[cfg(test)] mod tests { use super::*; use pretty_assertions::assert_eq; }` at the bottom of the file containing the code
- **`#[allow(clippy::too_many_lines)]`** on `render()` and `run()` during the transition — add a `// TODO: split further` comment, not a permanent suppression
