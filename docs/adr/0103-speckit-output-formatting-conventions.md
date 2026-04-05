# ADR 0103 — Speckit Output Formatting Conventions

**Date:** 2026-04-04
**Status:** Accepted

---

## Context

Speckit templates produce markdown documents that are read both in-editor (rendered TUI markdown) and by subsequent agent phases as machine-readable inputs. Two recurring readability problems appeared in real `/speckit.tasks` and `/speckit.clarify` output:

1. **Ambiguous numbering.** The `clarify` phase labelled ambiguity items A–F and resolution options 1–3, then numbered closing questions 1–6. The restart of the `1` counter inside each lettered item made it impossible to reference a specific option precisely (e.g. "option 2" under which item?). The `tasks` phase re-numbered tasks per phase, so dependency references like "depends on task 3" were ambiguous across phase boundaries.

2. **Unhighlighted code artifacts.** File paths, HTTP routes, header names, config keys, CLI commands, story IDs, and decision-record IDs were emitted as plain prose. This made them harder to scan visually and more likely to be mis-read or mis-copied by both humans and agents consuming the output.

Both problems are symptoms of missing formatting constraints in the templates. The fix belongs in the templates, not in post-processing.

---

## Decision

### 1. Universal backtick rule (all templates)

Every template in `src/spec_framework/templates/speckit/` now includes an explicit instruction to wrap the following in backticks wherever they appear in generated output:

- File and directory paths (e.g. `src/auth/mod.rs`, `docs/spec/features/foo/PLAN.md`)
- HTTP methods and routes (e.g. `POST /mcp`)
- Header names (e.g. `MCP-Protocol-Version`)
- Config keys and TOML fields (e.g. `janitor_threshold_tokens`)
- CLI commands (e.g. `cargo test`)
- Story IDs (e.g. `US-004`) and decision-record IDs (e.g. `DR-104`)
- Module, function, and type names

This rule is **general** — it improves every phase's output and applies regardless of content.

### 2. Per-template stable identifier schemes (numbering)

Each output-heavy template adopts an identifier scheme that allows unambiguous cross-referencing:

| Template | Scheme |
|---|---|
| `specify.md` | Stories: `US-001`, `US-002` …; acceptance criteria: `US-001-AC1`, `US-001-AC2` … |
| `clarify.md` | Ambiguity items: **A**, **B**, **C** …; resolution options: **A1**, **A2**, **B1** …; closing questions: **Question A**, **Question B** … |
| `tasks.md` | Tasks globally: `T-01`, `T-02` … across all phase boundaries |
| `analyze.md` | Conformant: `C-01` …; Drift: `D-01` …; Missing: `M-01` …; corrective actions reference by code (e.g. "Fix for D-01") |

`plan.md` and `constitution.md` are primarily narrative; no new numbering scheme is required, but the backtick rule still applies.

The schemes are intentionally **stable** (a task is always `T-07`, never "task 3 of phase 2") and **prefix-typed** (the letter encodes the category, preventing collisions across sections).

---

## Implementation

### Files changed

| File | Change |
|---|---|
| `src/spec_framework/templates/speckit/clarify.md` | Backtick rule; `A`/`B`/`C` item labels; `A1`/`A2` resolution options; `Question A` closing questions |
| `src/spec_framework/templates/speckit/tasks.md` | Backtick rule; global `T-01`/`T-02` numbering across phase boundaries |
| `src/spec_framework/templates/speckit/analyze.md` | Backtick rule; `C-xx`/`D-xx`/`M-xx` section-typed identifiers; corrective actions cross-referenced by code |
| `src/spec_framework/templates/speckit/plan.md` | Backtick rule added to step 4 output guidance |
| `src/spec_framework/templates/speckit/specify.md` | Backtick rule; `US-001` story IDs; `US-001-AC1` acceptance-criteria IDs |

No changes to runtime code, config, or existing spec documents. Existing `SPEC.md` / `PLAN.md` / `TASKS.md` files produced before this ADR are not retroactively renamed; the new schemes apply to newly generated or re-generated documents.

---

## Consequences

**Positive**
- Option and task references in user replies and agent prompts are now unambiguous (`A2`, `T-07`, `D-03`), reducing back-and-forth clarification.
- Backtick-wrapped identifiers render as inline code in the TUI markdown renderer, providing immediate visual scan-ability.
- Stable global task IDs (`T-01` … `T-N`) mean dependency lists in `TASKS.md` remain valid even after tasks are reordered or phases are split.
- Acceptance-criteria IDs (`US-001-AC1`) allow test cases, issues, and PR descriptions to reference spec requirements precisely.

**Negative / trade-offs**
- Templates are slightly longer and more prescriptive. An agent that ignores formatting instructions will still produce functionally correct output; the formatting is a quality improvement, not a correctness requirement.
- Existing spec documents use ad-hoc numbering. There is a minor risk of confusion if old and new documents are mixed in the same feature directory. The mitigation is to re-run the relevant phase to regenerate documents under the new scheme.

---

## Alternatives considered

| Alternative | Rejected because |
|---|---|
| Post-process agent output with a formatter | Adds complexity and a separate processing step; better to emit correct output from the template |
| Enforce formatting via a linter on saved `.md` files | Requires tooling not present in the repo; templates are simpler and catch the problem at generation time |
| Apply numbering only to `clarify` and `tasks` (where the problem was observed) | The root cause (no formatting guidance) applies to all templates; piecemeal fixes leave future phases inconsistent |
| Use emoji prefixes (✅ ⚠️ ❌) as identifiers in clarify/tasks | `analyze.md` already uses these for section headers; using them as item IDs would conflate section type with item identity |

---

## Related ADRs

| ADR | Relation |
|---|---|
| [0056](0056-spec-framework-integration.md) | Original Spec-Driven Development framework introduction |
| [0097](0097-speckit-auto-clear-context-per-phase.md) | Auto-clear context per phase — sister quality-of-life improvement to the speckit workflow |
