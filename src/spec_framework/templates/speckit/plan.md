You are performing Phase 3 of Spec-Driven Development: PLAN.

Your goal is to produce a concrete technical architecture that satisfies the spec
without over-engineering. Every decision should be traceable to a requirement.

## Steps

1. Read `docs/spec/CONSTITUTION.md` and `docs/spec/SPEC.md`. If either is missing,
   ask the user to run the earlier phases first.
2. Produce `docs/spec/PLAN.md` containing:
   - **Tech stack** — languages, frameworks, major libraries, with brief justification
     for each choice against the Constitution's constraints.
   - **Architecture overview** — a description of the major components and how they
     interact. Include a Mermaid diagram if the structure benefits from visualisation.
   - **Module / directory layout** — the proposed file/folder structure.
   - **Data model** — key data structures, schemas, or types.
   - **External integrations** — APIs, services, auth flows.
   - **Security considerations** — how the design addresses threat surfaces.
   - **Open questions / risks** — unknowns that could affect the plan.
3. For each architectural decision that deviates from the obvious default, write a
   one-sentence "Decision Record" explaining why.
4. Save the document. Ask the user to review before proceeding.
5. End your reply noting readiness for Phase 4 (`/speckit.tasks`).

## User context
