# SPEC: Full Subagent Decomposition (Option A)

**Status:** Draft — not yet reviewed
**Priority:** Rank 4 of 4 in the AI-IDE architecture exploration
**Estimated size:** ~2,500 LoC
**Estimated time:** 4–6 weeks focused
**Dependencies:** ADR 0128 (Investigation subagent) — the pattern; ADR 0057 (ask_user) — similar isolated context idea

---

## Goal

Replace Forgiven's single-agent architecture with a **supervisor + specialists** pattern. A lightweight supervisor receives the user's structured intent (ideally from the Intent Translator, Option D) and dispatches work to specialist subagents, each with a tightly scoped toolset and isolated context window. Specialist results return to the supervisor as summaries, never as full tool traces.

The design follows the empirically validated MASAI pattern (28.3% resolution on SWE-Bench Lite vs. single-agent baseline) and HyperAgent's Planner/Navigator/Editor/Executor decomposition.

This is the most ambitious option. It should be attempted **only after** a `forgiven-bench/` corpus exists and the simpler options (B, C, D) have been measured and exhausted.

---

## Problem


Today, Forgiven's agent loop is a single conversation that carries every tool call, every file read, every MCP response, every reasoning step in one accumulating history. Several known pathologies result:

1. **Context rot across phases.** A long session mixes exploration (round 1–3), planning (round 4), editing (round 5–8), testing (round 9–10), and review (round 11+). By round 10 the prompt contains information from rounds 1–9 that is mostly irrelevant to the current subtask. Research (Chroma, July 2025) documents systematic accuracy degradation as context accumulates.
2. **Tool pollution.** The model sees every available tool at every round, even when the current subtask needs only three of them.
3. **No parallel work.** A task that could run exploration and test planning simultaneously runs them serially.
4. **All-or-nothing truncation.** When ADR 0077 truncates history, it applies one policy across all message types. An exploration message from round 2 and a plan message from round 4 are treated identically.

ADR 0128 showed the subagent pattern works. Its investigation subagent is isolated, tightly scoped, read-only, and returns a 200-word summary. This spec extends that proven pattern to the full agent loop.

---

## Proposed design

### Agent topology

```
          ┌──────────────────┐
          │   User message   │
          └────────┬─────────┘
                   │
                   ▼
          ┌──────────────────┐
          │  Intent Translator │ (optional, Option D)
          └────────┬─────────┘
                   │ structured intent
                   ▼
          ┌──────────────────┐
          │   Supervisor     │ ← long-lived, orchestration only
          └────────┬─────────┘
                   │ task delegation
        ┌──────────┼──────────┬──────────┐
        ▼          ▼          ▼          ▼
   ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐
   │Navigator│ │Planner │ │Editor │ │Tester │
   │(read)  │ │(reason) │ │(write) │ │(exec)  │
   └────────┘ └────────┘ └────────┘ └────────┘
        │          │          │          │
        └──────────┴──────────┴──────────┘
                   │ summaries
                   ▼
          ┌──────────────────┐
          │   Supervisor     │ → integrates summaries, decides next step
          └──────────────────┘
```

### Specialist definitions

Each specialist is a `Subagent` with four properties:

1. **System prompt** — specialised to its role.
2. **Tool allowlist** — only the tools needed for the role.
3. **Context budget** — a hard cap on tokens per invocation.
4. **Output schema** — structured summary returned to supervisor, not full trace.

```rust
pub struct Subagent {
    pub name: &'static str,
    pub system_prompt: String,
    pub tools: Vec<ToolDef>,                     // allowlist, not full registry
    pub max_rounds: usize,
    pub max_tokens: usize,
    pub output_schema: SubagentOutputSchema,
}

pub enum SubagentOutputSchema {
    Navigator { files: Vec<PathBuf>, symbols: Vec<SymbolRef>, key_facts: Vec<String> },
    Planner { steps: Vec<PlanStep>, risks: Vec<String> },
    Editor { diffs: Vec<FileDiff>, rationale: String },
    Tester { pass: bool, failures: Vec<TestFailure>, coverage: Option<f32> },
}
```

#### Navigator (read-only exploration)

- **Tools:** `get_file_outline`, `get_symbol_context`, `search_files`, `read_file`, `list_directory`
- **System prompt:** "You are a code navigator. Explore the codebase to find relevant files and symbols for the given task. Do not propose changes. Return a structured summary."
- **Max rounds:** 3
- **Output:** list of relevant files + symbols + up to 5 key facts

#### Planner (reasoning, no IO)

- **Tools:** *(none — pure reasoning from supervisor context)*
- **System prompt:** "You are a task planner. Given the user's intent and the Navigator's findings, produce an ordered list of implementation steps. Identify risks."
- **Max rounds:** 1 (non-agentic, single call)
- **Output:** ordered steps + risks

#### Editor (writes)

- **Tools:** `edit_file`, `write_file`, `read_file` (for verification only)
- **System prompt:** "You are a code editor. Execute the given plan step. Return a diff. Do not plan."
- **Max rounds:** 2 per plan step
- **Output:** applied diffs + rationale

#### Tester (execution)

- **Tools:** `execute_command` (restricted to test commands), `read_file`
- **System prompt:** "You are a tester. Run the project's test suite. Report pass/fail and any failures."
- **Max rounds:** 1
- **Output:** pass/fail + failures + coverage

### Supervisor logic

The supervisor is a small, deterministic state machine in Rust — **not an LLM loop**. It dispatches specialists based on the task's declared scope from the Intent Translator:

```
on receive intent:
    if intent.scope is ProjectWide or Unknown:
        dispatch Navigator
        wait for summary
    if intent.expected_output in [Code, Diff]:
        dispatch Planner with Navigator's summary
        for each step in plan:
            dispatch Editor with step
            if step.needs_verification:
                dispatch Tester
                if Tester reports failure:
                    dispatch Editor with failure context to fix
    if intent.expected_output is Explanation:
        dispatch Navigator
        synthesise explanation from summary (single LLM call, no subagent)
```

The supervisor keeps a compact running context: just the Intent, the Navigator summary, and the Plan. Specialist full traces are never pulled up.

### Supervisor is a state machine, not an LLM

This is the key design decision. Making the supervisor an LLM itself was tempting, but:

- It reintroduces the context-rot problem at the supervisor level.
- It adds latency (an extra model call per decision).
- It makes debugging hard (decisions are opaque).

A Rust state machine is:

- Deterministic and debuggable.
- Zero-token per decision.
- Explicit about when/why specialists fire.

If more sophisticated orchestration is needed later, the state machine can be made configurable (e.g. via Lua scripts or a DSL). That is a separate spec.

### UX: visualising subagents

The agent panel gains a vertical strip showing active specialists:

```
┌─ Agent ──────────────────────────────────────┐
│ ┌──────────────────────────────────┐          │
│ │ Goal: Add error handling to X    │          │
│ │ Plan: 3 steps                    │          │
│ │  [✓] 1. Navigator — 4 files found │         │
│ │  [✓] 2. Planner   — plan ready    │         │
│ │  [●] 3. Editor    — step 2 of 3   │  ← live │
│ └──────────────────────────────────┘          │
│                                               │
│ 🤖 Editor: Applied diff to src/x.rs (...)     │
└───────────────────────────────────────────────┘
```

Each specialist's scrollback is accessible via `SPC a s` (subagent inspector) — a drill-down panel showing the full trace for a specific specialist without cluttering the main view.

### Message passing

Specialists communicate with the supervisor via in-process Rust channels (`tokio::sync::mpsc`). No JSON-over-stdio, no extra processes. This keeps the overhead minimal.

---

## Acceptance criteria

- [ ] `Subagent` trait and four specialist implementations (Navigator, Planner, Editor, Tester).
- [ ] Supervisor state machine dispatches specialists per the flow above.
- [ ] Intent Translator (Option D) integration — specialists receive structured intent, not raw message.
- [ ] Each specialist has its own isolated context; summaries return to supervisor; full traces never pollute supervisor context.
- [ ] Subagent inspector (`SPC a s`) shows per-specialist scrollback.
- [ ] Agent panel shows live specialist status.
- [ ] Integration tests: full task flow end-to-end with golden summary checks per specialist.
- [ ] Benchmark on `forgiven-bench/` shows total tokens per task ≤ current single-agent baseline, with answer quality ≥ current baseline.

---

## Measurement plan

On the `forgiven-bench/` corpus:

| Metric | Target |
|---|---|
| Mean tokens per task | ≤ current single-agent baseline (ADR 0130) |
| Answer quality (F1 vs. golden) | +5 to +10 pp over current baseline |
| P95 latency (end-to-end) | ≤ 1.5× current (extra specialist hops add latency) |
| Specialist context rot | Measured by per-specialist F1 degradation over rounds; target: no degradation within a specialist's 3-round budget |

Honest projection: total tokens may not decrease — the Navigator does the same reading work, just in a separate context. The win comes from **answer quality** and **context hygiene**, not raw token count. If the corpus shows token parity with quality gain, that is success.

---

## Risks and trade-offs

**Latency multiplication.** Each specialist boundary is a new API round-trip. A task that was 5 rounds in a single agent becomes 1 (Navigator) + 1 (Planner) + 3 (Editor) + 1 (Tester) = 6 rounds minimum. Mitigation: specialists can run in parallel where the supervisor permits (Navigator + Planner can sometimes be concurrent).

**Orchestration bugs.** The state machine is new surface area. Rust's type system helps (the `SubagentOutputSchema` enum forces explicit handling), but the flow itself has branching and retry logic that needs careful testing.

**User mental model.** Users are used to a single chat. A multi-specialist flow needs clear UX to avoid confusion. Mitigation: the top-level Goal + Plan view is always visible; specialist drill-down is opt-in.

**Specialist handoff quality.** If the Navigator's summary is bad, the Planner is blind. Mitigation: the output schema is strict — the Navigator cannot return a free-form blob, it must populate the structured fields.

**Massive surface area change.** This is a 4–6 week effort. During development, the single-agent loop must keep working. Mitigation: specialists live alongside the current loop behind a feature flag; the current loop remains the default until the new one is validated.

**Unknown interaction with existing features.** ADR 0081 importance-scoring, ADR 0077 history truncation, ADR 0123 observation masking — all operate on the conversation history of the (now-supervisor) level. These need review to ensure they still apply correctly when most content is in specialist sub-contexts.

**Cost if using cloud models.** Four specialists running on Claude Sonnet each round quadruples API cost per task. Mitigation: mix local (Ollama for Navigator) and cloud (Sonnet for Planner/Editor). The Intent Translator from Option D can inform this routing.

---

## Out of scope

- Custom user-defined specialists (future: `.forgiven/agents/custom/` loaded like Option B specialists).
- Specialist-to-specialist direct communication (all coordination goes through the supervisor).
- Specialists running on multiple machines.
- Replacing the Rust state-machine supervisor with an LLM supervisor (explicitly rejected in this spec).

---

## Implementation order

This effort is long enough to justify sub-milestones.

### Milestone 1 (week 1): Navigator only

1. Define `Subagent` trait, `SubagentOutputSchema` enum.
2. Implement Navigator and its tool allowlist.
3. Supervisor state machine — minimal: dispatch Navigator, collect summary, hand to existing single-agent loop as context.
4. Validate on 5 real tasks. If the Navigator summary is consistently useful, proceed.

### Milestone 2 (week 2): Planner + Editor

5. Implement Planner (pure-reasoning LLM call with Navigator summary).
6. Implement Editor specialist.
7. Supervisor dispatches Navigator → Planner → Editor sequence.
8. Existing single-agent loop still available behind flag.

### Milestone 3 (week 3): Tester + full flow

9. Implement Tester (restricted execute_command).
10. Supervisor handles test failures → feedback to Editor.
11. Full end-to-end flow for "add code + test" tasks.

### Milestone 4 (week 4): UX + benchmark

12. Subagent inspector (`SPC a s`).
13. Live specialist status in agent panel.
14. Full `forgiven-bench/` benchmark.
15. ADR documenting the decision.

### Milestone 5 (week 5–6): Polish + rollout

16. Parallel specialist execution where safe.
17. Performance profiling.
18. Default-on once benchmarks are clean.

---

## Related work

- **ADR 0128** — Investigation subagent. The proof of concept for the pattern. Becomes one flavour of Navigator in the full architecture.
- **ADR 0011** — Agentic tool-calling loop. The current single-agent implementation. Becomes the supervisor's fallback for pure-chat tasks.
- **ADR 0057** — ask_user. Reused for Supervisor ↔ user clarifications.
- **Option B (Codified Context)** — specialists can load Tier-2 domain specialist prompts dynamically.
- **Option C (Artifact Layer)** — Navigator produces artifacts that Planner/Editor reference without re-reading.
- **Option D (Intent Translator)** — feeds the Supervisor structured intent; dramatically improves dispatch quality.

## References

- Bouzenia et al. (2024). *MASAI: Modular Architecture for Software-engineering AI Agents.* 28.3% resolution on SWE-Bench Lite vs. single-agent baseline. Five specialist agents with sharply defined roles.
- HyperAgent team (2024). Planner/Navigator/Editor/Executor decomposition, improved issue resolution rates on complex repositories.
- OpenDev Terminal Agent (March 2026, arXiv:2603.05344). Production implementation showing "each subagent operates with an isolated context window... prevents cross-contamination between different phases of the workflow."
- Chroma (July 2025). Context rot evaluation of 18 state-of-the-art models — significant accuracy gaps between focused prompts (~300 tokens) vs. full context (~113K tokens).
