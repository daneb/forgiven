# ADR 0058 — Agent Panel Rendering Performance

**Date:** 2026-03-10
**Status:** Accepted

---

## Context

### Bug: scroll viewport cut off on long responses

Users reported that the bottom portion of AI responses was not visible after streaming completed; submitting a new prompt was required to "unlock" the missing text.  The symptom was reproducible and worsened with longer "Plan" style agentic outputs.

**Root cause — `row_count` underestimated actual display rows.**

The old display-row estimator in `PanelRenderCache` used:

```rust
let cols: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
cols.div_ceil(inner_width).max(1)
```

This assumes each logical `Line` fills the widget width exactly, matching only hard character-count wrapping.  However ratatui's `Wrap { trim: false }` breaks at **word boundaries** — a code-block line such as `"  │ fn foo(arg1: T, arg2: U) -> Result<V>"` may wrap at the last space before the panel edge, producing more physical rows than `chars ÷ width` predicts.

The consequence:

```
max_scroll = total_display_rows − visible_height    ← underestimated
row_offset = max_scroll − scroll                     ← too small
visible window = [row_offset, row_offset + visible_height)
                                                     ← misses last N rows
```

The gap grew with response length and became obvious for plan outputs with many code and list lines.

### Hot-path allocation audit during streaming

With the scroll bug fix requiring `Paragraph::line_count()` (the authoritative ratatui word-wrap count), a second problem was identified: the fix was applied to the **combined** `lines` Vec, which is rebuilt from a clone of `msg_lines` on **every streaming frame** (~20 fps).  A three-agent review of the code revealed:

| Hot path (per streaming frame) | Old cost |
|---|---|
| `cache.msg_lines.clone()` → render Vec | O(N_history lines) clone |
| `lines.clone()` for `line_count()` | O(N_history + N_streaming) clone again |
| MCP bottom bar rebuild | `Vec<String>` + `format!()×N` + `join()` every frame |
| `scroll_suffix` static branches | Needless `String::new()` allocs |

---

## Decision

### 1 — Use `Paragraph::line_count()` for exact row counting

Enable the `unstable-rendered-line-info` ratatui feature and call:

```rust
Paragraph::new(lines.to_vec())
    .wrap(Wrap { trim: false })
    .line_count(inner_width as u16)
```

This is the same layout pass ratatui runs internally; the result is always exact regardless of word-wrap position.  Extracted into a small free function:

```rust
fn wrapped_line_count(lines: &[Line<'static>], inner_width: usize) -> usize {
    if inner_width == 0 || lines.is_empty() { return lines.len(); }
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(inner_width as u16)
}
```

### 2 — Split `row_count` into `msg_row_count` + `streaming_row_count`

Because `line_count` is additive across independent `Line` objects, the counts for completed messages and the current streaming reply can be cached **separately** under the same invalidation conditions that already govern their respective `Vec<Line>` caches:

| Field | Recomputed when |
|---|---|
| `msg_row_count` | `msg_count` or `content_width` changes (rare — on message completion or resize) |
| `streaming_row_count` | `streaming_len` or `streaming_width` changes (every streaming frame, but only over `streaming_lines`, which is the current reply only) |

Total rows passed downstream:

```rust
let total_display_rows = (cache.msg_row_count + cache.streaming_row_count + 2).max(1);
// +2 for the two trailing empty buffer Lines always appended to the render Vec
```

The old combined `row_count_key: (msg_count, streaming_len, inner_width)` and `row_count` fields are removed.

**Effect on the streaming hot path (400 history lines + 100 streaming lines):**

| Operation | Before | After |
|---|---|---|
| Clone for `line_count` | 500 `Line<'static>` every frame | 100 `Line<'static>` every frame (5× fewer) |
| `line_count` on msg half | Every frame | Only on msg change (rare) |

### 3 — Cache MCP status bottom bar

The `mcp_bottom` title line was rebuilt every render frame with `format!()`, `Vec<String>` collection, and `join()`.  MCP server state is fixed after startup.  The line is now cached in `PanelRenderCache` under key `(has_manager, failed_servers.len())` and only rebuilt when the key changes (in practice: once per session).

```rust
mcp_status_key: (usize, usize),
mcp_bottom: Option<Line<'static>>,
```

### 4 — `scroll_suffix` as `Cow<'static, str>`

The two static-string branches of the scroll suffix previously called `.to_string()`, allocating a heap `String` on every frame even for literal content.  Changed to `Cow::Borrowed` so only the `format!` branch (when the user has actually scrolled) allocates.

---

## Implementation

| File | Change |
|---|---|
| `Cargo.toml` | `ratatui = { version = "0.30", features = ["unstable-rendered-line-info"] }` |
| `src/ui/mod.rs` | `PanelRenderCache`: removed `row_count_key`/`row_count`; added `msg_row_count`, `streaming_row_count`, `mcp_status_key`, `mcp_bottom` |
| `src/ui/mod.rs` | Added `wrapped_line_count()` free function |
| `src/ui/mod.rs` | `msg_row_count` computed after `msg_lines` rebuild; `streaming_row_count` after `streaming_lines` rebuild |
| `src/ui/mod.rs` | Removed combined-`lines` row-count block; replaced with additive sum |
| `src/ui/mod.rs` | MCP bottom bar moved into `PANEL_CACHE.with` block with key-guarded rebuild |
| `src/ui/mod.rs` | `scroll_suffix` typed as `Cow<'static, str>` |

No new dependencies beyond the ratatui feature flag.

---

## Regression Detection

The following invariants should hold at all times.  A violation indicates a regression in scroll correctness or render performance:

### Correctness invariants

1. **Bottom always visible at `scroll = 0`** — after streaming `Done`, the last line of every response must be visible without any user input.  The end of a response that contains a fenced code block with long lines (≥ `panel_width` chars) is the highest-risk case.

2. **`wrapped_line_count` equals ratatui's own count** — by construction the two are the same function; any divergence means `wrapped_line_count` is not being called with the same `Wrap` setting as the render site.

3. **`msg_row_count` is stable during streaming** — the field must not change between streaming frames (only the `streaming_row_count` should change).  If `msg_row_count` updates every frame it means the `(msg_count, content_width)` cache key is falsely invalidating.

### Performance invariants

4. **One `streaming_lines` rebuild per render frame** — the `streaming_len != cur_streaming_len` guard must fire at most once per frame.  If `poll_stream` is called multiple times before a render, only the last state matters; the guard still holds.

5. **`msg_lines` is NOT rebuilt during streaming** — while `stream_rx.is_some()`, `msg_count` is constant (the in-flight reply is in `streaming_reply`, not `messages`).  Any rebuild of `msg_lines` during streaming is a bug.

6. **MCP bottom bar key changes at most once per session** — after the first `mcp_manager` is wired in, `mcp_status_key` should stabilise.  A key that keeps changing means something is mutating `failed_servers` at runtime.

### Key metrics to watch (manual profiling)

| Metric | Baseline (400 msg lines, 100 streaming lines) | Regression threshold |
|---|---|---|
| `Line<'static>` clones for `line_count` per frame | ~100 (streaming_lines only) | > 300 suggests combined-Vec path crept back |
| `msg_lines` rebuilds per streaming session | 0 (until final `Done`) | > 0 during streaming |
| MCP bottom bar rebuilds per session | 1 | > 5 |
| `streaming_lines` rebuilds per frame | ≤ 1 | > 1 |

### How to add tracing for the above

Instrument `PanelRenderCache` rebuilds with `tracing::trace!` gates (compiled out in release):

```rust
// in the msg_lines rebuild block:
tracing::trace!("msg_lines rebuild: {} messages, width {}", cur_msg_count, content_width);

// in the streaming_lines rebuild block:
tracing::trace!("streaming_lines rebuild: {} bytes", cur_streaming_len);

// in the MCP rebuild block:
tracing::trace!("mcp_bottom rebuild: key {:?}", mcp_key);
```

View with `RUST_LOG=forgiven=trace forgiven 2>trace.log` and inspect for unexpected frequencies.

---

## Consequences

- **Positive**: Scroll viewport bug eliminated — long responses including Plan/agentic outputs always show the full content at `scroll = 0`.
- **Positive**: Per-frame clone cost during streaming reduced ~5× for long conversations (history lines no longer cloned for row counting on every frame).
- **Positive**: MCP status bar no longer allocates on every frame (stable after startup).
- **Positive**: `wrapped_line_count` is a single canonical function; future call sites cannot accidentally diverge from the render behaviour.
- **Negative**: `unstable-rendered-line-info` is an unstable ratatui feature; if ratatui removes or renames it a minor API update will be required.  The feature exists since ratatui 0.27 and the tracking issue (#293) has been open for stabilisation.
- **Negative**: `wrapped_line_count` calls `lines.to_vec()` (a clone) on each cache miss.  For the `msg_lines` path this is rare; for `streaming_lines` it is per-frame but over a small slice, so the cost is acceptable.
