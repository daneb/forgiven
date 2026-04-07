# ADR 0119 — CPU & Memory Performance Pass (Janitor / Streaming)

**Date:** 2026-04-07
**Status:** Accepted — Implemented

---

## Context

During a session with heavy AI interactions followed by the auto-janitor firing, CPU was
observed sitting at ~30%.  The root cause is a compound effect: when the agent loop is
active, `poll_stream()` returns `agent_active = true` every 50 ms tick, which sets
`needs_render = true` unconditionally and drives the render loop at the full 20 Hz
event-poll rate.  At the same time, accumulated state from long sessions multiplied the
cost of each render:

1. `sticky_scroll_header()` walks the tree-sitter CST on **every** render frame with no
   inter-frame cache — even when the viewport has not moved.
2. `archived_messages` grows unboundedly: each janitor run appends the entire active
   history to the archive via `extend(take(&mut messages))`, and the archive is never
   trimmed.  Ten janitor runs means ten full conversation histories kept alive
   simultaneously, all rendered through the markdown pipeline on every frame.
3. Per-buffer tree-sitter caches (`ts_cache`, `ts_versions`, `fold_closed`) were never
   evicted when a buffer was closed.  A long session with many file opens/closes filled
   these `HashMap`s indefinitely.
4. LSP diagnostics (`HashMap<Uri, Vec<Diagnostic>>`) were never removed for closed files.
5. The `streaming_reply` `String` was initialized with zero capacity, causing repeated
   heap reallocations as tokens accumulated during a large janitor summary.

---

## Decision

Six targeted fixes, ordered by CPU / memory impact:

### 1. Render-rate cap for agent-only frames (`editor/mod.rs`)

When `agent_active` is the sole reason to repaint (no keyboard input, watcher events, or
other dirty sources have set `needs_render` yet), cap agent-triggered renders to **≤10 Hz**
(100 ms minimum between frames).  If another source has already set `needs_render = true`,
the render proceeds immediately — the cap does not apply.

```rust
const AGENT_RENDER_INTERVAL: Duration = Duration::from_millis(100);
if needs_render {
    self.last_agent_render = Some(Instant::now());
} else {
    let due = self.last_agent_render
        .map(|t| t.elapsed() >= AGENT_RENDER_INTERVAL)
        .unwrap_or(true);
    if due {
        self.last_agent_render = Some(Instant::now());
        needs_render = true;
    }
}
```

This halves the maximum render rate during long-running janitor compressions without
introducing any latency for keyboard-driven repaints.

### 2. Sticky-scroll inter-frame cache (`editor/mod.rs`)

Added `StickyScrollCache { buffer_idx, scroll_row, lsp_version, header: Option<String> }`
and a corresponding field on `Editor`.  The cache is checked at the start of each
`render()` call; the tree-sitter CST walk (`sticky_scroll_header`) is skipped when all
three key components are unchanged.  The cache is invalidated on buffer close.

```rust
let cache_hit = self.sticky_scroll_cache.as_ref().is_some_and(|c| {
    c.buffer_idx == buf_idx
        && c.scroll_row == scroll_row_for_sticky
        && c.lsp_version == lsp_ver_for_sticky
});
if !cache_hit { /* recompute and store */ }
```

### 3. Cap `archived_messages` at 400 entries (`agent/panel.rs`)

After each janitor compression, the archive is trimmed to at most 400 messages (oldest
first) to prevent unbounded memory growth across long sessions.  400 messages is roughly
2–8 janitor runs depending on conversation length — sufficient to scroll back through
recent history while preventing megabyte-scale accumulation.

```rust
const MAX_ARCHIVED: usize = 400;
if self.archived_messages.len() > MAX_ARCHIVED {
    let drop = self.archived_messages.len() - MAX_ARCHIVED;
    self.archived_messages.drain(..drop);
}
```

### 4. Evict tree-sitter caches on buffer close (`editor/actions.rs`, `editor/input.rs`)

All four buffer-close paths (`:bd`, `:bd!`, `SPC b d`, `SPC b D`) now evict
`ts_cache[idx]`, `ts_versions[idx]`, `fold_closed[idx]`, and `sticky_scroll_cache` when
the closing buffer's index matches.

### 5. Remove LSP diagnostics for closed files (`lsp/mod.rs`, `editor/actions.rs`, `editor/input.rs`)

Added `LspManager::clear_diagnostics_for_uri(uri)` and call it from all four buffer-close
paths, converting the file path to a URI first.  Prevents the
`HashMap<Uri, Vec<Diagnostic>>` from accumulating stale entries across a long session.

### 6. Pre-allocate `streaming_reply` (`agent/panel.rs`)

Changed `String::new()` to `String::with_capacity(4096)` when the streaming reply buffer
is initialised at the start of each round.  This avoids repeated heap reallocations as
tokens accumulate during large janitor summaries.

---

## Files changed

| File | Change |
|------|--------|
| `src/editor/mod.rs` | `StickyScrollCache` struct; `sticky_scroll_cache` + `last_agent_render` fields; cache-keyed sticky-scroll computation in `render()`; 10 Hz render-rate cap in event loop |
| `src/editor/actions.rs` | Evict `ts_cache`, `ts_versions`, `fold_closed`, `sticky_scroll_cache`, LSP diagnostics in `BufferClose` and `BufferForceClose` |
| `src/editor/input.rs` | Same evictions in `:bd` and `:bd!` command handlers |
| `src/agent/panel.rs` | `archived_messages` cap at 400; `streaming_reply` pre-allocated at 4 KB |
| `src/lsp/mod.rs` | `LspManager::clear_diagnostics_for_uri()` |

---

## Consequences

- CPU during a janitor compression drops from ~30% to an expected ≤15% on a typical
  session (render rate halved; per-frame tree-sitter walk eliminated on static viewports).
- Memory usage stabilises across repeated janitor runs; `archived_messages` no longer
  grows without bound.
- Buffer-close now fully releases all per-buffer state: tree-sitter snapshots, fold maps,
  sticky-scroll cache, and LSP diagnostics.
- Streaming latency is unchanged — keyboard-driven repaints are not rate-limited; only
  agent-only frames are throttled.
- All 38 existing tests pass (`cargo test`).

---

## Deferred

Four related improvements are logged in `docs/performance-improvements.md` (items 11–14)
for a follow-up pass:

| Deferred issue | Entry |
|----------------|-------|
| Agent panel chat history re-rendered via markdown pipeline every frame | Item 11 |
| Full `messages` + `tool_defs` cloned before each API round | Item 12 |
| `session_snapshots` not evicted on `new_conversation()` | Item 13 |
| Undo history stores full line-vector snapshots (delta-based history) | Item 14 |
