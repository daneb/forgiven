# AI-IDE Architecture Exploration — Spec Index

**Status:** Drafts under review
**Context:** These four specs are the output of an exploration into how Forgiven should evolve as an AI-first IDE, grounded in 2025–2026 research on agentic coding, context engineering, and multi-agent architectures.

The four options were ranked by complexity, impact, size, and time:

| Rank | Spec | Complexity | Impact | Size | Time |
|---|---|---|---|---|---|
| 1 | [Intent Translator](intent-translator.md) | Low | Medium-High | ~400 LoC | 3–5 days |
| 2 | [Codified Context](codified-context.md) | Low–Medium | Medium-High | ~600 LoC | 1–2 weeks |
| 3 | [Artifact Layer](artifact-layer.md) | Medium | Medium (diminishing) | ~800 LoC | ~2 weeks |
| 4 | [Subagent Decomposition](subagent-decomposition.md) | High | High (uncertain) | ~2,500 LoC | 4–6 weeks |

---

## Why these four

Each spec is a self-contained architectural direction anchored in a specific research pattern:

- **Intent Translator** — Haseeb (2025) context-engineering pattern; pre-process user messages into structured task specs before the main agent runs.
- **Codified Context** — three-tier hot/warm/cold memory (Codified Context Infrastructure paper, March 2026; Chatlatanagulchai et al., November 2025).
- **Artifact Layer** — Google ADK "artifact + working context" separation; generalises ADR 0130's expand-on-demand pattern to all context sources.
- **Subagent Decomposition** — MASAI/HyperAgent multi-agent pattern; isolated contexts, summary-only handoff.

Crucially, these specs are **orthogonal**. They can be implemented independently. They also compose well:

- The **Intent Translator** feeds the **Subagent Supervisor** with structured intent.
- The **Codified Context** constitution gets injected into every specialist's system prompt.
- The **Artifact Layer** holds the outputs Navigator produces and Planner references.

---

## Recommended path

Start with **Option D (Intent Translator)**. It is the cheapest, most contained, and has the widest applicability. A week of work, no architectural commitment.

After that, the choice depends on what the `forgiven-bench/` corpus measurements reveal:

- If exploratory `read_file` loops dominate token cost → **Codified Context** (constitution + knowledge tier).
- If tool-result reuse is the dominant cost → **Artifact Layer** (generalise ADR 0130).
- If cross-phase context pollution is the dominant cost → **Subagent Decomposition**.

Without the corpus, picking 2–4 is guessing.

---

## Prerequisite: forgiven-bench/

Every spec has a measurement plan that depends on a corpus of 20 real tasks with golden answers. That corpus is described in `context-efficiency.md` and is a **prerequisite for validating any of the four specs**. Without it, any claimed improvement is an intuition.

This corpus is the scientific instrument. Build it before building the experiments.

---

## Relationship to existing ADRs

| Spec | Builds on | Supersedes |
|---|---|---|
| Intent Translator | 0057 (ask_user), 0116 (multi-provider LLM) | none |
| Codified Context | 0083 (MCP memory server) | formalises the informal CLAUDE.md convention |
| Artifact Layer | 0130 (expand-on-demand for tool results) | none (generalises 0130) |
| Subagent Decomposition | 0128 (Investigation subagent), 0011 (agentic loop) | 0011 behind a feature flag |

None of the four require rolling back any existing ADR. All four can live alongside the current architecture behind config flags.

---

## Non-goals (shared across all four)

- Training a custom LLM.
- Fine-tuning any model.
- Inventing a "new language" for LLMs (rejected in earlier exploration — see `context-efficiency.md` background).
- Replacing GitHub Copilot as the default backend.

---

## Open questions

1. Is the `forgiven-bench/` corpus format aligned with the existing `autoresearcher` corpus format? (Should be — same `_full.md` / `_task.md` / `_golden.md` triplet structure.)
2. Should the Intent Translator default to Copilot (no new API key needed) or Haiku (much cheaper per call)?
3. For the Subagent Decomposition, should the Tester have access to the full test suite or only tests that pattern-match the affected files?
4. Are there domain-specific specialists in Forgiven's users' projects that belong in the Codified Context Tier 2 catalogue (e.g. `.forgiven/agents/rust-style.md`, `python-data.md`)?

These should be resolved during spec review before implementation starts.
