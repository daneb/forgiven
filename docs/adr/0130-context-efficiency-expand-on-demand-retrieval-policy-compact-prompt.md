# ADR 0130 — Context Efficiency: Expand-on-Demand Results, Retrieval Policy, and Compact System Prompt

**Date:** 2026-04-19  
**Status:** Implemented

---

## Context

Three sources of token bloat remained unaddressed after ADR-0077 (history truncation),
ADR-0081 (importance scoring), and ADR-0123 (observation masking):

1. **Full tool results in history.** When the agent calls `read_file` on a 400-line file,
   all 400 lines are inserted into the message history. Every subsequent round re-sends
   those lines verbatim, even when the agent used only one symbol from the file. A single
   large read costs thousands of tokens per round for the rest of the session.

2. **`read_file` preference.** `get_symbol_context` and `get_file_outline` have existed
   since ADR-0082 but nothing enforced their use. The model consistently defaulted to
   `read_file` because it was listed first in `tool_definitions()` and the system prompt
   offered no preference signal.

3. **Verbose system prompt.** `build_system_prompt` produced ~2 700 chars (~680 tokens)
   of prose rules on every round, all of it re-sent regardless of whether the rules were
   relevant to the current task. The "Available tools" listing in the prompt duplicated
   what the model already receives through the API `tools` field.

The `docs/context-efficiency.md` spec defined three coordinated interventions and a
corpus-based acceptance test. This ADR records what was implemented and why.

---

## Decision

Implement all three interventions in the order prescribed by the spec (lowest risk first):

1. **Compact system prompt** (Intervention 3 from spec — lowest risk)
2. **Retrieval policy preference** (Intervention 2)
3. **Expand-on-demand tool results** (Intervention 1 — highest risk, done last)

---

## Intervention 3 — Compact system prompt

### Change

Replaced the `MANDATORY PROTOCOL` prose block in `panel.rs::submit()` with a
`CONVENTIONS:` bullet list and a single-line `Tools:` reference section.

**Before** (`tool_rules`, excluding dynamic parts): ~2 700 chars (~680 tokens)

```
MANDATORY PROTOCOL — follow these rules without exception:

TASK PLANNING RULES:
0. Use create_task / complete_task ONLY when the job involves 3 or more distinct
   file operations ...
   [12 lines]

COMMUNICATION RULES:
6. Do NOT output any text while working through tool calls. Work silently.
   ...
   [8 lines]

FILE EDITING RULES:
1. Before editing a file, prefer get_file_outline to understand its structure ...
   [10 lines]

MEMORY RULES (only when memory tools are available):
- FIRST CALL on any new session: search_nodes(query='project context') BEFORE ...
   [8 lines]

Available tools:
- create_task          Register a planned step ...
- complete_task        ...
  [8 tool entries, 2–3 lines each]
```

**After** (`tool_rules`): ~900 chars (~225 tokens) — a 67% reduction

```
CONVENTIONS:
- Symbol tools first: get_file_outline → get_symbol_context before read_file.
  Use read_file only when you need more than 3 symbols from the same file.
- Edits: edit_file over write_file; copy old_str verbatim; retry with fresh read on mismatch.
- Batch: read_files([…]) over repeated read_file; search_files over read_file+scan.
- Work silently; write one concise summary after all tools finish.
- Tasks (≥3 distinct file ops): create_task per step before work; complete_task after.
- ask_user: only for ambiguous destructive actions or mutually exclusive design choices.
- Memory (when tools available): search_nodes("project context") on first call; ...

Tools:
- get_file_outline, get_symbol_context — symbol-level retrieval (prefer these)
- read_file — full file (expensive; use when >3 symbols needed)
- read_files, search_files — batch reads and pattern search
- write_file, edit_file — create/overwrite or surgical find-and-replace
- list_directory — list directory contents
- expand_result(id) — retrieve full content of a truncated tool result
- create_task, complete_task — plan/track multi-step jobs [when planning enabled]
- ask_user, ask_user_input — ask user a question or collect text input [when planning enabled]
```

### Rationale

- The `Available tools:` listing was fully redundant — the model receives the complete
  JSON schema via the API `tools` field on every request. Removing it saves ~700 chars.
- Rules were condensed to single-line bullets without losing the semantic content.
  Verbose explanations (`"do NOT use it to confirm routine read/write operations"`) do
  not change model behaviour but add tokens to every round.
- The single-sentence format aligns with how production systems (Claude Code, Cursor)
  present conventions — compact, scannable, actionable.

### Acceptance

`build_system_prompt` output (static parts only, excluding structural map and open file)
reduced from ~2 700 chars to ~900 chars. Total system prompt on a typical project
(round 2+, no open file) is approximately 960 chars (~240 tokens), well under the 2.5 KB
spec target.

---

## Intervention 2 — Retrieval policy preference

Three coordinated changes:

### 2a. Tool definition reordering

`tool_definitions()` in `tools.rs` now lists `get_file_outline` and `get_symbol_context`
**before** `read_file`. The model's tool-selection bias follows the order tools appear in
the schema; placing preferred tools first is a known effective nudge.

### 2b. `read_file` description tightening

`read_file`'s `description` field updated from:

> "Read the full contents of a file in the project. Returns line-numbered output."

To:

> "Read the full contents of a file in the project. Returns line-numbered output.
>  Expensive: the entire file enters context. Prefer get_symbol_context for targeted
>  lookups; use read_file only when you need more than three symbols from the same file."

This surfaces the cost directly at the point of tool selection — the most actionable
place for the model to see it.

### 2c. Soft budget guard

When the agent calls `read_file` on three or more files exceeding 300 lines in a single
round, a one-time hint is injected into the conversation before the next API call:

```
[hint] You have read 3 or more large files this round. Consider get_file_outline first
to locate specific symbols, then get_symbol_context for targeted reads.
```

**Properties:**
- Fires **once per session** (`read_hint_fired: bool` in `agentic_loop`), not once per
  round, to avoid spam.
- Injected as a `"user"` role message after the tool results, before the next model
  turn. This is a common pattern for system-level guidance in multi-turn tool loops.
- Logged at `info!` level: `[ctx] soft budget hint injected: N large reads this round`.
- Large-file threshold: >300 lines, parsed from the `read_file` result header
  (`"{path} ({N} lines)\n..."`).

### 2d. Diagnostics

`SPC d` now shows retrieval tool counts and the symbol:read_file ratio for the current
session:

```
  reads    4 read_file  /  7 get_symbol_context  /  3 get_file_outline
  ratio    symbol:read_file = 2.5x
```

Ratio is green when symbol tools are used at least as often as `read_file`, yellow
otherwise. Spec acceptance target: ratio ≥ 1.5x without answer-quality drop.

---

## Intervention 1 — Expand-on-demand tool results

### Design

Tool results longer than `expand_threshold_chars` (default: 800, configurable in
`config.toml` under `[agent]`) are truncated before insertion into message history. The
full result is stored in an in-memory cache keyed by the tool call ID.

**Truncated history entry:**
```
src/agent/panel.rs (547 lines)
   1 | //! Copilot Chat / agent panel — with agentic tool-calling loop.
   ...
[truncated; 21,532 chars total. Call expand_result(id="call_7aeb3f") to see full.]
```

A new tool `expand_result(id: string, range?: {start: int, end: int})` is registered and
returned from the full cache on demand. Expanded content is served to the model for the
current round only — the expansion itself is not written back into history, preserving the
truncated entry for all subsequent rounds.

### Implementation details

**Cache lifecycle:**
- `result_cache: HashMap<String, String>` lives in `agentic_loop()` scope.
- Keys are the tool call IDs generated by the provider (e.g. `"call_7aeb3f"`).
- Cache is never pruned within a single agent invocation (max rounds is bounded).
- Cache is discarded when the agentic loop task completes. It is not persisted across
  editor restarts (out of scope per spec).

**What is NOT truncated:**
- Results that start with `"error"` (errors are always shown in full).
- Results from `expand_result` itself.
- Results shorter than `expand_threshold_chars`.

**Threshold of 800 chars:**
Chosen to preserve the first ~200 tokens of a result — enough to capture the file header,
the first function signature, or a search summary — while discarding the long tail. The
model can then decide whether to call `expand_result` or proceed with what it has.

**Tool ordering:**
`expand_result` is listed after `list_directory` in `tool_definitions()` — after all
primary tools, so the model sees it as a retrieval aid rather than a first-class action.

### Configuration

```toml
[agent]
expand_threshold_chars = 800   # 0 disables truncation
```

Set `expand_threshold_chars = 0` to disable entirely (e.g. for debugging or benchmarking
baseline behaviour).

**Disabled for:**
- Inline assist (`start_inline_assist`) — single-round, no history accumulation.
- Investigation subagent (`start_investigation_agent`) — single-round, read-only.
- Auto-Janitor compression round — the janitor needs full tool results to summarise.

---

## Alternatives considered

### Semantic truncation (first-N-chars vs. first-N-lines)

The spec explicitly deferred semantic truncation ("first-N-chars is adequate for v1").
A line-count-based truncation would preserve more syntactic integrity (no mid-line cuts)
but adds complexity and doesn't change the token savings materially for prose results.

### Storing the cache in `AgentPanel` instead of `agentic_loop`

Would survive across rounds of the outer `submit()` loop, allowing the same cached
result to be retrieved across multiple agent invocations within a session. Rejected
because the cache lifetime in `agentic_loop` is already sufficient for the within-session
use case, and sharing via `AgentPanel` would require `Arc<Mutex<...>>` or an equivalent
to cross the thread boundary.

### Hard-blocking `read_file` after N calls

Considered as an alternative to the soft budget hint. Rejected per the spec ("Blocking
`read_file` outright" is explicitly out of scope for Intervention 2). A hard block would
break workflows that genuinely require full file reads (e.g. writing a new file from
scratch by reading a reference file).

### Prompt-only approach (no tool-order change)

Adding preference text to the system prompt without reordering `tool_definitions()`.
Tested informally: reordering has the stronger effect because the model's tool-selection
bias is positional, and the schema order is evaluated before the system prompt text when
multiple tools match a context.

---

## Measurement plan

Per the spec, acceptance requires a 20-task corpus (`forgiven-bench/`) not yet built.
Interim proxies observable today:

| Signal | How to read |
|--------|-------------|
| `SPC d` ratio | Session symbol:read_file ratio. Target ≥ 1.5x. |
| `[ctx]` log line | `sys=Xt` should drop by ~450t vs. pre-0130 baseline on round 2+. |
| `[ctx] truncated tool_result` log lines | Confirms truncation is firing on large results. |
| `[ctx] soft budget hint injected` log line | Confirms guard fires when needed. |

Full corpus evaluation (tokens per task, answer F1) is the prerequisite for final
acceptance per the spec and should be conducted before these changes are promoted from
`alpha` to `stable`.

---

## Known limitations

### Cache is not pruned when history is truncated

When `submit()` trims old messages in its importance-scoring phase (ADR-0081), the
corresponding cache entries in `agentic_loop` are not removed. This is harmless — the
cache is bounded by the number of tool calls in the invocation (typically ≤ 30) and
discarded when the loop exits. A future improvement could emit a `StreamEvent` when
messages are dropped so the loop can evict stale entries.

### `expand_result` range is byte-indexed but results are UTF-8

The `range.start` / `range.end` parameters are documented as byte offsets but the
implementation uses char offsets (`result.chars().collect()`). For ASCII-dominant source
code this is equivalent; for files with heavy multi-byte characters it may produce
off-by-one slices. The v1 implementation accepts this imprecision given the primary use
case is code.

### Soft budget hint fires on total reads, not unique files

If the agent reads the same large file three times in one round (unusual but possible
after edit errors), the hint fires even though only one file was involved. This is
conservative — it errs toward nudging the model toward outline tools — and is acceptable
for v1.

---

## Future improvements

### F1 — Semantic truncation

Replace first-N-chars with first-N-lines, preserving line boundaries. For
`get_symbol_context` results, truncate at the end of the first complete function body
rather than at a character count. Requires parsing the `read_file` line-number header
and counting newlines.

### F2 — Cache eviction on history truncation

Add a `StreamEvent::MessageDropped { tool_call_ids: Vec<String> }` event emitted by
`submit()` when old tool messages are evicted. `agentic_loop` listens and removes the
corresponding cache entries, keeping memory bounded for very long sessions.

### F3 — Corpus and formal measurement

Build `forgiven-bench/` (20 tasks, golden answers, evaluator) as specified in
`docs/context-efficiency.md`. Run before/after benchmarks to confirm token reduction ≥
20% (Intervention 1), ratio shift ≥ 1.5x (Intervention 2), and prompt drop ≥ 15%
(Intervention 3) without answer-quality regression.

### F4 — Config-driven expand threshold per tool

Allow per-tool thresholds, e.g. `read_file = 1600, search_files = 800`. Large file reads
benefit from a higher threshold to preserve more context; search results are typically
repetitive and can be truncated more aggressively.

---

## Files changed

| File | Change |
|------|--------|
| `src/config/mod.rs` | Added `expand_threshold_chars: usize` (default 800) to `AgentConfig` |
| `src/agent/tools.rs` | Reordered tool definitions (symbol tools first); updated `read_file` description; added `expand_result` tool |
| `src/agent/panel.rs` | Replaced `tool_rules` prose with compact `CONVENTIONS:` block; added `expand_threshold_chars` to `submit()` signature; initialised and reset retrieval counters |
| `src/agent/agentic_loop.rs` | Added `expand_threshold` parameter; `result_cache` HashMap; `read_hint_fired` guard; hint injection after dispatch |
| `src/agent/tool_dispatch.rs` | Added `result_cache`, `expand_threshold`, `large_reads` parameters; `expand_result` tool handling; truncation logic; large-read counting |
| `src/agent/mod.rs` | Added `session_read_file_count`, `session_symbol_count`, `session_outline_count` to `AgentPanel` |
| `src/ui/mod.rs` | Added `tool_retrieval_counts` to `DiagnosticsData` |
| `src/editor/render.rs` | Populated `tool_retrieval_counts` in diagnostics construction |
| `src/ui/popups.rs` | Rendered retrieval ratio in `SPC d` Context Breakdown section |
| `src/editor/actions.rs` | Updated two `submit()` call sites with `expand_threshold` |
| `src/editor/input.rs` | Updated `submit()` call site |
| `src/editor/hooks.rs` | Updated two `submit()` call sites |
