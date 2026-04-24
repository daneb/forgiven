---
clears_context: true
---

You are performing the **PROPOSE** step of OpenSpec.

Your goal is to produce three artefacts that fully capture what will be built, why, and how — before any code is written. All three files must be written before this step ends.

## Deriving the change name

The user context below begins with a kebab-case slug (e.g. `agent-panel-resize`). Use it as-is. All artefacts for this change live under `openspec/changes/<change-name>/`.

If no change name is given, ask the user for one before proceeding.

## Steps

1. Set `CHANGE` = the slug from user context, `CHANGE_DIR` = `openspec/changes/<CHANGE>/`.
2. Read `docs/spec/CONSTITUTION.md` if it exists — use it as a hard constraint throughout (principles, limits, non-goals, success criteria).
3. Check whether `CHANGE_DIR/proposal.md` already exists; load it if so — you are amending, not overwriting.
4. Engage the user in lightweight requirements elicitation:
   - What problem does this change solve?
   - Who benefits and how?
   - What are the acceptance criteria (how will we know it is done)?
   - What is explicitly out of scope?
   - Are there open questions that must be resolved before design begins?
5. Write `CHANGE_DIR/proposal.md` containing exactly these sections:
   - **Problem** — 2–3 sentences on what is broken or missing.
   - **Solution summary** — the proposed approach in plain language.
   - **User stories** — labelled `US-001`, `US-002` …
   - **Acceptance criteria** — per story, labelled `US-001-AC1`, `US-001-AC2` …
   - **Out of scope** — explicit exclusions.
   - **Open questions** — anything unresolved before design begins (may be empty).
6. Write `CHANGE_DIR/design.md` containing exactly these sections:
   - **Tech stack** — languages, key crates/packages, rationale against Constitution constraints.
   - **Architecture overview** — components and interactions; include a Mermaid diagram if helpful.
   - **Module / directory layout** — proposed file and folder structure.
   - **Data model** — key types and schemas.
   - **Security considerations** — threat surface relevant to this change.
   - **Open questions / risks** — design-level unknowns (may be empty).
7. Write `CHANGE_DIR/tasks.md` as an ordered implementation checklist:
   - Each task has a short imperative title.
   - Tasks are labelled `T-01`, `T-02` … globally (no per-phase reset).
   - Each task states its inputs, expected outputs, and acceptance condition.
   - Tasks are atomic — one logical unit of work each.
   - Tasks are ordered with no forward dependencies.
   - Group with `## Phase N — <name>` headings.
8. Confirm all three files with the user and note readiness for `/openspec.review <CHANGE>`.

## Formatting rules (apply throughout all three files)

- Wrap all file paths, module names, CLI commands, HTTP routes, config keys, story IDs (`US-001`), task IDs (`T-01`), and ADR references in backticks.
- Use checkbox syntax (`- [ ]`) for every task in `tasks.md`.
- Do not invent requirements — capture only what the user confirms.

## User context