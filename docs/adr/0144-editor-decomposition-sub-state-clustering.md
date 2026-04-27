# ADR 0144 — Editor Decomposition: Sub-State Clustering and Request Multiplexing

**Date:** 2026-04-27
**Status:** Phase 1 Implemented (LspState extraction)

---

## Context

The `Editor` struct at `src/editor/mod.rs` had grown to **84 fields** before this
change, spanning a dozen unrelated concerns: buffers, modes, LSP, MCP, search, file
explorer, inline assist, review-changes overlay, sidecar IPC, file watcher, agent
panel, sticky scroll, fold caches, three preview popups, and several ad-hoc per-mode
buffers (`rename_buffer`, `new_folder_buffer`, `lsp_rename_buffer`, …).

Among those 63 fields are **ten in-flight `oneshot::Receiver` values** — one per
async operation — each polled by hand in the 50 ms event loop:

```text
pending_completion         — inline completion
copilot_auth_rx            — Copilot device auth
search_rx                  — ripgrep project-wide search
insights_narrative_rx      — :insights summarize LLM
mcp_rx                     — MCP manager startup
pending_goto_definition    — LSP goto-definition
pending_references         — LSP find-references
pending_symbols            — LSP document-symbols
pending_hover              — LSP hover
pending_rename             — LSP rename
```

Two compounding problems:

**Problem 1 — God-struct bloat.** Adding a new concern (e.g. ADR 0129 insights
dashboard, ADR 0141 companion auto-launch, ADR 0131 intent translator) requires
threading 3–8 new fields onto `Editor`. The struct's `new()` constructor is now ~140
lines of field initialisation. Reading any submodule requires holding the entire
struct in your head because everything is one borrow scope. This is a primary cause
of the velocity decay flagged in the technical-debt review (2026-04-27).

**Problem 2 — Hand-rolled receiver polling.** Each pending receiver requires a
custom `match rx.try_recv() { Ok | Empty | Err }` block. For LSP this was already
extracted into a local `poll_lsp_rx!` macro at `event_loop.rs:327`, but the same
pattern is duplicated for `commit_msg.rx`, `release_notes.rx`, `search_rx`,
`insights_narrative_rx`, `mcp_rx`, `pending_completion`, `copilot_auth_rx`, and
several others — none of which use the macro because their result-handling
signatures differ. The structural cost is O(features) per feature added.

**ADR 0138 already proved the muscle exists.** The render-decomposition work
(Phase 1–3) collapsed a 631-line `render()` into ~240 lines plus five sub-methods
and four cache structs in `state.rs`. The same pattern applies here.

---

## Decision

A staged decomposition, executed as no-op refactors, in this order:

1. **Phase 1 — `LspState` extraction (this ADR)**: cluster the eleven LSP-related
   fields into a single `LspState` struct in `state.rs`. Editor exposes one field:
   `lsp: LspState`.
2. **Phase 2 — `SearchState` consolidation**: fold `search_rx`, `last_search_instant`,
   and `in_file_search_buffer` into the existing `SearchState`.
3. **Phase 3 — `ExplorerPopupState`**: cluster `rename_buffer`, `rename_source`,
   `delete_confirm_path`, `binary_file_path`, `new_folder_buffer`,
   `new_folder_parent`, `show_file_info` into one explorer-overlay struct.
4. **Phase 4 — `SidecarState`**: cluster the 6 sidecar/companion fields
   (`sidecar`, `last_sidecar_send`, `sidecar_last_cursor_line`,
   `sidecar_last_mode`, `sidecar_snapshot_pending`, `sidecar_last_buffer_idx`,
   `companion_process`, `sidecar_client_connected`) into a single struct.
5. **Phase 5 — `RequestDispatcher`**: replace the ten ad-hoc receiver fields with
   one multiplexer that owns `HashMap<RequestId, PendingRequest>` and dispatches
   results via typed callbacks. The event loop polls one collection.

After Phase 1–4 the `Editor` struct drops from 84 fields toward ~50. After Phase 5
the event-loop polling loop becomes O(1) in feature count.

### Phase 1 design (this ADR)

```rust
/// All LSP-related state owned by the Editor.
///
/// Clusters the LSP manager, current diagnostics, in-flight RPC receivers, and
/// per-mode UI overlays (location list, hover popup, rename input) into one
/// sub-struct. Replaces eleven loose fields on `Editor`.
#[derive(Default)]
pub(crate) struct LspState {
    /// Owns the per-language LSP client child processes.
    pub manager: LspManager,
    /// Diagnostics for the current buffer (refreshed when LSP publishes).
    pub diagnostics: Vec<Diagnostic>,

    // ── In-flight LSP RPCs (polled in event_loop.rs each tick) ──────────────
    pub pending_goto_definition: Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_references:      Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_symbols:         Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_hover:           Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_rename:          Option<oneshot::Receiver<serde_json::Value>>,

    // ── Per-mode overlay state ──────────────────────────────────────────────
    /// Mode::LocationList — populated by goto-definition / references / symbols.
    pub location_list: Option<LocationListState>,
    /// Mode::LspHover — popup body + scroll.
    pub hover_popup: Option<HoverPopupState>,
    /// Mode::LspRename — text typed into the rename prompt.
    pub rename_buffer: String,
    /// Mode::LspRename — origin URI + position to send to `textDocument/rename`.
    pub rename_origin: Option<(lsp_types::Uri, lsp_types::Position)>,
}
```

**Visibility:** `pub(crate)` struct, `pub` fields. Following the convention
established by `SplitState`, `CommitMsgState`, `ReleaseNotesState`. Public-field
access preserves Rust's split-borrow capability (callers can hold `&mut self.lsp.manager`
and `&mut self.lsp.diagnostics` at the same time, which they do in `event_loop.rs:63`).

**No accessor methods.** Direct field access (`self.lsp.diagnostics`) keeps the
refactor mechanical and preserves split borrows. Adding accessors becomes worth it
only when invariants need enforcing — none exist today.

**Behaviour preservation.** This is a pure structural rename. No call sites change
semantics. All 72 existing access sites become `self.lsp.<field>`.

---

## Implementation

Phase 1 changed:

| File | Change |
|---|---|
| `src/editor/state.rs` | Added `LspState` struct + `Default` impl. |
| `src/editor/mod.rs` | Removed 11 LSP fields; added `lsp: LspState`. |
| `src/editor/lsp.rs` | All `self.<lsp_field>` → `self.lsp.<field>`. |
| `src/editor/event_loop.rs` | `poll_lsp_rx!` arguments + `current_diagnostics` + `lsp_manager` accesses migrated. |
| `src/editor/{actions,ai,input,render}.rs` | `lsp_manager` / `current_diagnostics` / `location_list` / `hover_popup` / `lsp_rename_buffer` accesses migrated. |
| `src/editor/mod.rs` (constructor) | 11 field initialisers collapsed to `lsp: LspState::default()`. |

Net effect on `Editor` field count: **84 → 74** (eleven LSP fields removed, one
`lsp: LspState` added). Remaining field-clustering work is Phases 2–5; expected
trajectory is `~74 → ~50` after Phases 2–4.

Verified post-refactor metrics:

- `cargo build` clean.
- `cargo test`: 142 / 142 passing (no test count delta — pure rename).
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- `grep -rn "self\\.lsp\\." src/editor src/ui` returns 67 sites (the 72 grep'd
  prior to refactor included three RenderContext consumers in `ui/mod.rs` that
  legitimately remain `ctx.<field>`, plus a few constructor initialisers that
  collapsed into `LspState::default()`).

**No tests added** in Phase 1. The refactor is pure rename; existing tests cover
behaviour. Phase 5 (`RequestDispatcher`) will introduce a small unit-tested dispatcher
type — *that* phase is the right place for new tests.

---

## Consequences

### Positive

- `Editor` field count drops by 10 today; trajectory drops by ~33 across phases.
- LSP concern is now grep-able in one place (`grep -r "self\.lsp\." src/`).
- Future LSP RPCs (e.g. `textDocument/codeAction`, `workspace/executeCommand`) add
  one field to `LspState`, not one field to `Editor` plus another to the constructor.
- Phase 5 dispatcher is unblocked: each phase 1–4 sub-struct can register its own
  receivers with a single owner.

### Negative

- One indirection added at every LSP call site (`self.lsp.x` instead of `self.x`).
  Compiler-erased; no runtime cost. Cosmetic only.
- Diff churn across 8 files — git blame becomes one commit deeper for every
  migrated line. Mitigated by doing the refactor as a single commit.

### Neutral

- Public API surface unchanged. The two previously-`pub` fields
  (`location_list`, `hover_popup`, `lsp_rename_buffer`) had no external callers
  outside the `editor` module (verified via grep against `src/ui/`, `src/agent/`,
  etc. — only RenderContext consumers, which are constructed inside `render.rs`
  and remain unaffected).

---

## References

- Technical-debt review, 2026-04-27 — item C1.
- ADR 0138 — Render decomposition (precedent for state-struct extraction pattern).
- ADR 0003 — Original LSP integration architecture.
- ADR 0129 — Insights dashboard (example of god-struct accretion).

## Status of follow-up phases

| Phase | Status | Owner |
|---|---|---|
| 1. LspState extraction | Implemented (this ADR) | — |
| 2. SearchState consolidation | Pending | — |
| 3. ExplorerPopupState | Pending | — |
| 4. SidecarState | Pending | — |
| 5. RequestDispatcher | Pending | — |
