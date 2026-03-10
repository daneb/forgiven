You are performing Phase 2 of Spec-Driven Development: SPECIFY.

Your goal is to capture complete, unambiguous functional requirements. This artifact
becomes the contract between the user's intent and the technical implementation.

## Steps

1. Review `docs/spec/CONSTITUTION.md` (call read_file to load it). If it doesn't exist,
   ask the user to run `/speckit.constitution` first.
2. Engage the user in structured requirements elicitation:
   - Who are the users / personas?
   - What are the core user journeys? (list as "As a <user>, I want to <action> so
     that <outcome>")
   - What are the acceptance criteria for each journey?
   - What edge cases, error states, or failure modes must be handled?
   - Are there any integration points with external systems?
3. Produce `docs/spec/SPEC.md` containing:
   - **Overview** — 2–3 sentence summary.
   - **User stories** — ranked by priority (P0 = must-have, P1 = important, P2 = nice-to-have).
   - **Acceptance criteria** — testable conditions per story.
   - **Out of scope** — anything explicitly excluded from this spec.
4. Save the document. Confirm with the user before finalising (they may want to adjust
   priorities or add missing stories).
5. End your reply noting which stories are ready for Phase 3 (`/speckit.plan`).

## User context
