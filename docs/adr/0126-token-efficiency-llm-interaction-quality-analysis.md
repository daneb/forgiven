# ADR 0126 — Token Efficiency and LLM Interaction Quality Analysis

**Date:** 2026-04-14  
**Last updated:** 2026-04-14  
**Status:** Accepted — roadmap fully implemented (see Implementation Status below)

---

## Context

By April 14, 2026, Forgiven had consumed approximately 70% of the monthly GitHub
Copilot Enterprise token quota. A forensic review of `sessions.jsonl` and the
`panel.rs` context assembly path was conducted to understand the drivers and
define an optimisation roadmap. This ADR records the findings, competitive
analysis, and research synthesis. The phased action plan lives in
`docs/context-optimization-roadmap.md`.

---

## Evidence: sessions.jsonl Top-20 Analysis

The top-20 most expensive single API calls from `sessions.jsonl`, sorted by
`prompt_tokens` descending:

| Model        | Max prompt tokens | % of window | Session total (worst) |
|--------------|------------------:|------------:|----------------------:|
| claude-sonnet-4.5 | 65,679 t | 32% of 200K | 905,511 t |
| gpt-5.2      | 60,544 t          | 15% of 400K | 921,814 t |
| gpt-5.2      | 40,271 t          | 10% of 400K | 420,229 t |
| gpt-4.1      | 35,654 t          | 27% of 128K | 736,526 t |
| gpt-4.1      | 35,539 t          | 27% of 128K | 807,507 t |

The session-total column is the diagnostic signal. Sessions accumulating
**800k–900k tokens** in a single Forgiven session mean: repeated agentic loops
without running the janitor, history growing unchecked until the 90% warning
fires (or the user manually intervenes).

**Key insight:** at 35k tokens per round on gpt-4.1 (128K window), a 10-round
`/speckit.implement` session consumes **350k tokens** before the session ends.
History is the dominant driver — at 27% window utilisation per round, history
alone accounts for ~27k of those 35k tokens by round 8–10.

---

## Deep Dive: Token Composition Per Round

Reading `submit()` in `panel.rs` reveals the exact cost breakdown for a typical
Forgiven session:

### System Prompt (static per session)

| Component | Tokens (estimated) | Notes |
|-----------|-------------------:|-------|
| Preamble ("You are an agentic coding assistant…") | ~20 t | Fixed |
| `project_tree` (depth-2, ~50+ source files) | 300–500 t | Refreshed every 30s |
| `tool_rules` (MANDATORY PROTOCOL block) | ~500 t | Fixed every call |
| `context_snippet` (open file, up to 150 lines) | 0–600 t | Changes with active buffer |
| **System total (no open file)** | **820–1,020 t** | |
| **System total (large file open)** | **up to 29,750 t** | ADR 0087 finding |

After ADR 0093 (cap open-file to 150 lines), the `context_snippet` ceiling
dropped to ~600t. For a typical Forgiven session (working on `panel.rs`
~1,000 lines), the system prompt is roughly **1,200–1,400 tokens per round**.

### History (grows per round)

This is the primary driver. The growth curve for a typical `/speckit.implement`
session using gpt-4.1 (128K window):

| Round | Prompt tokens (est.) | Session total (cumulative) |
|-------|---------------------:|---------------------------:|
| 1     | 3,000 t              | 3,000 t                    |
| 2     | 6,500 t              | 9,500 t                    |
| 3     | 10,200 t             | 19,700 t                   |
| 5     | 17,000 t             | 56,200 t                   |
| 8     | 26,500 t             | 131,000 t                  |
| 10    | 33,000 t             | 196,000 t                  |

History growth is super-linear because each round re-sends all prior rounds'
messages after truncation. Tool results — especially large `read_file` responses,
`search_files` outputs, and the agent's verbose summaries — compound rapidly.

### Observation Masking (ADR 0123) — Current State

The `observation_mask_threshold_chars` parameter is implemented and wired through
`submit()`. When set to a non-zero value, older non-recent assistant messages
exceeding the threshold are replaced with a stub. This is the **highest-leverage
lever currently available** for reducing history re-send cost.

From the sessions.jsonl data, it is unclear whether this parameter is set to a
meaningful value in the current config. If it is 0 (disabled), enabling it at
2,000 chars would be the single highest-ROI change.

### Prompt Caching

The Copilot gateway supports prompt caching (ADR 0078). The `cached_tokens` field
in `StreamEvent::Usage` measures cache hits. For caching to be effective, the
system prompt prefix must be stable across rounds. The `context_snippet` (open
file) changes the system prompt structure on every buffer switch, invalidating
the cached prefix. With the current architecture, caching is only effective
within a session when the same file stays open.

---

## LLM Interaction Quality Assessment

### What is working well

**Tool discipline.** The `tool_rules` block is effective. The model follows
`get_file_outline` → `get_symbol_context` → `edit_file` as the preferred
pattern, reducing redundant `read_file` calls. The `search_files` tool avoids
full-file reads for pattern lookups. These save tokens per round.

**specKit auto-clear (ADR 0097).** Each phase starts with a clean context window,
preventing cross-phase accumulation. This is architecturally sound and saves
significant tokens across a full specKit workflow.

**SpecSlicer (ADR 0100).** Injects ~600–1,300 tokens of targeted spec context
instead of the model reading full `TASKS.md` + `SPEC.md` (~5,000–8,000 tokens)
via `read_file`. This is the right pattern.

**Importance-scored history (ADR 0081).** Error and panic messages are preserved
longer; large low-value tool results are deprioritised. Correct priority ordering.

### What is degrading quality

**History noise: large tool results re-sent verbatim.** `read_file` responses
on medium files (500–800 lines) produce 2,000–5,000 token assistant messages.
These are sent in full on every subsequent round that doesn't truncate them.
Observation masking addresses this directly.

**Verbose agent summaries.** The model's end-of-round summaries often exceed
500–800 tokens. These are correct content but their verbosity compounds across
rounds. The brevity rules in `tool_rules` help but don't fully contain this.

**`gpt-5.2` sessions.** The 400K window model produces lower `%` utilisation
per call (10–15%) but costs significantly more per token than gpt-4.1. Sessions
accumulating to 921k session-total on gpt-5.2 represent the most expensive
individual sessions in the log. Task/model routing — using gpt-4.1 for routine
implement rounds and gpt-5.2 only for complex reasoning tasks — would reduce cost.

**No on-demand retrieval for the project tree.** The depth-2 project tree is
sent every single round, including rounds where the model is doing file edits
in a single well-known file. At 300–500 tokens, this is low absolute cost but
represents wasted context that could go to history.

---

## Competitive Analysis: How Other IDEs Manage Context

### Cursor — RAG-based retrieval

Cursor scans the opened folder and computes a Merkle tree of hashes of all valid files, then builds semantic embeddings for code chunks. At query time, the embedding of the user's request is compared to the vector index and the top-k relevant chunks are injected into context. The effective context window available for the user's code is consistently less than half the advertised window — Cursor consumes tokens for its system prompt, codebase index results, conversation history, and automatically included file contents.

**What Cursor does that Forgiven doesn't:** the model never receives the full
project tree. It receives only the semantically relevant k chunks for the
current query. This is the fundamental difference. Cursor's per-query context
is surgically assembled; Forgiven's is broadly assembled.

**The trade-off:** Claude Code uses file tools for code discovery — Read, Grep, Glob, and Bash — with no pre-built semantic index. A well-indexed project returns the relevant file in under a second with minimal context cost; Claude Code may spend 30 seconds reading through directories and consuming 15,000 tokens in the process. Forgiven is closer to Claude Code's model (tool-based retrieval on demand) than Cursor's model (pre-indexed retrieval). This is appropriate for a local terminal IDE with privacy constraints.

### Aider — Tree-sitter structural map

Aider uses tree-sitter to parse the codebase into a structural map of functions, classes, and imports. Only the map goes into context. When the model needs to edit a specific file, Aider sends that file. This keeps baseline context small but requires accurate map-to-file retrieval.

Forgiven already has `get_file_outline` (ADR 0082) which implements the same
pattern at the symbol level. The gap is that Forgiven doesn't inject an
*aggregate* project-level structural summary at the start of sessions — only
the depth-2 file tree. A structural summary (top-level types and functions
per file, not just filenames) would be more useful at lower token cost.

### Windsurf Cascade — cross-session memory layer

Windsurf's Cascade system uses a memory layer that persists across conversations, storing project context, prior decisions, and file relationships outside the model's context window. This reduces per-session context pressure but adds latency for memory retrieval and can introduce stale information if the codebase changes between sessions.

Forgiven has the MCP memory server (ADR 0083) available. Using it to persist
architecture decisions, known patterns, and key file locations across sessions
would reduce the system prompt's need to re-establish context on every session.
The MCP memory server already exposes `add_observations` and `search_nodes`.

### GitHub Copilot — tight retrieval, small context budget

GitHub Copilot uses a proprietary retrieval system that sends relevant code snippets to the model. The context window is relatively small but the retrieval is tightly optimised for fast completions. Copilot's inline completions model keeps per-call context minimal by design; agent mode expands context but uses workspace-level implicit context from open tabs, not the full project tree.

### Research findings — effective context window limits

Model performance degrades when prompts rely on large context, and models exhibit increased hallucination rates as token counts rise. Most models had severe degradation in accuracy by 1,000 tokens in context; all models fell far short of their Maximum Context Window by as much as >99%.

This is the critical research finding for Forgiven. Sending 35k token prompts
to a 128K window model does not produce 35k worth of quality reasoning. The
model's effective attention degrades. Smaller, focused prompts consistently
outperform large, broad ones on coding tasks. A 500-token optimised prompt often extracts better answers than a 5,000-token verbose prompt. Caching reduces token processing costs by 90%+ on repeated requests.

---

## Root Cause Summary

The token budget problem has four distinct drivers in priority order:

| Driver | Estimated monthly impact | Status |
|--------|------------------------:|--------|
| History re-send (large tool results) | ~40% of budget | ✅ Observation masking active (default 2 000 chars, `panel.rs`) |
| Long agentic sessions without janitor | ~30% of budget | ✅ 100k session-total warning + 90% window warning + manual `SPC a j` |
| gpt-5.2 selection for routine tasks | ~15% of budget | ✅ `suggest_model_for_task()` hint in input area (`models.rs`, `agent_panel.rs`) |
| Static project tree every round | ~5% of budget | ✅ Structural map on round 0, one-line stub on subsequent rounds (`panel.rs`) |

---

## Implementation Status

All roadmap phases have been implemented as of 2026-04-14.

### Phase 1 — Quick wins
| Item | Status |
|------|--------|
| Obs mask threshold visible in `SPC d` | ✅ `ui/popups.rs` Context Breakdown section |
| `max_agent_rounds` default lowered to 10 | ✅ `config/mod.rs:default_max_agent_rounds()` |
| Project tree suppressed after round 1 | ✅ `panel.rs` — structural map on round 0, stub thereafter |
| 100k session-total warning | ✅ `panel.rs` — `session_total_100k_warned` flag |

### Phase 2 — Structural improvements
| Item | Status |
|------|--------|
| Structural project map (Aider pattern) | ✅ `build_structural_map()` at `panel.rs:81` — symbol names per `src/` file |
| Prompt caching alignment | ✅ Anthropic: stable prefix cached, `context_snippet` split as volatile (`panel.rs:1033`) |
| Task-complexity model router | ✅ `suggest_model_for_task()` in `models.rs`, rendered as dim hint in `agent_panel.rs` |

### Phase 3 — On-demand retrieval
| Item | Status |
|------|--------|
| Conditional `context_snippet` for specKit | ✅ `panel.rs` — `spec_cmd_ctx` gates injection; specKit commands get `None` |
| MEMORY RULES imperative first-call instruction | ✅ `panel.rs` — `search_nodes` call required before first response |
| Investigation subagent (`SPC a v`) | ✅ `start_investigation_agent()` in `panel.rs`; single-round tool-enabled call; result injected as `Role::System` |

### Phase 4 — Evaluation and adaptive behaviour
| Item | Status |
|------|--------|
| Structure-aware janitor compression | ✅ `context.rs` — fixed-section prompt: Files changed, Key decisions, Open questions, Next step, Context notes |
| Session-end efficiency record | ✅ `session.rs:append_session_end_record()` — writes `"type":"session_end"` to `sessions.jsonl` at `new_conversation()` and janitor completion |
| Adaptive round-limit hint | ✅ `session.rs:suggest_max_rounds()` — median of last 200 matching records + 2; rendered as dim hint before first submit |

---

## What This ADR Does Not Do

This ADR records the analysis and implementation status. The roadmap doc
`docs/context-optimization-roadmap.md` contains the full phase-by-phase
rationale and expected savings estimates.

---

## Consequences

**Positive**
- Establishes a shared, evidence-based understanding of where tokens are going.
- Frames the problem in terms of root causes rather than symptoms.
- Grounds future optimisation decisions in competitive context and research.
- Identifies observation masking activation as the highest-ROI immediate action.

**Negative / trade-offs**
- This analysis is based on `sessions.jsonl` through April 14, 2026 and the
  current codebase. The findings will need to be revisited if significant
  architectural changes alter the token composition.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0081](0081-importance-scored-history.md) | Importance-scored history truncation — primary history management mechanism |
| [0082](0082-symbol-aware-context-tools.md) | `get_file_outline` / `get_symbol_context` — Aider-pattern retrieval |
| [0083](0083-mcp-memory-server.md) | MCP memory server — cross-session persistence |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Original context bloat audit |
| [0092](0092-persistent-session-metrics-jsonl.md) | sessions.jsonl — evidence source for this analysis |
| [0099](0099-context-breakdown-token-awareness.md) | Per-segment token breakdown visibility |
| [0100](0100-spec-slicer-virtual-context.md) | SpecSlicer — surgical context injection pattern |
| [0123](0123-context-management-v2-observation-masking-and-disk-persistence.md) | Observation masking — highest-leverage current lever |
