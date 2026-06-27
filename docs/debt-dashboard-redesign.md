# Debt Dashboard Redesign

> Spec for improving the technical debt tracking on the Forgiven splash/landing screen.  
> Companion to `docs/agent-panel-redesign.md`.

## Problem Statement

The debt dashboard infrastructure is solid — background tokio task, mtime-invalidated cache, three well-defined metrics — but the numbers feel disconnected from lived experience for three reasons:

1. **No baseline or trend.** A snapshot number (`14 high-complexity fns`) has no emotional weight without knowing if it improved or regressed since the last scan.
2. **No entry point.** The welcome screen shows scores but offers no way to act on them. Identifying `editor/mod.rs::run_` as the worst offender is useful only if you can navigate there.
3. **New/small projects feel broken.** Zero ADRs renders the intent debt column empty. 100% cognitive activity on a young project is technically correct but meaningless.

## Current Implementation (Baseline)

| Concern | Location |
|---|---|
| Computation trigger | `src/main.rs` — tokio task at startup |
| Cache | `~/.local/share/forgiven/debt_cache.json` (mtime-sum invalidation, 1hr TTL) |
| Intent debt | `src/debt/intent.rs` — ADR status scan, velocity window |
| Technical debt | `src/debt/technical.rs` — Sonar-style complexity, unwrap/todo/FIXME counts |
| Cognitive debt | `src/debt/cognitive.rs` — git log 30d touch ratio, re-entry risk, tool errors |
| Render | `src/ui/buffer_view.rs` — `render_welcome()`, three equal-width columns |
| Narrative | Ollama summary, cached 24hr at `~/.local/share/forgiven/debt_narrative.txt` |

## Proposed Slices

### Slice A — Trend Delta

**Goal:** Give each metric number meaning by showing movement since the last scan.

**What to build:**
- Extend `CacheEntry` with a `previous: Option<DebtReport>` field. On each successful recompute, rotate current → previous before saving.
- In `render_welcome()`, compute per-metric deltas and annotate each figure:
  - `14 high-complexity fns (+2)` in red if regressed
  - `14 high-complexity fns (-3)` in green if improved
  - No annotation if unchanged (reduce noise)
- Delta display format: `(+N)` / `(-N)` — keep it terse, one character wide

**Acceptance tests:**
- Cache round-trips correctly with `previous` populated
- Delta sign is correct (regression = positive = red, improvement = negative = green)
- No delta shown on first run (no previous record)

---

### Slice B — Debt Detail Buffer + Jump-to

**Goal:** Make the welcome screen a doorway, not a dead end.

**What to build:**
- New keybinding `SPC d d` — opens a read-only `DebtDetail` buffer (virtual buffer, no file path)
- Buffer content: full ranked list of all debt items grouped by category, each rendered as a navigable line:
  ```
  TECHNICAL DEBT
    [HIGH]  editor/mod.rs:342  run_editor_loop  score=28
    [HIGH]  lsp/client.rs:89   handle_response  score=21
    ...
  COGNITIVE DEBT
    [STALE] graphics/  (last touched: 94 days ago)
    ...
  ```
- `Enter` on any file-linked line opens that file at the correct line number
- Key hint added to welcome screen footer: `SPC d d  debt detail`
- Buffer is regenerated from the cached `DebtReport` on open (no rescan)

**Acceptance tests:**
- Buffer opens without panic when debt report is None (shows "No debt report available — press SPC d r to scan")
- Enter on a file line navigates to correct file + line
- Buffer re-uses cached report (does not trigger a new scan)

---

### Slice C — Manual Refresh + New-Project Grace

**Goal:** Give users control over scan timing; make the dashboard honest on young projects.

**What to build:**

*Manual refresh:*
- New keybinding `SPC d r` — clears the cache and re-triggers `debt::compute()` on demand
- Shows a `[scanning…]` status indicator in the debt panel while running
- On completion, repaints the welcome screen with the fresh report

*New-project grace:*
- In `debt::compute()`, before running intent debt, check:
  - Fewer than 5 `.md` files in `docs/adr/` → suppress intent debt column, render dimmed: `"Add ADRs to track intent debt (docs/adr/)"`
  - Repo age (first commit date via `git log --reverse --format=%at | head -1`) < 30 days → suppress cognitive debt column, render dimmed: `"Cognitive metrics available after 30 days of commits"`
- These checks are fast (one git command, one directory count) and run before the heavier analysis

**Acceptance tests:**
- `SPC d r` triggers fresh compute regardless of cache age
- Project with 0 ADRs renders suppressed intent column (not zeros)
- Project with first commit < 30 days ago renders suppressed cognitive column

---

## Checklist

- [ ] **A-1** — Extend `CacheEntry` with `previous: Option<DebtReport>`
- [ ] **A-2** — Rotate current → previous on each recompute
- [ ] **A-3** — Render delta annotations in `render_welcome()`
- [ ] **A-4** — Tests: cache round-trip, delta sign, no delta on first run

- [ ] **B-1** — Add `DebtDetail` virtual buffer type
- [ ] **B-2** — Populate buffer from cached `DebtReport`, grouped + ranked
- [ ] **B-3** — `Enter` navigates to file + line
- [ ] **B-4** — `SPC d d` keybinding wired up
- [ ] **B-5** — Key hint on welcome screen footer
- [ ] **B-6** — Tests: opens without panic when no report, navigation correct

- [ ] **C-1** — `SPC d r` keybinding triggers forced rescan
- [ ] **C-2** — `[scanning…]` indicator during rescan
- [ ] **C-3** — ADR count grace check (< 5 ADRs → suppress intent column)
- [ ] **C-4** — Repo age grace check (< 30 days → suppress cognitive column)
- [ ] **C-5** — Tests: force rescan bypasses cache, grace conditions render correctly

- [ ] **CC** — `cargo clippy -D warnings` passes, `cargo test` green after all slices

## Progress Summary

| Slice | Items | Done | Status |
|---|---|---|---|
| A — Trend Delta | 4 | 0 | Not started |
| B — Detail Buffer | 6 | 0 | Not started |
| C — Refresh + Grace | 5 | 0 | Not started |
| Cross-cutting | 1 | 0 | Not started |
| **Total** | **16** | **0** | — |
