# Performance Improvement Areas

A catalogue of identified hotspots in the forgiven codebase, ordered roughly by expected
impact. Each area has a short description of the problem, where it lives, and what a fix
would look like. Tackle them one at a time.

---

## 1. Buffer Line-Array Cloned on Every Render Frame

**File:** `src/editor/mod.rs` ~line 681, ~line 777

Every call to `render()` snapshots the current buffer (and the split-pane buffer) with
`buf.lines().to_vec()`. For a 5 000-line file that is 5 000 `String` allocations per
frame — even when nothing changed.

**Fix ideas:**
- Wrap `Buffer::lines` in an `Arc<Vec<String>>` and `Arc::clone()` it instead of
  deep-copying. Any mutation replaces the `Arc` (copy-on-write), reads are zero-cost.
- Alternatively, store a `generation: u64` counter on `Buffer` and pass `&[String]`
  directly to the renderer, restructuring borrows so no owned copy is needed.

**Expected gain:** Eliminates the largest per-frame allocation for large files.

---

## 2. Highlight Cache Clones Spans on Every Hit ✓ DONE

**File:** `src/editor/mod.rs` ~line 709, ~line 800

Even on a cache hit the code does:
```rust
self.highlight_cache.as_ref().map(|c| c.spans.clone())
```
This still allocates a full `Vec<Vec<Span<'static>>>` copy every frame.

**Fix ideas:**
- Store `spans: Arc<Vec<Vec<Span<'static>>>>` in `HighlightCache` and
  `Arc::clone()` on hits. The rendering path only reads spans, so shared ownership is safe.
- Alternatively, pass `&[Vec<Span<'static>>]` into `UI::render` (borrowed from the
  cache) and avoid cloning altogether.

**Expected gain:** ~0 allocations on every non-scrolling frame; noticeable on fast
cursor movement.

**Implemented:** Changed `HighlightCache.spans` to `Arc<Vec<Vec<Span<'static>>>>` for
both the primary and split-pane caches. Cache hits now `Arc::clone()` (one atomic
increment) instead of deep-copying every span. Cache misses wrap the freshly built
`Vec` in an `Arc` before storing, then hand out an `Arc::clone` — so the cache and
caller share the same allocation. The UI call site uses
`as_deref().map(Vec::as_slice)` to borrow `&[Vec<Span<'static>>]` without any
further allocation. Added `use std::sync::Arc` to the import list.

---

## 3. Undo History Front-Removal is O(n) ✓ DONE

**File:** `src/buffer/history.rs` line 43, line 72, line 89

`EditHistory::past` and `future` are plain `Vec<BufferSnapshot>`. When the cap
(`MAX_SNAPSHOTS = 100`) is hit, `vec.remove(0)` is used to drop the oldest entry. That
shifts every remaining element — O(n) per operation.

**Fix:**
- Change `past: Vec<BufferSnapshot>` and `future: Vec<BufferSnapshot>` to
  `VecDeque<BufferSnapshot>`. `pop_front()` / `push_back()` are O(1).

**Expected gain:** Tiny in isolation, but correct and cheap.

**Implemented:** Switched both `past` and `future` to `VecDeque`. All `remove(0)` →
`pop_front()`, all `push()` → `push_back()`, all `pop()` → `pop_back()`.

---

## 4. Explorer `flat_visible()` Rebuilt Every Render Frame ✓ DONE

**File:** `src/explorer/mod.rs` line 199

`flat_visible()` walks the full node tree and allocates a new `Vec<&FileNode>` on
every call. It is called at least once per render frame while the explorer is visible.

**Fix ideas:**
- Add a `dirty: bool` flag to `FileExplorer`. Set it on expand/collapse/reload; clear it
  after rebuilding. Store a `flat_cache: Vec<...>` and return it on clean frames.
- Because the list holds `&FileNode` references (lifetimes), a cached version would need
  to store indices or owned copies instead.

**Expected gain:** Removes a tree-walk allocation on every cursor-move frame.

**Implemented:** Added `FlatNode` (owned snapshot struct with `path`, `name`, `is_dir`,
`is_expanded`, `depth`) to avoid self-referential lifetime issues. Added
`flat_cache: RefCell<Vec<FlatNode>>` and `cache_dirty: Cell<bool>` to `FileExplorer`
for interior-mutability caching (keeps `flat_visible` as `&self`). All tree-mutation
methods (`load_root`, `reload`, `toggle_node_at`, `toggle_hidden`) set `cache_dirty =
true`. `flat_visible()` now returns `Ref<'_, Vec<FlatNode>>` — on a cache hit it is
a single `RefCell::borrow()` with zero allocation; on a miss the tree is walked once
and the result stored. The UI's `render_file_explorer` is unchanged as `Ref` derefs
transparently and `FlatNode` exposes the same fields.

---

## 5. `scan_files()` Blocks the Event Loop

**File:** `src/editor/mod.rs` ~line 2745, called at line 1274

`scan_files()` does a recursive synchronous `fs::read_dir` walk on the main async
task. For large repos (tens of thousands of files) this can freeze the UI for hundreds
of milliseconds each time the file picker is opened.

**Fix:**
- Spawn `scan_files` in a `tokio::task::spawn_blocking` (or a dedicated `tokio::spawn`
  with blocking I/O). Send results back via a `oneshot` channel, same pattern already
  used for search and completions.
- Show a spinner in the PickFile overlay while the scan is in-flight.

**Expected gain:** File picker opens instantly; scan happens in background.

---

## 6. `refilter_files()` Runs Sequentially on Every Keystroke

**File:** `src/editor/mod.rs` ~line 2644

Every character typed in the PickFile query triggers a full sequential fuzzy-score pass
over `file_all` (potentially thousands of entries), plus a sort.

**Fix ideas:**
- Offload scoring to `rayon::par_iter()` for parallel scoring across CPU cores.
- Add a debounce (same pattern as completions/search) so scoring only fires after a short
  idle, not on each individual keypress.
- Limit displayed results to the first N (e.g. 200) after sorting — the user never sees
  more than the terminal height anyway.

**Expected gain:** Snappier typing in the file picker, especially in large repos.

---

## 7. Markdown Preview Re-Rendered on Every Frame ✓ DONE

**File:** `src/editor/mod.rs` ~line 758

While `Mode::MarkdownPreview` is active, `crate::markdown::render(&content, width)` is
called unconditionally on every render frame. Markdown parsing and line-wrapping are
non-trivial for large documents.

**Fix:**
- Cache the rendered `Vec<Line<'static>>` keyed on `(lsp_version, viewport_width)`.
  Reuse the cache when both are unchanged; regenerate on modification or terminal resize.
- This mirrors the existing `HighlightCache` pattern already in place.

**Expected gain:** Near-zero CPU for markdown preview when the user is just scrolling.

**Implemented:** Added `MarkdownCache { lsp_version, viewport_width, lines }` struct and
`markdown_cache: Option<MarkdownCache>` field on `Editor`. `render()` checks the cache
before calling `markdown::render`; invalidates on content change or terminal resize.

---

## 8. `Buffer::from_file` Recomputes `replace` Inside the Inner Loop ✓ DONE

**File:** `src/buffer/buffer.rs` line 122

```rust
let is_last_empty =
    i == content.replace("\r\n", "\n").matches('\n').count() && l.is_empty();
```

`content.replace("\r\n", "\n")` allocates a fresh `String` and `.matches('\n').count()`
counts all newlines — on **every iteration** of the `filter_map`. For a 10 000-line file
that is 10 000 redundant full-string copies.

**Fix:**
- Compute the newline count once before the iterator:
  ```rust
  let normalised = content.replace("\r\n", "\n");
  let last_idx = normalised.matches('\n').count();
  // then use `last_idx` inside filter_map
  ```

**Expected gain:** File open time for large files drops significantly.

**Implemented:** Hoisted `normalised` and `newline_count` out of the closure. The
`filter_map` now reads the pre-computed count with zero extra allocations.

---

## 9. In-File Search Scans Entire Buffer on Each Keystroke

**File:** `src/buffer/buffer.rs` — `update_search()` (called from keymap handler)

Every character typed in `/` search mode calls `update_search()` which scans all
lines of the buffer with a case-insensitive substring search. For large buffers this
blocks the event loop on each keypress.

**Fix ideas:**
- Debounce: wait ~150 ms of idle before running the search (same pattern as completions).
- Incremental search: as characters are appended, filter `search_matches` from the
  previous result set rather than rescanning from scratch.
- Offload to `tokio::task::spawn_blocking` for very large buffers.

**Expected gain:** `/` search stays responsive in large files.

---

## 10. Split Pane Takes Two Full Buffer Snapshots Per Frame

**File:** `src/editor/mod.rs` ~line 769–780

When a vertical split is active, both the focused and the background pane independently
call `buf.lines().to_vec()` in the same render frame. Combined with item 1, a split on
two large files doubles the per-frame allocation pressure.

**Fix:**
- This is resolved as a side-effect of fixing item 1 (Arc-based line storage). The split
  pane snapshot becomes a cheap `Arc::clone` rather than a deep copy.

**Expected gain:** Dependent on fix for item 1.

---

## Priority Order (Suggested)

| # | Area | Effort | Impact | Status |
|---|------|--------|--------|--------|
| 8 | `from_file` double replace in loop | Low | Medium | ✓ Done |
| 3 | Undo history `VecDeque` | Low | Low | ✓ Done |
| 7 | Markdown preview cache | Low | Medium | ✓ Done |
| 4 | Explorer `flat_visible` cache | Medium | Medium | ✓ Done |
| 2 | Highlight cache `Arc<spans>` | Medium | High | ✓ Done |
| 9 | In-file search debounce | Medium | Medium | — |
| 6 | `refilter_files` debounce + limit | Medium | Medium | — |
| 5 | `scan_files` async offload | Medium | High | — |
| 1 | Buffer lines `Arc<Vec<String>>` | High | High | — |
| 10 | Split pane snapshot (follows #1) | — | High | — |
