# ADR 0115 — Agent Brevity Constraints

**Date:** 2026-04-05
**Status:** Accepted

---

## Context

The agent system prompt previously had one communication rule: work silently
during tool calls, then write a concise summary afterward. In practice, large
models (Copilot/Claude) still padded responses with filler — "Certainly!", "Of
course", "I'll now...", hedging qualifiers, and multi-paragraph summaries of
trivial changes. This verbosity has two costs:

1. **Token waste** — filler tokens count against context limits. With open-file
   injection and multi-turn sessions already pressing against limits (see ADR
   0092), every unnecessary sentence accelerates context exhaustion.
2. **Signal-to-noise degradation** — the agent panel is a narrow TUI widget;
   padded responses obscure the actual result.

Research supporting this change:

- *caveman* (internal) showed that explicit brevity instructions reduce Copilot
  response length by 40–60% with no accuracy loss.
- arXiv 2604.00025 showed 26 percentage-point accuracy improvement and 65–75%
  token savings when large models (Claude/GPT-4) are given explicit conciseness
  constraints. Effect is largest for models ≥ 70B; small models (≤ 7B) see
  negligible benefit and are not targeted here.

---

## Decision

Add a mandatory brevity rule (rule 7) to the COMMUNICATION RULES section of the
`tool_rules` system prompt, injected via `build_system_prompt()` in
`src/agent/panel.rs`.

Rule text:

> Be maximally concise in every response. No filler phrases, no hedging, no
> pleasantries. If the answer is one sentence, write one sentence. Never use
> 'Certainly!', 'Of course', 'I'll now...', or similar preamble. State only
> what changed and why — nothing else.

The rule is injected unconditionally for all providers. It is most impactful for
Copilot (large model). For Ollama/small models it is a no-op in practice but
harmless.

---

## Implementation

Single edit to `src/agent/panel.rs` — rule 7 appended to the COMMUNICATION
RULES block in the `format!()` that builds `tool_rules`. No new structs, no
config surface, no feature flag.

---

## Consequences

- **Positive:** Reduced token consumption per session; cleaner agent panel UX.
- **Positive:** Aligns system prompt with research-validated prompting practice
  for large models.
- **Neutral:** Small models are unaffected; the instruction is ignored when the
  model lacks instruction-following capacity.
- **Negative (none identified):** The rule does not suppress factual content —
  only preamble and padding. Accuracy is expected to improve or stay flat per
  the cited research.
