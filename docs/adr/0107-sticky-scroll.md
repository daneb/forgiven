# ADR 0107 — Sticky Scroll Context Header

**Date:** 2026-04-05
**Status:** Accepted

---

## Context

When editing a large function or method it is easy to lose track of which scope
the cursor is in once the function signature has scrolled above the viewport.
VS Code, Neovim (treesitter-context), Zed, and Helix all display a "sticky
scroll" overlay that pins the first line of the enclosing scope at the top of
the editor pane.

With ADR 0104's Tree-sitter AST in place, computing the enclosing scope for any
viewport position is a single `ancestor_matching` walk — the same primitive used
by text objects.

---

## Decision

### 1. Query: `sticky_scroll_header`

New public function in `src/treesitter/query.rs`:

```rust
pub fn sticky_scroll_header(snap: &TsSnapshot, scroll_row: usize) -> Option<String>
```

Returns the source text of the **first line** of the innermost function or
class ancestor whose `start_position().row < scroll_row`.

- Returns `None` when `scroll_row == 0` (nothing above the viewport).
- Returns `None` when the viewport top is not inside any scope.

### 2. Rendering

The header is computed once per frame in `Editor::render()` and passed to
`UI::render` via the new `RenderContext::sticky_header: Option<&str>` field.

`render_buffer` renders the header as a 1-row overlay at the top of the editor
`area` when `sticky_header` is `Some`:

```text
  fn process_events(&mut self, events: Vec<Event>) {   ← sticky header (dim)
  ┌─────────────────────────────────────────────────────
  │   for event in &events {
  │       match event {
  │           Event::Key(k) => self.handle_key(k),
```

The header row uses `DarkGray + DIM` styling to distinguish it visually from
editable content.  It is not interactive — the cursor cannot land on it.

### 3. Viewport adjustment

When a sticky header is shown the content viewport height is reduced by 1:

```
content_viewport_height = viewport_height - sticky_height
```

This is used when computing the buffer-data line range and the highlight-cache
line range, so neither over-fetches nor leaves the viewport partially blank.

The terminal cursor Y position is offset by `header_rows` (`0` or `1`) so
cursor placement is correct.

---

## Implementation

### `src/treesitter/query.rs`

`sticky_scroll_header(snap, scroll_row)` — calls `ancestor_matching` with a
predicate requiring `start_position().row < scroll_row` and the node being a
function or class node.

### `src/editor/mod.rs` — `render()`

After populating `ts_cache`:

```rust
let sticky_header_owned: Option<String> = self
    .ts_cache
    .get(&buf_idx)
    .and_then(|s| crate::treesitter::query::sticky_scroll_header(s, scroll_row));
```

`content_viewport_height = viewport_height - sticky_height` is used for
`buffer_data` and highlight-cache line ranges.

### `src/ui/mod.rs`

`RenderContext::sticky_header: Option<&'a str>` — the pre-computed header text.

### `src/ui/buffer_view.rs`

`render_buffer` receives `sticky_header: Option<&str>`.  When `Some`:

1. Renders a 1-row header `Paragraph` at `area.y` using the provided text.
2. Shifts the content area to `area.y + 1`, `area.height - 1`.
3. Adds `header_rows` to the terminal cursor Y position.

---

## Consequences

**Positive**

- Immediate, zero-latency context display — no LSP round-trip.
- Works for all Tree-sitter languages; degrades gracefully to no header for
  unsupported file types.
- Zero overhead when not scrolled into a scope (header is `None`).

**Negative / trade-offs**

- Only shows the **first line** of the scope, not a multi-line signature.
  Long or multi-line function signatures are truncated to one row.
- The header text is the raw source text; no syntax highlighting is applied
  (adding spans would require re-running syntect on a single line per frame,
  which is fast but adds complexity).
- The 1-row reduction in viewport height is a permanent cost when inside a scope
  (even if the user doesn't need the context).

**Future work**

- Apply syntect highlighting to the sticky header line.
- Show multiple levels of nesting (up to 2 scope lines) when deeply nested.
- Make sticky scroll optional via config (`sticky_scroll = true`).

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0104](0104-tree-sitter-core-integration.md) | TsEngine — prerequisite |
| [0105](0105-tree-sitter-text-objects.md) | `ancestor_matching` — reused |
| [0106](0106-code-folding.md) | Code folding — rendered in the same `render_buffer` pass |
