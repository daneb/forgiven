# Context Window Management — Research & Options Analysis

**Date:** 2026-04-08  
**Author:** Research session (Claude Sonnet 4.6)  
**Status:** Reference document — informs ADR 0123

---

## Why This Was Written

forgiven's auto-janitor (ADR 0101, 0117, 0120, 0121) accumulated significant complexity
across multiple sessions. Before investing further, we researched what production tools
actually do and what the evidence says about rolling-summary compression.

---

## What Production Tools Do

| Tool | Approach | Auto? |
|---|---|---|
| **Claude Code** | LLM summarisation at ~95% of 200k window. Preserves system prompt across compaction. Docs warn early instructions may be lost. | Yes |
| **Cline** | LLM summarisation at ~80%. Previously just truncated from the middle. Active bug (issue #9748) where it misfires on misreported context window sizes. | Yes |
| **Codex CLI** | LLM summary + keeps last ~20k tokens verbatim. Official docs: *"Long conversations and multiple compactions can cause the model to be less accurate."* | Yes |
| **Aider** | **No auto-summarisation.** Manual `/drop` and `/clear`. Philosophy: user controls context. | No |
| **Cursor** | **No summarisation.** Embeddings-based codebase indexing; short threads (~20k limit) by design. | No |
| **Amp (Sourcegraph)** | Explicitly against auto-compaction. Manual "handoff" and thread forking instead. | No |
| **Zed AI** | No summarisation. Start a new thread per task. | No |

**Observation:** Tools built for long agentic runs (Claude Code, Cline, Codex) adopted
summarisation because they routinely hit limits. Tools that stayed short-session (Cursor,
Aider, Zed, Amp) avoided it entirely and see it as a complexity/quality trade-off not
worth making.

---

## What Research Says

### JetBrains — "The Complexity Trap" (NeurIPS 2025 DL4Code workshop)
*Most directly relevant empirical result.*

Compared LLM summarisation vs. **observation masking** (removing large tool output
payloads but keeping the action/reasoning record) on coding agent benchmarks.

**Result: simple observation masking matched or slightly outperformed LLM summarisation
in 4 of 5 settings, at lower cost.**

Sources:
- [Blog post](https://blog.jetbrains.com/research/2025/12/efficient-context-management/)
- [GitHub repo](https://github.com/JetBrains-Research/the-complexity-trap)

### Active Context Compression — arXiv:2601.07190
LLMs compress their own context rarely and poorly without explicit scaffolding (Claude
Haiku compressed only 2× per task, achieving 6% savings). With explicit prompting every
10–15 tool calls: 22.7% savings. Takeaway: LLMs are poor self-managers of context without
heavy scaffolding.

### MemGPT — arXiv:2310.08560
Structured approach: separate working memory, archival memory, and recent context. LLM
manages memory via tool calls. Qualitatively different from rolling-summary compression —
agent-directed selective retention. Performance unaffected by increased context length.

### ACON — arXiv:2510.00615
Treats compression as an optimisation problem across environment observations and
interaction history. 26–54% peak token reduction on AppWorld, OfficeBench, and
Multi-objective QA, largely preserving task performance.

### "LLMs Get Lost In Multi-Turn Conversations" — arXiv:2505.06120
Average 39% performance degradation in multi-turn settings vs. single-turn, even before
hitting limits. Models achieving 90%+ on single-turn tasks struggle when context is
underspecified across turns. Known as the "Lost in Conversation" phenomenon.

### Context Rot Research (Morph)
Performance degradation accelerates beyond 30,000 tokens even in 200k-window models.
~65% of enterprise AI failures in 2025 attributed to context drift or memory loss — not
raw context exhaustion. Average conversation quality drops 12–15% per additional turn
beyond turn four.

Sources: [Context Rot | Morph](https://www.morphllm.com/context-rot),
[FlashCompact: Every Method Compared](https://www.morphllm.com/flashcompact)

---

## Known Failure Modes of Rolling-Summary Auto-Compression

These are documented by the tools themselves, not speculative:

1. **Cumulative drift across multiple compactions.** Every tool that uses this (Claude
   Code, Codex CLI, Cline) acknowledges that sequential compactions compound degradation.
   Codex CLI documents this explicitly.

2. **Early constraints and decisions vanish.** Claude Code's own docs: "detailed
   instructions from early in the conversation may be lost." In coding: architecture
   decisions, rejected approaches, non-obvious requirements.

3. **Incorrect early assumptions get baked in.** Early model errors get compressed into
   the summary as facts. Later corrections don't reliably override them.

4. **Code and tool output cannot survive faithful summarisation.** 500-line diffs, stack
   traces, and grep results cannot be summarised to prose without precision loss.

5. **The model loses its rhythm.** Pure prose summaries replacing all prior context cause
   measurable formatting and quality drift.

6. **Trigger timing is hard to calibrate.** Claude Code changed its threshold multiple
   times in 2025. Cline has an active bug from misreported window sizes.

7. **Compaction itself can fail.** If context is already near the limit, the compaction
   call can itself hit the limit, leaving the session broken.

---

## Options Evaluated

### A — Raise threshold to 95%, keep rolling summary
- **Complexity:** None (config default change)
- **Pro:** Fires far less often; most specKit phases never hit it
- **Con:** All quality problems remain when it does fire

### B — Observation masking (strip large tool payloads on re-send)
- **Complexity:** Low — modify history assembly in `send_messages()`, no async round
- **Pro:** JetBrains NeurIPS 2025 validated; no timing bugs; no state machine; precision
  loss is explicit and recoverable (model can re-call the tool)
- **Con:** Model loses verbatim prior tool results; must re-fetch for exact content

### C — Pre-compaction disk persistence + search tool
- **Complexity:** Low–medium — write JSONL to disk before compaction; expose search tool
- **Pro:** Compaction quality no longer matters — ground truth always accessible; forgiven
  already archives in memory and writes `sessions.jsonl` for metrics (80% of infrastructure
  exists); survives editor restarts
- **Con:** Adds a tool call and latency when model needs to look something up; model must
  know when to search

### D — Verbatim recent buffer + summarise older (LangChain hybrid)
- **Complexity:** Medium
- **Pro:** Model always has exact recent context; fixes the specKit question/answer timing
  issue more simply than ADR 0121's deferred-trigger state machine
- **Con:** Seam between summarised and verbatim portions causes coherence drift

### E — Manual only (`SPC a j`), disable auto-trigger
- **Complexity:** None (set `janitor_threshold_tokens = 0` as default)
- **Pro:** Zero timing bugs, zero quality risk, user decides
- **Con:** Users forget; bad UX in long specKit runs

### F — Tiered: B + C + A (the recommended combination)
- **Complexity:** Low–medium total
- **Layers:** Observation masking prevents most threshold crossings → disk persistence
  eliminates quality risk → rolling summary at 95% as last resort
- **Pro:** Each layer independently useful; B + C together likely make A fire rarely
- **Con:** Three moving parts

---

## Chosen Direction

**B + C + manual janitor (no auto-trigger).** See ADR 0123.

Rationale:
- forgiven's largest token cost drivers (open-file injection, large tool payloads) are
  better addressed by prevention (ADR 0092 fixed open-file) and masking (B) than by
  after-the-fact compression.
- specKit auto-clears between phases (ADR 0097), so most sessions are naturally short.
- The auto-janitor's complexity (ADR 0101/0117/0120/0121) and the bugs it accumulated are
  evidence that it is fighting the architecture rather than working with it.
- A 90% in-chat warning (implemented 2026-04-08) gives the user a clear signal with the
  exact keybind to act — bridging the gap between "no auto" and "completely invisible."

---

## Sources

| Source | URL |
|---|---|
| JetBrains "Complexity Trap" blog | https://blog.jetbrains.com/research/2025/12/efficient-context-management/ |
| JetBrains GitHub repo | https://github.com/JetBrains-Research/the-complexity-trap |
| Active Context Compression | https://arxiv.org/abs/2601.07190 |
| MemGPT | https://arxiv.org/abs/2310.08560 |
| ACON | https://arxiv.org/abs/2510.00615 |
| LLMs Get Lost In Multi-Turn | https://arxiv.org/html/2505.06120v1 |
| Context Rot — Morph | https://www.morphllm.com/context-rot |
| FlashCompact — Morph | https://www.morphllm.com/flashcompact |
| Claude Code Compaction docs | https://platform.claude.com/docs/en/build-with-claude/compaction |
| Claude Code Auto-Compact analysis | https://www.morphllm.com/claude-code-auto-compact |
| Context Compaction Research Gist | https://gist.github.com/badlogic/cd2ef65b0697c4dbe2d13fbecb0a0a5f |
| Cline Auto Compact docs | https://docs.cline.bot/features/auto-compact |
| Aider token limits docs | https://aider.chat/docs/troubleshooting/token-limits.html |
| LangChain ConversationSummaryBufferMemory | https://python.langchain.com/api_reference/langchain/memory/langchain.memory.summary_buffer.ConversationSummaryBufferMemory.html |
| Pinecone LangChain conversational memory | https://www.pinecone.io/learn/series/langchain/langchain-conversational-memory/ |
| Stop LLM Summarisation From Failing — Galileo | https://galileo.ai/blog/llm-summarization-production-guide |
