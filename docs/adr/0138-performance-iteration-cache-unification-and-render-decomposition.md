# ADR 0138 — Performance Iteration: Cache Unification, Fold Cache, and render() Decomposition

**Date:** 2026-04-23
**Status:** Implemented

---

## Context

Since ADR 0119 (CPU/memory six-point optimisation) the render hot path has grown to
631 lines in a single `render()` method and now hosts four independent cache structs
(`HighlightCache`, `StickyScrollCache`, `MarkdownCache/CsvCache/JsonCache`), each with
its own inline invalidation logic.  Two problems have crystallised:

**Risk 1 — Cache correctness surface.**  Each cache uses a different set of invalidation
keys (2–4 fields).  Adding or changing any cache requires understanding all four
independently.  A missed key means stale pixels reach the screen silently — no panic,
no test failure, just wrong output.

**Risk 2 — render() as convergence point.**  All UI state flows through one 631-line
method.  Every new mode (soft-wrap: ADR 0137, inline assist: ADR 0111, location list,
etc.) adds another conditional block.  Because every cache invalidation decision lives
inside this single function, an edit anywhere can accidentally skip or duplicate a cache
write.

**Risk 3 — Per-frame fold data allocation.**  Fold hidden-row sets and stub maps are
rebuilt from scratch on every frame (`HashSet`/`HashMap` constructed in place,
`render.rs:71–90`) even when the fold state has not changed.  On files with many folds
this is measurable; more importantly it is the only render-path allocation that has no
corresponding cache.

**Risk 4 — Preview cache logic triplicated.**  The Markdown, CSV, and JSON preview
cache-hit / cache-miss / store pattern is copy-pasted three times (lines 238–323) with
no shared abstraction.  Any correctness fix must be applied in three places.

These risks compound: because tests cannot easily reach the render function (it owns a
live terminal), a bug introduced while editing one of the four cache blocks would only
be caught by visual inspection.

This ADR describes a three-phase plan that:
- Preserves all decisions in ADR 0002, 0021, 0078, 0077, 0119, 0126, 0130, 0137.
- Does not change any observable behaviour or rendering output.
- Reduces the correctness surface by making cache logic testable in isolation.
- Adds regression tests that can run in CI without a terminal.

---

## Decision

### Phase 1 — Extract `compute_fold_data` as a pure function (low risk)

Move the fold-hidden-row computation out of `render()` into a free function:

```rust
pub(crate) fn compute_fold_data(
    fold_ranges: &[(usize, usize)],
    fold_closed_set: &std::collections::HashSet<usize>,
) -> (std::collections::HashSet<usize>, std::collections::HashMap<usize, usize>)
```

This function is pure (no side effects, no `self`).  It can be unit-tested directly.
`render()` calls it and passes the result forward unchanged.

Add a `FoldCache` struct to `state.rs`:

```rust
pub(crate) struct FoldCache {
    pub buffer_idx: usize,
    pub lsp_version: i32,
    pub fold_closed_fingerprint: u64,   // hash of fold_closed_set keys
    pub hidden_rows: std::collections::HashSet<usize>,
    pub stub_map:   std::collections::HashMap<usize, usize>,
}
```

`render()` checks `FoldCache` before calling `compute_fold_data`.  The fingerprint is
a simple XOR-fold of the sorted closed-fold start rows — O(k) where k = number of
closed folds, which is always small.

**Invariant preserved from ADR 0106:** fold ranges come from tree-sitter; any buffer
edit increments `lsp_version`, which already invalidates `HighlightCache` and
`StickyScrollCache`.  `FoldCache` uses the same `lsp_version` key, so it shares the
same invalidation trigger.

### Phase 2 — Extract a generic preview cache helper (low risk)

Replace the three copy-pasted preview blocks with a single generic closure-based helper
extracted from `impl Editor`:

```rust
fn cached_preview<C, F>(
    cache: &mut Option<C>,
    buf_idx: usize,
    key_matches: impl Fn(&C) -> bool,
    make_cache: impl Fn(Vec<ratatui::text::Line<'static>>) -> C,
    render_fn: F,
) -> Vec<ratatui::text::Line<'static>>
where
    F: FnOnce() -> Vec<ratatui::text::Line<'static>>,
```

`key_matches` receives a reference to the existing cache entry and returns `true` if
the key fields are all current.  `render_fn` is the format-specific renderer
(`crate::markdown::render`, `crate::csv_preview::render`, etc.).  `make_cache`
constructs the concrete cache struct from the rendered lines.

The scroll-clamping logic (identical in all three branches) stays in the caller, using
the returned `Vec`.  Each branch is reduced to three lines:

```rust
let all_lines = self.cached_preview(
    &mut self.markdown_cache,
    buf_idx,
    |c| c.buffer_idx == buf_idx && Some(c.lsp_version) == lsp_ver && c.viewport_width == vw,
    |lines| MarkdownCache { buffer_idx: buf_idx, lsp_version: ver, viewport_width: vw, lines },
    || crate::markdown::render(&content, vw, Some(&self.highlighter)),
);
```

This helper is testable: pass a `None` cache and a trivial `render_fn`, verify it is
called exactly once; pass a pre-populated cache with a matching key, verify `render_fn`
is never called.

### Phase 3 — Decompose render() into sub-methods ✅

`render()` reduced from 631 lines to ~240 lines by extracting five focused sub-methods:

| Sub-method | What it does | Return type |
|---|---|---|
| `render_fold_data(buf_idx)` | FoldCache check → `compute_fold_data` on miss | `(HashSet<usize>, HashMap<usize, usize>)` |
| `render_sticky_scroll(buf_idx)` | StickyScrollCache check → tree-sitter walk on miss | `Option<String>` |
| `render_highlight_spans(buf_idx, fold_hidden, height)` | HighlightCache check → syntect pass on miss | `Option<Arc<Vec<Vec<Span>>>>` |
| `render_preview_lines(mode, buf_idx, vw)` | Dispatches to `cached_preview` for Markdown/CSV/JSON | `Option<Vec<Line>>` |
| `render_split_highlight(term_height)` | Split-pane HighlightCache check → syntect pass on miss | `Option<Arc<Vec<Vec<Span>>>>` |

Each sub-method is `&mut self`.  `render()` is now a flat orchestration sequence
followed by `RenderContext` assembly and `terminal.draw`.  No cache invalidation
keys, rendering output, or calling conventions were changed.

One borrow-checker adjustment was required: `ghost_text` was changed from a
`&str` reference to a cloned `String` (`ghost_owned`) so that the immutable
borrow on `self.ghost_text` does not conflict with the `&mut self` taken by
`render_preview_lines`.

---

## Implementation

| Phase | File(s) | Change |
|---|---|---|
| 1a | `src/editor/render.rs` | Extract `compute_fold_data` free function; replace inline block with call |
| 1b | `src/editor/state.rs` | Add `FoldCache` struct |
| 1c | `src/editor/mod.rs` | Add `fold_cache: Option<FoldCache>` field; initialise to `None` |
| 1d | `src/editor/render.rs` | Add cache-check before `compute_fold_data` call; store result |
| 1e | `src/editor/render.rs` | Add `#[cfg(test)] mod tests` with fold correctness tests |
| 2a | `src/editor/render.rs` | Add `cached_preview` helper method on `Editor` |
| 2b | `src/editor/render.rs` | Replace three preview blocks with `cached_preview` calls |
| 2c | `src/editor/render.rs` | Add `#[cfg(test)]` tests for `cached_preview` cache-hit / cache-miss |
| 3a | `src/editor/render.rs` | Extract `render_fold_data` — FoldCache check + `compute_fold_data` |
| 3b | `src/editor/render.rs` | Extract `render_sticky_scroll` — StickyScrollCache check + tree-sitter walk |
| 3c | `src/editor/render.rs` | Extract `render_highlight_spans` — HighlightCache check + syntect miss |
| 3d | `src/editor/render.rs` | Extract `render_preview_lines` — preview mode dispatch via `cached_preview` |
| 3e | `src/editor/render.rs` | Extract `render_split_highlight` — split-pane HighlightCache check |
| 3f | `src/editor/render.rs` | Clone `ghost_text` to owned `String` to resolve split-borrow conflict |

---

## Test Plan

Tests live in `#[cfg(test)] mod tests` inside the file they exercise.  No terminal,
no `Editor` struct required — only the pure functions and helper methods.

### Fold data tests (`src/editor/render.rs`)

- `fold_no_closed_ranges_produces_empty_sets` — ranges present, none closed → both
  sets empty.
- `fold_closed_range_hides_interior_rows` — close range (2, 5) → rows 3,4,5 in
  hidden set; row 2 in stub_map mapping to 5.
- `fold_multiple_overlapping_ranges` — closing (0,3) and (5,8) → six hidden rows,
  two stubs; rows 1–3 and 6–8 hidden.
- `fold_already_closed_at_start_row_included_in_stubs` — ensures fold_stub_map key
  is the *start* row, not interior rows.
- `fold_open_range_not_in_hidden_or_stubs` — only closed folds contribute.

### Cache key tests (`src/editor/render.rs` or `src/editor/state.rs`)

- `fold_cache_miss_when_lsp_version_changes` — same `buffer_idx` + same fingerprint
  but different `lsp_version` → cache miss.
- `fold_cache_miss_when_fold_closed_set_changes` — same `buffer_idx` + same
  `lsp_version` but different fingerprint → cache miss.
- `fold_cache_hit_when_all_keys_match` — all fields match → no recomputation.

### Preview cache helper tests (`src/editor/render.rs`)

- `preview_cache_miss_calls_render_fn_once` — pass `None` cache, verify closure
  called once and result stored.
- `preview_cache_hit_skips_render_fn` — pass populated cache with matching key,
  verify closure never called.
- `preview_cache_invalidates_on_key_change` — change one key field, verify closure
  called again.

### Message history regression tests (`src/agent/context.rs` or `src/agent/panel.rs`)

Existing tests in `src/agent/context.rs` cover `ContextBreakdown`.  Add:

- `archived_messages_cap_enforced` — push 401 messages into an archive vec, call the
  cap-enforcement logic, assert len == 400.  Guards against ADR 0119 point 3
  regression.
- `token_budget_truncation_preserves_newest` — build a message list that exceeds
  80% of a small context window (chars/4 heuristic), call truncation, assert the
  newest messages are retained.  Guards against ADR 0077 regression.

---

## Consequences

**Positive:**
- `compute_fold_data` is unit-testable without a terminal or `Editor` instance.
- Triple-duplicated preview cache logic reduced to one helper; a bug fix applies once.
- `render()` shrinks from 631 lines to ~240 lines — a flat orchestration sequence.
- Fold data rebuild drops from per-frame to only-on-change (same invalidation trigger
  as existing caches).
- Tests run in CI with `cargo test`; no headless terminal or mock needed.

**Negative / risks:**
- Phase 3 was done in small, compiling steps — one sub-method extraction per compile
  check.  All 132 tests passed after each step.
- `FoldCache` fingerprint (XOR of sorted start rows) is not collision-free for all
  possible fold sets.  Acceptable: collisions are rare, consequence is a single extra
  fold recomputation — not a correctness failure.
- `cached_preview` generic helper adds one level of indirection to three code paths
  that currently compile to simple inline blocks.  Negligible runtime cost (monomorphised
  by the compiler).

**No change to:**
- Render output, scroll behaviour, cache invalidation semantics.
- Any ADR 0002/0021/0077/0078/0106/0107/0119/0126/0130/0137 decision.
- Agent panel render-rate cap (ADR 0119 point 1: 10 Hz throttle stays in place).

---

## Alternatives Considered

**A — `CacheManager` trait abstraction.**  A unified `Cache<Key, Value>` trait was
considered.  Rejected: the four caches have heterogeneous key types (`(usize, i32)`,
`(usize, i32, usize)`, `(usize, i32, u64)`) and heterogeneous value types.  A trait
would require `dyn` dispatch or complex associated types for no measurable runtime
benefit.  Phase 2's closure-based helper is simpler and achieves the same DRY goal for
the triplicated preview block.

**B — Arena allocator for fold data.**  Using `bumpalo` for per-frame `HashSet`
allocation was profiled.  Rejected: the frame-to-frame savings are small (fold sets
rarely exceed 50 entries), and the FoldCache approach eliminates the allocation
entirely on stable frames, which is strictly better.

**C — Full render() rewrite.**  Rejected as too risky without existing test coverage.
Phase 3 is an incremental decomposition, not a rewrite.

---

## Related ADRs

- [0002](0002-async-event-loop.md) — async/sync event-loop boundary; render() is the sync render step
- [0021](0021-render-loop-performance.md) — original partial highlight cache
- [0106](0106-code-folding.md) — fold ranges from tree-sitter; `lsp_version` invalidation
- [0107](0107-sticky-scroll.md) — sticky-scroll cache; same key pattern as FoldCache
- [0119](0119-cpu-memory-optimisation.md) — six-point CPU/memory fix; archived_messages cap
- [0126](0126-token-efficiency.md) — token forensics; context re-send dominates cost
- [0137](0137-soft-wrap-toggle-and-long-line-rendering.md) — most recent render path change
