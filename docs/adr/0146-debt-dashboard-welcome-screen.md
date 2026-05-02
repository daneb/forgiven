# ADR 0146 — Debt Dashboard on the Welcome Screen

**Date:** 2026-05-02
**Status:** Implemented

---

## Context

Every session with forgiven begins on the welcome screen. That screen currently
shows the logo, tagline, key hints, and startup time. It answers the question
*"how do I start?"* but not *"how is the project doing?"*

A mature codebase accumulates three distinct categories of debt that compound
silently between sessions:

**Intent debt** is the gap between decisions made and decisions enacted. forgiven
uses Architecture Decision Records to capture design choices. An ADR at `Accepted`
status represents a conscious commitment; if no corresponding code materialises,
the intent is stranded. Over time the ADR index also ages: decisions recorded
eighteen months ago may no longer reflect the constraints that drove them, and
an index that grows faster than it is pruned becomes background noise rather than
a reference.

**Technical debt** is the accumulated structural cost of code that works but is
hard to re-enter. Cyclomatic complexity is the traditional measure, but it is a
poor proxy for reading difficulty because it counts branches flatly. A function
with five nested `if` blocks is far harder to reason about than five top-level
`if` statements, and an early `return` that short-circuits reading is neutral or
positive. The 2018 Sonar cognitive-complexity model (Campagne) directly penalises
nesting depth: each control-flow keyword scores `1 + current_nesting_level`. This
separates "complex to test" (cyclomatic) from "complex to read" (cognitive) — the
latter is what matters for maintenance velocity. Beyond complexity, explicit markers
— `todo!()`, `unimplemented!()`, `.unwrap()` outside test modules, `FIXME`/`HACK`
comments, `#[allow(dead_code)]` suppressions, `// Phase N` stubs — are a developer
signalling future obligations to themselves. Counting them surfaces the implicit
backlog.

**Cognitive debt** is the risk that a developer has lost the mental model of part
of the codebase and would need significant re-orientation before making correct
changes. This is distinct from technical quality: a well-factored module that
hasn't been touched in two months is cognitively distant even if it is clean.
Research basis: Fritz et al. (2010) showed that developer expertise is reflected
directly in code-interaction patterns — experts navigate to code; novices search
for it. Ko et al. (2006) found that developer familiarity with a module is the
strongest predictor of change correctness. Cherubini et al. (2007) showed that
when a developer's mental model diverges from the code, bug introduction rates
rise. The operational definition here is: git-log activity in the last 30 days
gives an active-surface percentage; the cross-product of high cognitive complexity
and no recent activity identifies *re-entry risk functions* — the ones most likely
to surprise on return.

The three categories are complementary rather than redundant. A project can have
low technical debt (clean, tested, well-structured) but high cognitive debt (nothing
touched in six weeks). It can have low cognitive debt (actively worked every day)
but high intent debt (twenty ADRs decided but unbuilt). Seeing all three together
on every launch creates a low-friction habit of monitoring.

The constraint throughout was: **meaningful or nothing**. Every signal displayed
must be actionable. Raw counts without context (e.g., "47 TODOs") fail this bar;
named hotspots, ratios, and trend indicators pass it.

---

## Decision

### Module structure

A new `src/debt/` module owns all debt computation and caching. It is deliberately
separate from `src/insights/` — that module tracks *session behaviour* (what you
did); this module tracks *codebase health* (what the code looks like).

```
src/debt/
├── mod.rs        # DebtReport, IntentDebt, TechnicalDebt, CognitiveDebt; compute()
├── intent.rs     # ADR directory analysis
├── technical.rs  # Static file scanning + cognitive complexity scorer
├── cognitive.rs  # Git log + sessions.jsonl analysis
├── cache.rs      # JSON cache with mtime fingerprint invalidation
└── narrative.rs  # Optional Ollama single-turn narrative
```

### Metric definitions

#### Intent debt (`IntentDebt`)

| Field | Derivation |
|-------|-----------|
| `total_adrs` | Count of `.md` files in `docs/adr/` |
| `implemented` | Files where `**Status:**` contains `Implemented` |
| `accepted_pending` | `Accepted` but not `Implemented` |
| `proposed` | `Proposed` status — undecided questions |
| `superseded` | `Superseded` — dead weight in the index |
| `stale_count` | Active (Accepted/Proposed) files not modified in 18+ months |
| `recent_velocity` | Files modified in the past 6 months |

`implementation_rate = implemented / (implemented + accepted_pending)` is the
headline percentage. The staleness threshold (18 months) is calibrated to the ADR
lifecycle: a decision should be reflected in code within one development cycle or
the decision was premature. The velocity signal answers "is the ADR practice still
alive?" — slowing velocity can indicate either project maturity (fewer new decisions
to capture) or loss of confidence in the system.

ADR staleness uses filesystem mtime rather than `git log --follow` because mtime
is synchronous, zero-subprocess-overhead, and accurate for an actively developed
project (not a fresh clone). The 18-month threshold is not a hard rule; it flags
for re-evaluation, not deletion.

#### Technical debt (`TechnicalDebt`)

**Cognitive complexity scorer** (primary signal):

Implemented as a brace-balance heuristic per function rather than a full tree-sitter
AST walk. For each line of a function body:

1. Track brace depth (`{` increments, `}` decrements).
2. For each control-flow keyword (`if`, `else if`, `else`, `while`, `for`, `loop`,
   `match`): add `1 + depth_at_keyword`.
3. For each `&&` or `||`: add `1` (boolean short-circuit, flat).

This matches the Sonar cognitive-complexity intuition without requiring per-language
grammar queries. It ranks functions correctly: deeply nested conditionals score much
higher than equivalent flat structures, and an early `return` scores zero. Functions
with score ≥ 15 are flagged "high"; ≥ 25 are "critical". The top-3 worst sites are
named in the dashboard.

Tree-sitter is available in the codebase and used for folding, sticky scroll, and
text objects; it was deliberately not used here because (a) the brace-based scorer
is already accurate enough for ranking and (b) full AST traversal per file at startup
would add 200–400 ms to the analysis pass for no material gain in signal quality.

**Explicit debt markers** (secondary signals):

| Signal | Pattern |
|--------|---------|
| `todo_macros` | `todo!()` / `unimplemented!()` — explicit future obligations |
| `unwraps_outside_tests` | `.unwrap()` not inside `#[cfg(test)]` — panic surface |
| `fixme_comments` | Lines starting `//` containing `FIXME`, `HACK`, or `XXX` |
| `dead_code_suppressed` | `#[allow(dead_code)]` attributes — hidden debt suppressions |
| `phase_comments` | `// Phase N` patterns — multi-phase work not yet complete |
| `long_files` | Files > 500 LOC — cognitive load surface |
| `test_module_ratio` | Modules with `#[cfg(test)]` / total modules — structural coverage |

The dashboard combines `todo_macros + unwraps_outside_tests` into a single "markers"
line to avoid overwhelming detail. Named worst-complexity sites provide the specific
file and function to act on.

#### Cognitive debt (`CognitiveDebt`)

**Active surface** (`git log --since=30.days.ago --name-only --pretty=format:`):

The output is a list of file paths touched in the last 30 days. Active surface
percentage = `recently_touched / total_src_files`. Stale top-level `src/`
subdirectories (those absent from the git-log output) are named explicitly —
`mcp/`, `graphics/`, `sidecar/` — because a module name is more actionable
than a percentage.

The 30-day window is chosen to match a typical sprint or release cycle. It is
not configurable in this version; it should be validated against actual usage
patterns before making it a config value.

**Re-entry risk**:

For each `.rs` file not in the recent-touch set, compute cognitive complexity
scores for all functions. Functions with score ≥ 15 in untouched files are
re-entry risks: complex enough to require careful reading, distant enough that the
mental model has likely faded. The top-3 are named in the dashboard. This is the
operationalisation of the Fritz/Ko/Cherubini findings: the intersection of
structural complexity and temporal distance predicts where errors are most likely
on return.

**Tool-error hotspots**:

`sessions.jsonl` already records `tool_error` events (ADR 0129 Phase 2). Grouping
by tool name surfaces which operations the developer repeatedly gets wrong — `read_file`
errors indicate path confusion; `edit_file` errors indicate structural unfamiliarity
with the target area. This is a Fritz (2010) -style behavioural expertise signal
derived from existing instrumentation.

### Computation architecture

The computation follows the established editor pattern: spawn a tokio task, deliver
the result via `oneshot::Receiver`, poll non-blocking in the 50 ms event loop. No
new patterns, no new primitives.

```
main()
  ├── setup_services()                       # LSP + MCP startup (existing)
  ├── tokio::spawn(debt::compute()) ──tx──► debt_rx  ← stored on Editor
  └── editor.run()
        └── event_loop (50 ms tick)
              ├── debt_rx.try_recv()
              │     ├── Ok(report) → editor.debt_report = Some(report)
              │     │                 spawn(narrative::generate()) ──► debt_narrative_rx
              │     └── Empty      → skip
              └── debt_narrative_rx.try_recv()
                    ├── Ok(text)  → editor.debt_narrative = text
                    └── Empty     → skip
```

`debt::compute()` itself runs three phases in sequence (all sync I/O except the
git subprocess in `cognitive::analyse()`):

1. **Cache check** — mtime fingerprint of `docs/adr/` and `src/`; if unchanged and
   < 1 hour old, return cached `DebtReport` in < 5 ms.
2. **Static analysis** — `intent::analyse()` (ADR file scan) and
   `technical::analyse()` (source file scan). No subprocesses. Typical latency:
   100–250 ms on a cold run of this project.
3. **Cognitive analysis** — `cognitive::analyse()` runs one async git subprocess
   and reads `sessions.jsonl`. Typical latency: 200–500 ms depending on git history
   depth.

The narrative is a separate, optional phase spawned only after the `DebtReport`
lands in the editor. It calls Ollama's `/v1/chat/completions` endpoint with a
structured prompt (≤250 token response, 15 s timeout). The narrative is cached
separately with a 24-hour TTL in `debt_narrative.txt`. If Ollama is unreachable or
times out, the dashboard shows without a narrative — silently, no loading state.

### Cache

**Location**: `~/.local/share/forgiven/debt_cache.json` (XDG_DATA_HOME-aware).

**Invalidation key**: sum of modification-time seconds for all files under
`docs/adr/` plus all files under `src/`. If any file is created, deleted, or
modified, the sum changes and the cache is bypassed. Max age is 1 hour regardless.
This gives near-instant second-launch latency with reliable freshness.

### Rendering

The welcome screen (`src/ui/buffer_view.rs:render_welcome`) is split vertically
only when a `DebtReport` is available and the terminal has enough height
(logo height + dashboard height + 2 lines minimum). The debt section never
appears half-rendered: it shows only when the full report is ready.

The three-column layout uses `Layout::horizontal([Constraint::Ratio(1,3); 3])`.
Each column is a borderless `Paragraph` — no borders, no boxes. The visual language
deliberately understates the dashboard: it should inform, not alarm. Warning-level
items (low implementation rate, critical functions, low active surface) use Yellow;
normal items use DarkGray; headline numbers use White. The Ollama narrative, when
present, wraps across the full width beneath the three columns using a local
`word_wrap()` helper (no additional crate dependency).

**Meaningful-or-stop gates**: each column degrades to a ✓ confirmation line when
its signals are absent or healthy, rather than showing zeros. "✓ all decisions
current" and "✓ no tool error spikes" are honest signals, not placeholders.

---

## Rationale

**Why brace-based cognitive complexity and not tree-sitter?**  
The brace scorer produces the same *ranking* as a full tree-sitter walk for the
dominant case (Rust). The worst-case divergence occurs with multi-line condition
expressions or macro-generated code — neither of which changes the identity of the
top-3 worst functions. Adding a full per-file tree-sitter pass for every `.rs` file
at startup adds latency without changing the actionable output.

**Why filesystem mtime for ADR staleness, not git log?**  
Filesystem mtime is synchronous (no subprocess), available immediately without
network or process latency, and accurate for a project where ADRs are modified only
inside the repository (not through filesystem copies). The git-log approach would
require a subprocess per file or a single expensive `git log --all --follow` pass;
it is not worth the latency given that mtime is equally accurate here. Cognitive
staleness uses `git log` because filesystem mtime is unreliable for source files
(a `git pull` or `touch` would reset it); ADRs are authored by the developer
directly, so their mtime is trustworthy.

**Why a separate `src/debt/` module rather than extending `src/insights/`?**  
`src/insights/` measures *developer session behaviour* — what happened during
coding sessions, averaged over time. `src/debt/` measures *static codebase state*
— what the code looks like right now. These are orthogonal concerns. Merging them
would create the same kind of god-module that ADR 0144 is working to undo in
`Editor`. The two modules can read the same data (sessions.jsonl) without owning
the same concern.

**Why is the narrative Ollama-only and not the active provider?**  
The narrative is a qualitative synthesis task on pre-computed structured data, not
a coding task. It runs unconditionally at launch without user intent, so it must be
local and free. Cloud providers incur per-token cost; Ollama does not. If the user
has no Ollama configured, the narrative is silently absent. This is preferable to
charging the user for an unrequested API call.

**Why 30 days for cognitive activity window?**  
Thirty days corresponds to a typical sprint cycle and captures "have I worked in
this area recently?". Weekly would miss long but active features; quarterly would
flag areas that were heavily active last month. The threshold is intentionally
hardcoded in v1: it should be validated against real usage before becoming a config
value, and premature configurability is its own form of debt.

---

## Consequences

**Positive:**
- Every cold launch now surfaces the three most actionable dimensions of codebase
  health without requiring the developer to remember to check.
- The intent debt metric creates a lightweight accountability loop around the ADR
  practice: if the implementation rate drops, it is visible on the first screen.
- Re-entry risk naming (specific function, specific file, cognitive score) makes
  "I haven't been in that code for a while" concrete rather than vague.
- Cache ensures the feature has zero user-perceived latency after the first run.
- All signals degrade gracefully: no git → no cognitive, no Ollama → no narrative,
  no sessions.jsonl → no error hotspots. The dashboard always shows what it can.

**Negative / constraints:**
- Four new fields on `Editor` (`debt_rx`, `debt_report`, `debt_narrative_rx`,
  `debt_narrative`) — adding to the struct bloat that ADR 0144 is working to reduce.
  These are candidates for the Phase 5 `RequestDispatcher` consolidation.
- The brace-based cognitive scorer has false positives for macro-heavy code (macros
  expand to many `{`/`}` braces) and false negatives for very long single-expression
  functions. For this codebase the error rate is acceptable; for heavily macro-driven
  codebases it would require the tree-sitter scorer.
- The 30-day activity window uses `git log`; in a detached-HEAD or no-git context,
  cognitive signals are unavailable. The dashboard degrades cleanly but the cognitive
  column is effectively absent.
- Narrative quality depends on the Ollama model configured. A small model (< 7B)
  may produce generic or low-quality commentary. The feature is useful even without
  the narrative; the narrative is a bonus, not a load-bearing signal.

---

## Related ADRs

- **ADR 0092** — Session metrics JSONL (`sessions.jsonl`) — source of tool-error
  hotspot data
- **ADR 0129** — Insights dashboard (Phase 1–4) — parallel session-analytics
  module; `src/insights/` vs `src/debt/` design decision documented above
- **ADR 0138** — Render decomposition — established the pattern of per-frame
  computed data threaded through `RenderContext`
- **ADR 0144** — Editor decomposition — context for the four new `Editor` fields
  and their eventual migration into `RequestDispatcher`
