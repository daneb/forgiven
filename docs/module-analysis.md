# Module Size Analysis

**Generated:** 2026-04-09  
**Scope:** All `.rs` files with more than 120 lines, grouped by module.

---

## Summary Table

| Module | File | Lines | Status |
|--------|------|------:|--------|
| editor | `src/editor/mod.rs` | 2,388 | ⚠️ CRITICAL — 4+ concerns in one file |
| editor | `src/editor/actions.rs` | 1,215 | Large — surround + text-objects are extractable |
| editor | `src/editor/input.rs` | 1,181 | Large — inline-assist handler straddles mod.rs |
| editor | `src/editor/lsp.rs` | 840 | Acceptable — well isolated |
| editor | `src/editor/ai.rs` | 800 | Acceptable — already mostly isolated |
| editor | `src/editor/mode_handlers.rs` | 732 | Acceptable |
| editor | `src/editor/hooks.rs` | 434 | Good — 5 tests present |
| editor | `src/editor/pickers.rs` | 335 | Acceptable |
| editor | `src/editor/search.rs` | 128 | Small |
| editor | `src/editor/file_ops.rs` | 124 | Small |
| **editor total** | | **8,177** | |
| agent | `src/agent/panel.rs` | 1,838 | Large — session/history/submit/UI all mixed |
| agent | `src/agent/tools.rs` | 946 | Large — schemas + dispatch + symbol extraction mixed |
| agent | `src/agent/agentic_loop.rs` | 866 | Large — SSE + tool-exec + HTTP layer interleaved |
| agent | `src/agent/mod.rs` | 575 | Medium — types and path helpers together |
| agent | `src/agent/models.rs` | 519 | Acceptable |
| agent | `src/agent/provider.rs` | 260 | Small |
| agent | `src/agent/auth.rs` | 179 | Small |
| agent | `src/agent/token_count.rs` | 22 | Trivial |
| **agent total** | | **5,205** | |
| buffer | `src/buffer/buffer.rs` | 1,393 | Acceptable — 3 tests, cursor ops could use more |
| lsp | `src/lsp/mod.rs` | 1,121 | Acceptable |
| mcp | `src/mcp/mod.rs` | 886 | Acceptable |
| keymap | `src/keymap/mod.rs` | 762 | Acceptable |
| spec_framework | `src/spec_framework/spec_slicer.rs` | 402 | Acceptable |
| spec_framework | `src/spec_framework/mod.rs` | 356 | Acceptable |
| markdown | `src/markdown/mod.rs` | 638 | Acceptable |
| config | `src/config/mod.rs` | 619 | Acceptable |
| treesitter | `src/treesitter/query.rs` | 341 | Acceptable |
| explorer | `src/explorer/mod.rs` | 303 | Acceptable |
| **Grand total** | | **~25,100** | |

---

## Why `editor/mod.rs` Is Critical

At 2,388 lines it mixes **six distinct concerns**:

| Concern | Approx lines | What it is |
|---------|-------------:|------------|
| State types | ~400 | 14 structs/enums — pure data, no Editor logic |
| Editor struct + `new()` | ~390 | 120-field struct definition + constructor |
| `render()` | ~520 | Entire TUI draw pass |
| `run()` | ~1,060 | Main async event loop |
| Fold ops | ~125 | `fold_toggle`, `fold_close_all`, `fold_open_all` |
| Inline-assist poll | ~90 | `poll_inline_assist`, `strip_assist_fence` |

Every change to rendering triggers recompilation of the event loop. rust-analyzer holds all 2,388 lines in memory to resolve any single method call. The document-symbol outline is 48+ methods with no grouping.

---

## Planned Extraction Targets

### `editor/mod.rs` → 5 new files

| New File | What Moves | Est. Lines |
|----------|------------|----------:|
| `editor/state.rs` | All 14 state structs/enums (lines 42–443) | ~400 |
| `editor/render.rs` | `render()` method body | ~520 |
| `editor/event_loop.rs` | `run()` method body | ~1,060 |
| `editor/folding.rs` | `fold_toggle`, `fold_close_all`, `fold_open_all` | ~125 |
| `editor/inline_assist.rs` | `poll_inline_assist`, `strip_assist_fence`, `handle_inline_assist_mode` (from input.rs) | ~150 |
| **mod.rs remainder** | Struct def, `new()`, accessors, small helpers | ~350 |

### `editor/actions.rs` → 2 new files

| New File | What Moves | Lines |
|----------|------------|------:|
| `editor/surround.rs` | `surround_pair`, `find_surround_on_line`, `apply_surround_*` (lines 1,040–1,134) | ~95 |
| `editor/text_objects.rs` | `text_object_range`, `set_selection_range`, `apply_text_object_*` (lines 1,131–1,215) | ~85 |

### `agent/agentic_loop.rs` → 2 new files

| New File | What Moves | Lines |
|----------|------------|------:|
| `agent/streaming.rs` | SSE parsing loop (lines 237–455) as `parse_sse_stream()` | ~220 |
| `agent/tool_dispatch.rs` | Tool exec + snapshot + compression phase (lines 466–661) | ~200 |

### `agent/mod.rs` / `agent/panel.rs` → 2 new files

| New File | What Moves | Lines |
|----------|------------|------:|
| `agent/context.rs` | `ContextBreakdown`, `SubmitCtx`, `message_importance()`, `compress_history()` | ~200 |
| `agent/session.rs` | `metrics_data_path()`, `history_file_path()`, `append_session_metric()`, `revert_session()`, `has_checkpoint()` | ~120 |

---

## Performance Issues Found (Agent Module)

These are logic problems, separate from the structural refactor — fix in a dedicated phase.

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| 1 | `messages.clone()` per round | `agentic_loop.rs` lines 194, 214 | Change `start_chat_stream_with_tools` to accept `&[Value]` |
| 2 | `tool_defs.clone()` per round | `agentic_loop.rs` lines 195, 215 | Wrap in `Arc<Value>` |
| 3 | `sse_buf.drain(..=pos)` per line | `agentic_loop.rs` SSE loop | Index cursor + single drain per chunk |
| 4 | `UnboundedSender<StreamEvent>` | `panel.rs`, `agentic_loop.rs` | `mpsc::channel(128)` with backpressure |
| 5 | `std::fs::read_to_string` in async | `tools.rs` | `tokio::fs::read_to_string(...).await` |

---

## Test Baseline (as of 2026-04-09)

| File | Tests |
|------|------:|
| `src/editor/hooks.rs` | 5 |
| `src/buffer/buffer.rs` | 3 |
| `src/treesitter/mod.rs` | ~10 |
| `src/treesitter/query.rs` | ~8 |
| `src/spec_framework/mod.rs` | ~5 |
| `src/spec_framework/spec_slicer.rs` | ~4 |
| `src/highlight/mod.rs` | ~3 |
| **Total** | **~38** |

Files being refactored (`editor/mod.rs`, `actions.rs`, `agent/agentic_loop.rs`, `agent/panel.rs`, `agent/tools.rs`) have **zero tests**. Characterization tests must be written before any structural moves.
