# ADR 0021 — Render Loop Performance Optimisations

**Status:** Accepted

---

## Context

Users reported occasional high CPU usage during normal editing. A static audit of the
hot path identified four independent sources of unnecessary work:

1. **Syntect re-run every frame** — `highlight_line()` created a fresh `HighlightLines`
   parser object for every visible line (~30) on every loop iteration (~20 FPS), even
   when the buffer content and scroll position were completely unchanged.  Estimated cost:
   600 syntect invocations per second while simply reading code.

2. **Per-character `String` allocation in `render_highlighted_line`** — The visual-
   selection renderer walked every character on every highlighted line, calling
   `ch.to_string()` to produce a single-character `Span`.  With a 100-char line and 30
   visible lines this produced ~3 000 small heap allocations per frame regardless of
   whether a selection was active.

3. **Unconditional 20 FPS render** — The main loop called `self.render()` on every
   iteration, including iterations where nothing had changed (no keypress, no LSP
   activity, no agent tokens, no completion result).  Pure waste during idle reading.

4. **Unbounded agent token drain** — `AgentPanel::poll_stream()` drained the entire
   channel in one go.  A fast-streaming LLM response (hundreds of tokens per iteration)
   could stall the render loop for multiple frames.

---

## Decision

Four targeted fixes were applied across four files — no architectural changes.

### Fix 1 — Highlight cache (`src/editor/mod.rs`)

A `HighlightCache` struct stores the rendered `Vec<Vec<Span<'static>>>` for the visible
viewport.  The cache key is `(buffer_idx, scroll_row, lsp_version)`.

```rust
struct HighlightCache {
    buffer_idx: usize,
    scroll_row: usize,
    lsp_version: i32,
    spans: Vec<Vec<Span<'static>>>,
}
```

Before calling syntect, `render()` checks whether all three key fields match the cached
values.  On a hit the stored spans are cloned cheaply (ratatui `Span` is ~40 bytes) and
syntect is never touched.  On a miss the viewport is highlighted and the result stored.

Cache is invalidated automatically by any edit (`lsp_version` increments on every
`insert_char`, `delete_char`, etc.) or scroll (`scroll_row` changes).

**Expected gain:** eliminates ~90–95 % of syntect CPU during normal navigation.

### Fix 2 — `render_highlighted_line` fast path (`src/ui/mod.rs`)

Added a `row_in_selection` guard before the character loop:

```rust
let row_in_selection = match &sel_range {
    None => false,
    Some((start, end)) => row >= start.row && row <= end.row,
};
```

- **Fast path** (`!row_in_selection`): clips spans to the viewport using the original
  span-level loop — no per-character `String` allocation.
- **Slow path** (`row_in_selection`): character-by-character walk to overlay the
  selection background colour, identical to the previous behaviour.

In Normal mode (no selection) and on the vast majority of rows in Visual mode, the fast
path is taken.

**Expected gain:** eliminates ~3 000 single-char `String` allocs per frame during normal
editing; slow path only activates for the handful of rows inside a visual selection.

### Fix 3 — `needs_render` dirty flag (`src/editor/mod.rs`)

`self.render()` is now guarded by a `needs_render: bool` that starts `true` (so the
first frame always draws) and is reset to `false` after each render.  It is set to
`true` only when something actually changed:

| Source | Trigger |
|--------|---------|
| Keyboard input | after `handle_key()` |
| LSP notifications | `process_messages()` returns `true` |
| LSP human messages | `drain_messages()` yields ≥ 1 entry |
| Agent stream tokens | `poll_stream()` returns `true` |
| Inline completion result | completion channel yields a value |
| Copilot auth event | auth channel yields a value |
| Background tasks in-flight | `copilot_auth_rx.is_some() \|\| pending_completion.is_some()` |

The LSP `process_messages()` return type was changed from `Result<()>` to `Result<bool>`
to expose whether any notification was processed.  Diagnostic cloning
(`get_diagnostics()`) is now also gated on `lsp_changed` so the `Vec<Diagnostic>` is
only cloned when the LSP actually delivered new data.

**Expected gain:** near-zero CPU when the editor is idle (user reading, not typing).

### Fix 4 — Agent token cap (`src/agent/mod.rs`)

`poll_stream()` now breaks after processing `MAX_TOKENS_PER_FRAME = 64` tokens:

```rust
token_count += 1;
if token_count >= MAX_TOKENS_PER_FRAME { break; }
```

Remaining tokens stay in the channel and are drained across subsequent frames.  64
tokens per 50 ms frame gives a sustained throughput of ~1 280 tokens/second — far above
any real LLM streaming rate — while bounding the worst-case time spent in `poll_stream`
per frame.

**Expected gain:** prevents multi-frame stalls during fast agent streaming responses.

---

## Consequences

**Positive**
- CPU usage during idle reading drops to near zero.
- Typing and cursor-movement latency is reduced (fewer allocations per keypress).
- Agent panel streaming no longer interferes with editor responsiveness.
- No user-visible behaviour change — all fixes are purely internal.

**Negative / trade-offs**
- The highlight cache adds ~`n_visible_lines × avg_spans_per_line × 40 bytes` of memory
  per open buffer (~5–15 KB typical).  Negligible in practice.
- `process_messages()` signature change is a minor API break inside the crate (no
  external consumers).
- The 64-token cap means very fast LLM responses take slightly more frames to fully
  render, but this is imperceptible to users (~3 ms at 20 FPS).
