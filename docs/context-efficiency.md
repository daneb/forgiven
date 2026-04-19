# SPEC: Context Efficiency Improvements for Forgiven

**Status:** Draft for review
**Target:** Claude Code implementation
**Goal:** Reduce tokens per agent turn without loss in answer quality, measured against a corpus.

---

## Background

Current Forgiven agent loop (as of v0.8.9-alpha.2, ADRs 0077, 0081, 0082, 0084, 0087, 0088) has three context-bloat sources that remain under-addressed:

1. Tool results are returned in full, even when the agent reads only the first few lines.
2. `read_file` is the default retrieval tool; `get_symbol_context` exists but agent preference is not enforced or measured.
3. The system prompt is prose, not structured, and is re-sent on every round.

This spec proposes three interventions, each measurable against a corpus defined in `forgiven-bench/`.

## Glossary

- **Answer quality**: recall-dominant token F1 between agent output and a golden reference (same metric autoresearcher uses).
- **Tokens per task**: cumulative prompt + completion tokens across all rounds until the agent emits a final answer.
- **Efficiency**: answer quality / tokens per task. Higher is better.

---

## Intervention 1 — Expand-on-demand tool results

### Problem

When the agent calls `read_file` or `search_files`, the entire result is inserted into history. A 400-line file returned for one referenced symbol costs 400 lines of tokens every subsequent round (until history is truncated by ADR-0077/0081).

### Proposed change

Tool results longer than a threshold (`expand_threshold_chars`, default 800) are truncated on insertion into history. Only the first N chars plus a synthetic tool-call invitation are retained.

Example replacement:

```
<tool_result id="r_7aeb">
First 800 chars of result...
[truncated; 4,217 chars total. Call expand_result(id="r_7aeb") to see full.]
</tool_result>
```

A new tool `expand_result(id: string, range?: {start: int, end: int})` is added. When invoked, the agent receives the full content (or the requested byte range). The expanded content is NOT written back into history; it is a one-shot expansion for the current round only.

### Acceptance criteria

- `expand_threshold_chars` is configurable in `config.toml` under `[agent]`, default 800.
- The full result is retained in an in-memory cache keyed by `id`, pruned when history is truncated.
- `expand_result` tool definition is registered and appears in the LLM's tool list.
- A new log line `[ctx] truncated tool_result r_xxx from Nchars to 800chars` is emitted at `info!` level.

### Out of scope

- Persisting cached results across editor restarts.
- Semantic truncation (first-N-chars is adequate for v1).

### Measurement

Track on a corpus of 20 real tasks (TBD): mean tokens per task before/after, mean answer quality before/after. Accept if tokens drop >=20% with answer quality drop <=3 percentage points.

---

## Intervention 2 — Retrieval policy preference: symbol > file

### Problem

`get_symbol_context` and `get_file_outline` exist (ADR-0082) but there is no mechanism pushing the agent to prefer them. The agent defaults to `read_file` because it is the simplest tool.

### Proposed change

Three coordinated changes:

1. **System prompt reordering.** `get_file_outline`, `get_symbol_context`, and `search_files` are listed *before* `read_file` in the tool section, with one-line guidance: "Prefer symbol-level retrieval. Use read_file only when you need more than three symbols from the same file."

2. **Tool description tightening.** `read_file`'s description is updated to include: "Expensive (full file in context). Prefer get_symbol_context for targeted lookups."

3. **Soft budget guard.** If the agent calls `read_file` three or more times in a single round on files larger than 300 lines, a one-time hint is injected on the next round: `[hint] You have read 3 large files this round. Consider get_file_outline first to locate specific symbols.`

### Acceptance criteria

- System prompt order is deterministic and verifiable with a unit test.
- Soft budget guard fires exactly once per session (not per round) to avoid spam.
- A new diagnostics line in `SPC d` shows `reads this session: N read_file / M get_symbol_context / K get_file_outline`.

### Out of scope

- Blocking `read_file` outright.
- LSP-based retrieval tuning (separate effort).

### Measurement

On the same corpus: measure ratio of `get_symbol_context` to `read_file` calls. Accept if the ratio shifts from current baseline (measure first) to >=1.5x without answer quality drop.

---

## Intervention 3 — Structural system prompt

### Problem

The current system prompt (`src/agent/mod.rs::build_system_prompt`) is prose with prose explanations. Measured informally, it is in the 2-4KB range. On every round, the full prompt is re-sent.

### Proposed change

Rewrite the system prompt in three sections:

1. **Role (<=200 chars).** One sentence: "You are a coding assistant operating inside the Forgiven IDE on project ${PROJECT_NAME}."
2. **Tools (JSON schema).** Use the existing tool schema JSON, with a one-line natural-language description per tool. No verbose explanations. Total target: <=1.5KB.
3. **Conventions (<=500 chars).** Bullet list: "Prefer symbol tools. Truncate large reads. Use edit_file for changes."

The rewrite must preserve all currently enforced behaviours - the test suite for ADR-0011 (agentic tool-calling loop) must pass unchanged.

### Acceptance criteria

- `build_system_prompt` output is <=2.5KB on a typical project.
- All agent integration tests pass.
- Diff of prompt is reviewable - the PR includes a before/after byte count.

### Out of scope

- Dynamic per-round system prompt variation (future ADR).
- Removing the open-file injection (ADR-0092 handles this separately).

### Measurement

Measure prompt size in bytes before/after. Measure total tokens per task on the corpus. Accept if token drop >=15% with answer quality drop <=2 percentage points.

---

## Corpus (prerequisite for all three)

Before implementation, build `forgiven-bench/`:

- 20 representative tasks from real Forgiven usage: 6 spec-like ("add tool X"), 8 debug-like ("why does Y fail"), 6 refactor-like ("extract Z").
- Each task: `task.md` (the user prompt), `golden.md` (the correct answer or diff), `setup.sh` (checkout to a known commit).
- Golden answers written manually. Use the same corpus format as autoresearcher (full/task/golden triplets).
- Include evaluator that runs a task end-to-end against the real Forgiven agent loop against a frozen Copilot model and records: total rounds, total tokens, answer quality (recall F1 vs golden).

This corpus is the single acceptance gate for all three interventions. Without it, every "improvement" is an intuition.

---

## Implementation order

1. Build corpus (1-2 days; this is the scientific instrument).
2. Establish baseline: run current Forgiven against corpus, record tokens/quality per task. This is the number to beat.
3. Intervention 3 (structural prompt) - lowest risk, quickest measurable win.
4. Intervention 2 (retrieval policy) - medium risk, depends on corpus signal.
5. Intervention 1 (expand-on-demand) - highest risk (adds new tool, touches history/cache), do last.

## Non-goals

- Re-architecting the agent loop.
- Changing the tool schema beyond adding `expand_result`.
- Building a training pipeline or fine-tuning anything.
