You are performing Phase 3 of Spec-Driven Development: PLAN.

Your goal is to produce a concrete technical architecture that satisfies the spec
without over-engineering. Every decision should be traceable to a requirement.

## Deriving the feature name

The user context below begins with the feature name (the same slug used in
`/speckit.specify`). All files for this feature live under
`docs/spec/features/<feature-name>/`.

If no feature name is provided, ask the user for one before proceeding.

## Steps

1. Extract the feature name from the user context (first token). Set:
   - FEATURE = <feature-name>
   - FEATURE_DIR = `docs/spec/features/<FEATURE>/`
2. Read `docs/spec/CONSTITUTION.md` and `FEATURE_DIR/SPEC.md`. If either is missing,
   ask the user to run the earlier phases first.
3. Check whether `FEATURE_DIR/PLAN.md` already exists; load it if so.
4. Produce `FEATURE_DIR/PLAN.md` containing:
   - **Tech stack** — languages, frameworks, major libraries, with brief justification
     for each choice against the Constitution's constraints.
   - **Architecture overview** — a description of the major components and how they
     interact. Include a Mermaid diagram if the structure benefits from visualisation.
   - **Module / directory layout** — the proposed file/folder structure.
   - **Data model** — key data structures, schemas, or types.
   - **External integrations** — APIs, services, auth flows.
   - **Security considerations** — how the design addresses threat surfaces.
   - **Open questions / risks** — unknowns that could affect the plan.
   Wrap all file paths, module names, function names, HTTP routes, header names,
   config keys, and story/decision-record IDs in backticks throughout the document.
5. For each architectural decision that deviates from the obvious default, write a
   one-sentence "Decision Record" explaining why.
6. Save the document to `FEATURE_DIR/PLAN.md`. Ask the user to review before
   proceeding.
7. End your reply noting readiness for Phase 4 (`/speckit.tasks <FEATURE>`).

## User context

