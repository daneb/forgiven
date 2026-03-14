# ADR 0063 — Structural Refactor: Buffer Combinator, RenderContext, and Editor Sub-states

**Date:** 2026-03-14
**Status:** Accepted

---

## Context

A maintenance audit identified three patterns in the codebase that compound
with every AI-assisted change. Because AI tooling follows the dominant pattern
already present in a file, each of these was on a trajectory to get worse
automatically — without any deliberate decision to let it.

### 1. Repeated buffer-access boilerplate

`if let Some(buf) = self.current_buffer_mut() { ... }` appeared **77 times**
across `editor/mod.rs`. Every new Vim motion, text operation, or action handler
added another copy of this guard. The pattern is mechanically correct but
creates a consistent +2 indentation levels on ~80 % of `execute_action`'s
match arms and scattered handlers.

### 2. `UI::render()` had 28 parameters

The function signature grew by 1–3 parameters with each new overlay or popup
mode. An `#[allow(clippy::too_many_arguments)]` suppression had already been
added to silence Clippy. Because AI generation follows existing call-site
patterns, each new mode would continue appending positional arguments.

### 3. `Editor` struct had 46 top-level fields

Mode-specific state (apply-diff overlay, split pane, commit message popup,
release notes popup) was stored as flat fields directly on `Editor`. New
features added fields alongside existing ones regardless of their logical scope.
The struct initialiser in `Editor::new()` required 14 explicit field initialisations
for these four feature groups alone.

---

## Decision

### 1. `with_buffer()` combinator

Two methods added to `Editor` immediately after `current_buffer_mut()`:

```rust
/// Apply a mutating closure to the current buffer, returning `Some(T)` on
/// success or `None` when no buffer is open.
#[inline]
fn with_buffer<T, F: FnOnce(&mut Buffer) -> T>(&mut self, f: F) -> Option<T> {
    self.current_buffer_mut().map(f)
}
```

69 of the 77 occurrences were converted. The eight remaining cases involve `?`
error propagation inside the block, complex multi-value returns used as `let`
bindings, or the render/cache pipeline — all correctly left as-is where
closure syntax would reduce rather than improve clarity.

`with_buffer_ref` (immutable variant) was drafted then removed: every
`current_buffer()` usage in the codebase also writes back to another `self`
field (e.g. `self.ghost_text`, `self.current_diagnostics`), making the `&self`
closure borrow impossible. The mutable variant covers all convertible cases.

### 2. `RenderContext` struct

All 27 non-`frame` parameters extracted into a single struct:

```rust
pub struct RenderContext<'a> {
    pub mode: Mode,
    pub buffer_data: Option<&'a BufferData>,
    pub status_message: Option<&'a str>,
    // … 24 more fields …
}
```

`frame: &mut Frame` remains a direct parameter — its mutable borrow lifetime
cannot be stored in a struct. The `#[allow(clippy::too_many_arguments)]`
suppression is removed. `UI::render` now takes two arguments:

```rust
pub fn render(frame: &mut Frame, ctx: &RenderContext<'_>) { … }
```

The function body is unchanged; 27 named locals are unpacked from `ctx` at
the top of the function. The call site in `editor/mod.rs` assembles a
`RenderContext` struct literal instead of a 28-argument positional call.

All fields are `Copy` (references, `bool`, `Option<Duration>`), so unpacking
is zero-cost.

### 3. Four Editor sub-structs

Fourteen fields extracted into four private named structs, each owning all
state for one Mode variant:

| Struct | Fields | `Editor` field |
|--------|--------|----------------|
| `SplitState` | `other_idx`, `right_focused`, `highlight_cache` | `self.split` |
| `ApplyDiffState` | `path`, `content`, `lines`, `scroll` | `self.apply_diff` |
| `CommitMsgState` | `buffer`, `rx`, `from_staged` | `self.commit_msg` |
| `ReleaseNotesState` | `count_input`, `rx`, `buffer`, `scroll` | `self.release_notes` |

All four derive `Default`. Non-default initial values (`CommitMsgState::from_staged
= true`, `ReleaseNotesState::count_input = "10"`) are expressed with struct
update syntax in `Editor::new()`:

```rust
commit_msg: CommitMsgState { from_staged: true, ..Default::default() },
release_notes: ReleaseNotesState { count_input: String::from("10"), ..Default::default() },
```

The two previously-`pub` split fields (`split_other_idx`, `split_right_focused`)
were found to have no external callers — the `pub` was vestigial. Both are now
private fields on `SplitState`.

`Editor` drops from **46 to 32** top-level fields.

---

## Consequences

**Positive**

- AI-generated Vim motions and text operations will use `self.with_buffer(|buf|
  ...)` by pattern matching, keeping `execute_action` flat and consistent.
- New overlay modes add one field to `RenderContext` and one struct literal
  field at the call site — the function signature is permanently stable.
- New mode-specific state belongs in a dedicated sub-struct; the `Editor`
  top-level field count grows by one (the struct) rather than N (the fields).
- `cargo clippy -- -D warnings` is fully green; the `too_many_arguments`
  suppression is gone.

**Negative / trade-offs**

- Field access for the four sub-struct groups is now one level deeper
  (`self.split.other_idx` vs `self.split_other_idx`). This is a minor
  readability trade-off judged worthwhile given the structural benefit.
- The `with_buffer` combinator cannot eliminate the 8 cases involving `?`
  propagation or complex return shapes. Those remain as explicit `if let` blocks.

---

## Alternatives considered

**Trait-based motion dispatch for `execute_action`**
Extracting motion groups into sub-traits or command objects would reduce
`execute_action` more aggressively but is a significantly larger refactor with
higher regression risk given the absence of unit tests for the editor layer.
Deferred.

**Single `EditorState` sub-struct for all mode-specific data**
Bundling all 14 fields into one `EditorState` was considered but rejected —
it provides no namespacing benefit and just relocates the problem one level
deeper.

**`RenderContext` by value instead of by reference**
Since all fields are `Copy`, `UI::render` could take `RenderContext<'_>` by
value. By-reference was chosen to keep the call site consistent with how
Rust APIs conventionally pass larger structs, and to leave the door open for
non-`Copy` fields in future without a signature change.
