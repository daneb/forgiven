# Context Optimisation Roadmap

**Owner:** Dane Balia
**Last updated:** 2026-04-14
**Informs:** ADR 0126 — Token Efficiency and LLM Interaction Quality Analysis

---

## Problem Statement

Forgiven consumed ~70% of the monthly GitHub Copilot Enterprise token quota by
April 14, 2026. The root cause analysis (ADR 0126) identifies four drivers:

1. Large tool results re-sent verbatim in history (observation masking gap)
2. Long agentic sessions accumulating without compression
3. Premium model (gpt-5.2) used for routine tasks that don't require it
4. Static project tree sent every round regardless of relevance

The target: reduce monthly token consumption by 50–60% while maintaining or
improving agent task quality.

---

## Phase 0 — Immediate (no code changes required)

**Target: this week. Estimated savings: 15–20%.**

These are configuration and behaviour changes that require no Rust code.

### 0.1 Confirm observation masking is active

Open `~/.config/forgiven/config.toml` and verify:

```toml
[agent]
observation_mask_threshold_chars = 2000
```

If missing or set to 0, add it. This single setting stubs out older large
assistant messages (tool results, verbose summaries) when re-sent in history.
From the JetBrains NeurIPS 2025 research, observation masking matches or
outperforms rolling-summary compression at lower complexity cost.

**Expected impact:** Each round's history payload drops by 30–50% in sessions
with multiple large `read_file` or `search_files` calls.

### 0.2 Use the janitor earlier

Run `SPC a j` after completing a logical unit of work — not when the 90%
warning fires. A clean session before starting a new `/speckit.implement` task
costs one API call but saves 10–15 rounds of growing history.

Rule of thumb: run the janitor when `session_total` in `SPC d` exceeds 100k
tokens. Do not wait for the 90% warning.

### 0.3 Model selection discipline

Use gpt-4.1 as the default for all specKit implement/tasks phases. Reserve
gpt-5.2 for tasks explicitly requiring deep multi-file reasoning or complex
architectural decisions. The `Ctrl+T` model cycle is available for per-task
switching.

**Expected impact:** gpt-5.2 sessions in the top-20 were the most expensive.
Routing these to gpt-4.1 would reduce per-session cost by 60–70% on those
sessions (lower per-token rate on a cheaper model).

---

## Phase 1 — Quick wins (1–3 days of implementation)

**Target: end of April. Estimated additional savings: 15–20%.**

### 1.1 Activate observation masking config UI

**ADR reference:** ADR 0123 (implemented), ADR 0126 (analysis)

The config key exists. Verify it is exposed in the `SPC d` diagnostics overlay
so the current threshold value is visible per session. If not visible, add a
one-line display to the Context Breakdown section.

This is a diagnostic/confidence change, not a functional one.

### 1.2 Lower max_rounds default to 10

The current default is 20 rounds. Analysis shows most tasks complete in 5–8
rounds. A 20-round session accumulates 2× the history of a 10-round session.

Change `default_max_rounds()` in `src/config/mod.rs` from 20 to 10. Users who
need more rounds can extend per-task via the config or by approving continuation.

**Expected impact:** worst-case session history is halved. Per-session token
ceiling drops from ~350k to ~175k for a full implement session.

**Risk:** tasks that genuinely require more than 10 rounds will prompt the user
for continuation approval (ADR 0027 mechanism already exists). Low risk.

### 1.3 Suppress project tree on tool-only rounds

The `project_tree` (300–500 tokens) is sent on every round, including rounds
where the model is executing a single `edit_file` call in a well-known file.
It is only useful on the first round of a new conversation and on rounds where
the model might need to discover files.

**Change:** inject `project_tree` only on round 1 of a session (`session_rounds
== 0`). On subsequent rounds, omit it or replace it with a 1-line hint:
`[Project tree omitted — use list_directory if needed]`.

**Expected impact:** saves 300–500 tokens on every round except round 1. In a
10-round session, this saves 2,700–4,500 tokens in system prompt cost.

**Implementation:** in `submit()`, check `self.session_rounds` before including
`project_tree` in the system prompt string.

### 1.4 Add per-session janitor threshold warning at 100k

The current 90% warning fires at 90% of the model's context window — 115k
tokens for gpt-4.1. By then, most of the budget damage is done.

Add a second, softer warning at 100k *session total* tokens (regardless of
window size) that appears in the chat panel:

```
ℹ  Session total: 100k tokens. Consider running SPC a j before your next task.
```

This fires earlier and is based on cumulative cost, not per-round pressure.

**Implementation:** in `poll_stream()` Done handler, check
`total_session_prompt_tokens > 100_000` before the existing 90% check. One-time
per session (`context_near_limit_warned` gate can be split into two flags).

---

## Phase 2 — Structural improvements (1–2 weeks)

**Target: May 2026. Estimated additional savings: 10–15%.**

### 2.1 Structural project map (Aider pattern)

**ADR reference:** ADR 0082 (symbol-aware tools), ADR 0126 (analysis)

Replace the depth-2 file tree (filenames only) with a structural map:
top-level `pub struct`, `pub fn`, `pub enum`, and `impl` names per file,
generated via `get_file_outline` at session start and cached for the session.

This gives the model the same orientation as the file tree but with symbol-level
context. The model can answer "where does X live?" without a `read_file` round.
The structural map is larger (500–800 tokens vs 300–500t) but saves 1–2
`read_file` round-trips per session that currently cost 2,000–5,000 tokens each.

**Implementation:** new `build_structural_map(root: &Path) -> String` function
that calls `get_file_outline` logic across the top-level `src/` files. Cached
with the same 30s TTL as `project_tree`. Replaces `project_tree` in the system
prompt.

### 2.2 System prompt caching alignment

**ADR reference:** ADR 0078 (prompt caching)

Prompt caching is most effective when the system prompt prefix is stable across
rounds. The `context_snippet` (open file) currently sits at the *end* of the
system prompt, after the stable `tool_rules` block. This is already correct for
maximising cache hits on the stable prefix.

Verify the prompt structure in `submit()` is ordered: `[preamble] →
[project_tree] → [tool_rules] → [context_snippet]`. The first three components
are stable across rounds; only `context_snippet` changes on buffer switch.

If the order is different, reorder to put the volatile component last. This
maximises the cached prefix length and reduces billing cost per round.

**Expected impact:** 50–90% token cost reduction on rounds where the open file
hasn't changed, via prompt caching.

### 2.3 Task-complexity model router

Route API calls based on estimated task complexity:

| Task type | Model |
|-----------|-------|
| Single-file edit (`edit_file` only) | gpt-4.1 |
| Multi-file refactor, new feature | gpt-4.1 |
| Complex architectural reasoning, debugging subtle bugs | gpt-5.2 |
| specKit constitution / specify phases | gpt-4.1 |
| specKit implement (multi-round tool loop) | gpt-4.1 |

Implement as a `preferred_model_for_task(user_text: &str) -> Option<&str>`
heuristic that scans the user's input for complexity signals and returns a model
suggestion. Display the suggestion in the input area before submit.

Do not auto-switch models without user confirmation — model choice has quality
implications beyond cost.

---

## Phase 3 — On-demand retrieval (2–4 weeks)

**Target: June 2026. Estimated additional savings: 10–15%.**

### 3.1 Remove open-file auto-injection for non-editing sessions

The `context_snippet` is injected into the system prompt on every round,
regardless of whether the user's task involves the open file. For pure agent
tasks (`/speckit.implement`) the open file is irrelevant.

**Change:** only inject `context_snippet` when:
- The user explicitly attaches a file via the `@` file picker, OR
- The user's typed message references the open file by name or path, OR
- The active buffer was modified since the last round

Otherwise omit `context_snippet` from the system prompt entirely. The model
can always call `read_file` if it needs the buffer content.

This is the single change from ADR 0087's "root cause — not fixed here" section
that was deferred. It reduces system prompt from ~1,200–1,400t to ~820–1,020t
for most rounds.

**Risk:** the model loses passive awareness of the open file. For chat-style
sessions (asking questions about code) this is a regression. Mitigate by
keeping `context_snippet` for chat-mode sessions and removing it for
specKit-triggered sessions only.

### 3.2 Cross-session memory via MCP memory server

**ADR reference:** ADR 0083

Use the MCP memory server (`search_nodes`, `add_observations`) to persist
architecture decisions, known patterns, and key file locations across sessions.
This reduces the need for repeated discovery rounds.

At session start, the tool_rules block already instructs the model to call
`search_nodes` with query `'project context'`. Verify this is working by
checking Recent Logs in `SPC d` after the first round of a new session — the
log should show a `search_nodes` call.

If the memory server is not being used, add an explicit example to the
`MEMORY RULES` section of `tool_rules` that demonstrates the expected
`add_observations` pattern for recording ADR decisions.

### 3.3 Agentic subagent pattern for investigation tasks

When the user asks a broad question ("where is authentication handled?",
"explain this subsystem"), the main session bears the full context cost of the
model's exploration. A subagent pattern isolates investigation cost:

1. Spin up a single-round agentic call with `max_rounds = 1`
2. Ask the subagent to investigate and summarise, returning key file names and
   call paths
3. Inject the summary (100–300 tokens) into the main session

This is the Claude Code subagent pattern. Forgiven already has
`start_inline_assist()` as a precedent for single-round tool-disabled calls.
A `start_investigation_agent()` variant with tool-calling enabled but capped at
1 round would enable this pattern.

---

## Phase 4 — Evaluation and adaptive behaviour (1–2 months)

**Target: Q3 2026. Estimated savings: variable.**

### 4.1 Token efficiency score per session

Extend `sessions.jsonl` with a computed `efficiency_score`:

```
efficiency_score = task_completed (0/1) / session_prompt_total
```

This requires capturing task completion signal (janitor run, new_conversation,
or explicit user rating). Even a binary "did the session produce a file change?"
signal would be sufficient for ranking.

Use this to identify which interaction patterns produce the best
quality-per-token ratio and tune the phases above accordingly.

### 4.2 Adaptive round limit

Instead of a fixed `max_rounds = 10`, model the expected rounds for the current
task based on historical data from `sessions.jsonl`. If similar tasks (same
specKit phase, same approximate file count) completed in 6 rounds historically,
suggest `max_rounds = 8` as the initial limit with easy continuation.

This requires a simple similarity function over past session metadata — no
embedding or ML required. A heuristic based on task prefix and round count
from the last 10 similar sessions is sufficient.

### 4.3 Structure-aware compression for compressed history

When the janitor runs (`SPC a j`), the current compression prompt produces a
free-text summary. Replace this with a structure-aware compression that
preserves:
- File paths and symbols that were modified
- ADR decisions made during the session
- Open questions and the immediate next step

The golden corpus work (autoresearcher) identified that document-type-aware
compression significantly outperforms generic compression. Apply the same
principle to session history compression.

---

## Success Metrics

| Metric | Baseline (April 14) | Phase 0 target | Phase 1–2 target |
|--------|-------------------:|---------------:|----------------:|
| Monthly quota consumed by Apr 14 | 70% | 45% at same pace | 30% at same pace |
| Average tokens per specKit implement round | ~35k | ~28k | ~20k |
| Peak session total | 921k | 500k | 300k |
| % rounds hitting 90% window warning | unknown | < 5% | < 2% |

Track via weekly `sessions.jsonl` analysis using the same query from ADR 0126.

---

## What This Roadmap Explicitly Does Not Do

- **No RAG/vector indexing.** Cursor's approach requires a remote vector DB and
  cloud embedding infrastructure incompatible with Forgiven's local-first,
  privacy-first design. The tool-based retrieval model (`get_file_outline`,
  `get_symbol_context`, `search_files`) is the correct architecture for Forgiven.
- **No LLMLingua integration as default.** The autoresearcher experiment
  (April 2026) demonstrated that generic token-importance compression degrades
  quality for dense technical documents without document-structure awareness.
  LLMLingua remains available as an MCP sidecar (ADR 0088) for explicit use but
  is not in the default context pipeline.
- **No automatic model switching.** Model routing (Phase 2.3) provides
  suggestions only. The user retains control over model selection.

---

## References

- ADR 0126 — Token Efficiency and LLM Interaction Quality Analysis (this doc's
  evidence base)
- JetBrains "The Complexity Trap" (NeurIPS 2025 DL4Code): observation masking
  matches/beats LLM summarisation in 4 of 5 settings
- Paulsen (2026) "Context Is What You Need: The Maximum Effective Context Window":
  models degrade severely beyond ~1,000 tokens; effective window is <1% of
  advertised window for many tasks
- Cursor codebase indexing (Jan 2026): RAG retrieves only relevant k chunks;
  effective context < 50% of advertised window even with 1M models
- Morph "Cursor Context Window" (2026): Cognition measured agent success rates
  decrease after 35 minutes; doubling task duration quadruples failure rate
