# ADR 0113 — Multi-File Review / Change Set View

**Date:** 2026-04-05
**Status:** Accepted

---

## Context

The agent can modify many files in a single session. The current tooling gives two
coarse-grained options:

- `SPC a u` — revert **all** agent-touched files at once (ADR 0112).
- Ctrl+A per chat block — apply/review a single file suggested in the last message
  (ADR 0035, subsequently removed).

Neither lets the user review what changed across all files and decide per-file
whether to keep or revert. Cursor, Zed, and Windsurf all offer a unified change-set
view with per-hunk or per-file accept/reject as a first-class workflow.

The missing piece in Forgiven is a mode that:

1. Shows every file the agent touched this session, side by side in one scrollable view.
2. Lets the user accept each file (keep current disk state) or reject it (restore
   from the pre-session snapshot).
3. Does not require leaving the editor or shelling out to `git diff`.

---

## Decision

### Data source

`AgentPanel::session_snapshots: HashMap<String, String>` (ADR 0112) already contains
the original content of every file the agent modified this session. The current state
is on disk. The diff is `snapshot → current_disk`.

No new data collection is needed — the review overlay is purely a view over data
that already exists.

### Diff algorithm

The `similar` crate (`similar = "2"`) produces Myers diffs from two strings. It is
pure safe Rust (compatible with `unsafe_code = "forbid"`), zero async overhead, and
requires no unsafe blocks. `TextDiff::from_lines` with `grouped_ops(3)` yields
standard unified-diff groups with three lines of context around each change, keeping
the view compact even for large files.

### New structs (`src/editor/mod.rs`)

```rust
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
    HunkStart(usize),   // carries hunk index; replaces old HunkSep
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verdict { Pending, Accepted, Rejected }

pub struct FileDiff {
    pub rel_path: String,
    pub lines: Vec<DiffLine>,
    pub hunk_verdicts: Vec<Verdict>,  // one per hunk
    pub original: String,             // pre-agent content (empty for new files)
    pub agent_version: String,        // post-agent content
}

pub struct ReviewChangesState {
    pub diffs: Vec<FileDiff>,
    pub scroll: usize,
    pub focused_file: usize,
    pub focused_hunk: Option<usize>,        // hunk focused for a/r actions
    pub file_offsets: Vec<usize>,
    pub hunk_line_offsets: Vec<Vec<usize>>, // flat line index of each HunkStart
}
```

`FileDiff::file_verdict()` derives a file-level verdict from `hunk_verdicts`:
all-Accepted → Accepted, all-Rejected → Rejected, otherwise Pending.

`apply_hunk_verdicts(original, agent_version, verdicts)` reconstructs the file
by emitting `agent_version` lines for accepted/pending hunks and `original` lines
for rejected hunks, using `TextDiff::grouped_ops(3)` to identify hunk boundaries.

### UX flow

```
1. [Normal / Agent]  SPC a r          → compute diffs, enter Mode::ReviewChanges
2. [ReviewChanges]   j / k            → scroll one line
3. [ReviewChanges]   Ctrl+D / Ctrl+U  → scroll half-page
4. [ReviewChanges]   ] / [            → jump to next / previous file
5. [ReviewChanges]   Tab / Shift+Tab  → cycle forward / backward through hunks
6. [ReviewChanges]   y                → accept focused file (all hunks)
7. [ReviewChanges]   n                → reject focused file (all hunks, restore)
8. [ReviewChanges]   a                → accept focused hunk only
9. [ReviewChanges]   r                → reject focused hunk only (partial restore)
10. [ReviewChanges]  Y                → accept all pending files
11. [ReviewChanges]  N                → reject all pending files
12. [ReviewChanges]  q / Esc          → close, return to Mode::Normal
```

`SPC a r` is a no-op (status message) when `has_checkpoint()` is false.

### Keybind

`SPC a r` → `Action::ReviewChangesOpen` — registered in the `agent` sub-tree
alongside `SPC a u` (revert session).

Mnemonic: **r**eview changes.

### Rendering

A full-screen overlay rendered via a new `UI::render_review_changes_overlay()`
function in `src/ui/popups.rs`.

Layout:

```
╭─ Review Changes  (1/3)  y=accept  n=reject  Y/N=all  [/]=jump  q=quit ──────╮
│                                                                               │
╰───────────────────────────────────────────────────────────────────────────────╯
 ── src/agent/panel.rs [pending] ───────────────────────────────────────────────
  ···
    use super::provider::{ProviderKind, ProviderSettings};
  - pub fn old_method(&self) {
  + pub fn new_method(&self) {
    }
  ···

 ── src/editor/mod.rs [accepted] ───────────────────────────────────────────────
  ···
```

- 3-row header block with cyan border: file counter + key hints.
- File header lines: cyan bold, highlighted when focused.
- `+` lines: light green. `-` lines: red. Context lines: dark gray.
- `···` hunk separators: dark gray.
- Verdict badge in file header: `[pending]` yellow, `[accepted]` green,
  `[rejected]` red.
- Status bar shows `REVIEW` in light green.

### Accept / reject

**Accept (`y` or `Y`):**
Mark `verdict = Accepted`. The current disk state is already what the user wants —
nothing is written.

**Reject (`n` or `N`):**
1. Read `original` from `session_snapshots` for that path.
2. Write `original` back to disk (`fs::write`).
3. Push the path to `agent_panel.pending_reloads` so any open buffer reloads.
4. Mark `verdict = Rejected`.

After `y` / `n`, focus advances to the next `Pending` file automatically.

### Lifecycle

- Entering `Mode::ReviewChanges` builds `ReviewChangesState` fresh from the current
  `session_snapshots` and disk state. It is stored on `Editor` as
  `Option<ReviewChangesState>`.
- `q` / `Esc` drops the state and returns to `Mode::Normal`.
- `new_conversation()` clears `session_snapshots` (ADR 0112); a subsequent
  `SPC a r` would show "No agent changes to review".
- Rejecting files does NOT clear their entry from `session_snapshots` — a second
  `SPC a r` after `n` would show the same file with its original content as "current"
  (because it was just restored), so the diff would show no changes. This is
  acceptable and self-consistent.

---

## Files modified

| File | Change |
|------|--------|
| `Cargo.toml` | Add `similar = "2"` |
| `src/keymap/mod.rs` | Add `Mode::ReviewChanges`; add `Action::ReviewChangesOpen`; register `SPC a r` leaf |
| `src/editor/mod.rs` | Add `DiffLine`, `Verdict`, `FileDiff`, `ReviewChangesState`; add `review_changes: Option<ReviewChangesState>` field; init to `None`; populate render context |
| `src/editor/actions.rs` | Handle `Action::ReviewChangesOpen` |
| `src/editor/input.rs` | Route `Mode::ReviewChanges` to handler |
| `src/editor/mode_handlers.rs` | Add `handle_review_changes_mode()` |
| `src/ui/status.rs` | Add `Mode::ReviewChanges` arms |
| `src/ui/mod.rs` | Add `review_changes` field to `RenderContext`; call `render_review_changes_overlay` |
| `src/ui/popups.rs` | Add `render_review_changes_overlay()` |

---

## Alternatives considered

### Shell out to `git diff`
Pro: handles binary files, submodules, untracked files.
Con: requires git; adds subprocess latency; ties the feature to git presence.
The in-memory snapshot approach (ADR 0112) already covers all agent-touched files
without any git dependency.

### Per-hunk accept/reject
Implemented. `HunkStart(usize)` carries a hunk index; `hunk_verdicts: Vec<Verdict>`
tracks per-hunk decisions. `apply_hunk_verdicts()` does partial file reconstruction.
`Tab`/`Shift+Tab` navigate hunks; `a`/`r` accept/reject the focused hunk.

### Separate diff buffer / split pane
Rendering the diff into a live editor buffer (like a `git diff` pane) would let the
user edit the diff directly. This is complex layout work and out of scope. The
full-screen overlay is simpler and sufficient.

---

## Consequences

**Positive**

- Users can review and selectively accept/reject individual files without touching `git`.
- Reuses `session_snapshots` from ADR 0112 — no new data collection in the agentic loop.
- `similar` is the only new dependency; no unsafe code.
- Per-file reject delegates to the same `fs::write` + `pending_reloads` path as
  `revert_session()`, keeping the restoration logic consistent.

**Negative / trade-offs**

- `similar` adds a compile-time dependency (~100 KB compiled). Acceptable for a dev tool.
- Files created by the agent appear in the review with `original = ""`. Rejecting them
  deletes the file from disk (consistent with ADR 0112 `revert_session()`).

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0007](0007-vim-modal-keybindings.md) | Mode enum — `ReviewChanges` is a new mode |
| [0035](0035-agent-apply-diff.md) | Previous per-file diff overlay (removed) |
| [0112](0112-agent-checkpoints.md) | `session_snapshots` is the data source |
| [0111](0111-inline-assistant.md) | Pattern for mode-scoped overlay state |
