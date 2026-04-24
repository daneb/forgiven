---
clears_context: true
---

You are performing the **REVIEW** step of OpenSpec.

Your goal is to audit the three proposal artefacts for completeness, internal consistency, and alignment with the project constitution — before a single line of implementation is written.

## Deriving the change name

The user context below begins with the change name slug. All artefacts live under `openspec/changes/<change-name>/`.

If no change name is given, ask for one before proceeding.

## Steps

1. Set `CHANGE` = the slug from user context, `CHANGE_DIR` = `openspec/changes/<CHANGE>/`.
2. Read `docs/spec/CONSTITUTION.md` (if it exists), then read `CHANGE_DIR/proposal.md`, `CHANGE_DIR/design.md`, and `CHANGE_DIR/tasks.md`. If any of the three change artefacts is missing, stop and ask the user to run `/openspec.propose <CHANGE>` first.
3. Review `proposal.md` for:
   - Every user story has at least one acceptance criterion.
   - Every acceptance criterion is testable — not vague ("should work") but specific ("returns HTTP 200 with body matching schema X").
   - Out-of-scope is explicit and does not hide real requirements as exclusions.
   - Open questions list is either empty (resolved) or has a disposition for each item.
4. Review `design.md` for:
   - Tech stack decisions are consistent with Constitution constraints.
   - No undocumented external dependencies.
   - Architecture overview accounts for every user story in `proposal.md`.
   - Security considerations address the obvious threat surface of this change.
5. Review `tasks.md` for:
   - Every task is atomic (one logical unit per task).
   - No task has a forward dependency (tasks can be executed in T-NN order).
   - Every task traces to at least one user story in `proposal.md`.
   - The complete task list covers every acceptance criterion in `proposal.md`.
   - Checkbox syntax (`- [ ]`) is used for every task.
6. Produce a gap report with exactly three sections:
   - **Ready** — `R-01`, `R-02` … items confirmed complete and correct.
   - **Gap** — `G-01`, `G-02` … items that must be resolved before apply (blocking).
   - **Suggestion** — `S-01`, `S-02` … optional improvements (non-blocking).
   Wrap all file paths, story IDs, task IDs, and code references in backticks.
7. For each Gap item: propose a specific correction and ask the user whether to apply it now or proceed anyway.
8. On user approval of each correction, update the relevant artefact file and confirm. Once all gaps are resolved (or accepted), note readiness for `/openspec.apply <CHANGE>`.

## User context