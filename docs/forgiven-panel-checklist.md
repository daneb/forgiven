# Forgiven Agent Panel — Redesign Checklist

> Tracks progress against `docs/agent-panel-redesign.md`.  
> Update status as each slice lands: `[ ]` → `[x]`  
> Each slice must have passing tests before marking done.

---

## Phase 0 — Panel Centrality & Layout

Goal: make the agent panel a first-class citizen in the layout, not a sidebar afterthought.

- [x] **P0-S1** — Audit current layout split ratios and panel z-order; document baseline
- [x] **P0-S2** — Promote agent panel to primary pane (increase default width allocation)
- [x] **P0-S3** — Add token budget status bar at panel bottom (live count, % of context window)
- [x] **P0-S4** — Add panel title bar with current model, provider, and session name
- [x] **P0-S5** — Tests: layout renders at 80-col, 120-col, 200-col terminal widths without overflow

---

## Phase 1 — Streaming & Copy UX

Goal: eliminate lethargy; make output feel instant and easy to interact with.

### 1a. Throughput fix
- [x] **P1-S1** — Raise `MAX_TOKENS_PER_FRAME` from 64 → 256; tune render tick to 50ms
- [x] **P1-S2** — Fix O(n) `PANEL_CACHE` markdown invalidation — invalidate per-paragraph, not full buffer
- [x] **P1-S3** — Benchmark: measure tokens/sec visible before and after (target: ≥ 1,500 tok/s)

### 1b. Copy UX
- [x] **P1-S4** — Add `AgentNavState` struct (active: bool, cursor_line: usize)
- [x] **P1-S5** — Tab toggles nav mode; `j`/`k` move cursor; highlight active line
- [x] **P1-S6** — `y` yanks current line to clipboard (use `arboard` or `cli-clipboard`)
- [x] **P1-S7** — Existing code-block copy (`Y` on fenced block) preserved and tested
- [x] **P1-S8** — Tests: nav state transitions, cursor boundary conditions, yank produces correct text

---

## Phase 2 — Harness Capabilities

Goal: project init, multi-language repo map via PageRank, session resume, planning mode.

### 2a. Repo map upgrade
- [x] **P2-S1** — Extend `build_structural_map()` to collect non-.rs files (py, ts, go, java, etc.)
- [x] **P2-S2** — Build symbol reference graph from existing `extract_symbols()` output
- [x] **P2-S3** — Implement PageRank over reference graph (damping=0.85, convergence 1e-6) — no new crates
- [x] **P2-S4** — Replace structural map injection with ranked repo-map (top-N files by score)
- [x] **P2-S5** — Token budget: repo map capped at configurable token limit (default 4,096)
- [x] **P2-S6** — Tests: PageRank stable on acyclic graph; ranking order matches known reference counts

### 2b. Session harness
- [x] **P2-S7** — Project init flow: on first run, emit constitution prompt and persist to `.forgiven/constitution.md`
- [x] **P2-S8** — Session resume: serialize `AgentHistory` to `.forgiven/sessions/<id>.json` on exit
- [x] **P2-S9** — Load most-recent session on startup (with opt-out key binding)
- [x] **P2-S10** — Planning mode: `/plan` command emits structured plan block; plan persisted to `.forgiven/plan.md`
- [x] **P2-S11** — Tests: init creates file, resume loads correct history, plan block parses correctly

---

## Phase 3 — Auto-Compaction & Token Management

Goal: automatic context hygiene; never hit the hard limit; transparent to user.

- [ ] **P3-S1** — Enable `janitor_threshold_tokens` at 70% of provider context window (was 0/disabled)
- [ ] **P3-S2** — Add hysteresis: compact fires once above 70%, resets at 50% — no thrash
- [ ] **P3-S3** — Two-step compact UX: show "⚡ Compacting context…" status, then "✓ Compacted (N→M tokens)"
- [ ] **P3-S4** — ADR invariant guard: MIN_RECENT=4 enforced, archive cap=400, history never modified in-place
- [ ] **P3-S5** — Per-tool cost attribution: display token delta per tool call in activity log
- [ ] **P3-S6** — `/compact` manual command mirrors auto-compact (existing infrastructure, just wire it up)
- [ ] **P3-S7** — Tests: janitor fires at correct threshold; hysteresis prevents double-fire; invariants hold post-compact

---

## Cross-cutting

- [ ] **CC-1** — All new public types/functions have rustdoc comments
- [ ] **CC-2** — `cargo clippy -D warnings` passes after each phase
- [ ] **CC-3** — `cargo test` full suite green after each phase
- [ ] **CC-4** — Update `docs/agent-panel-redesign.md` with any decisions that diverge from the plan

---

## Progress summary

| Phase | Slices | Done | Status |
|-------|--------|------|--------|
| P0 — Centrality | 5 | 5 | Complete |
| P1 — Streaming & Copy | 8 | 8 | Complete |
| P2 — Harness | 11 | 11 | Complete |
| P3 — Compaction | 7 | 0 | Not started |
| Cross-cutting | 4 | 0 | Not started |
| **Total** | **35** | **24** | — |
