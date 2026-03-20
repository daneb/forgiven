You are performing Phase 2 of Spec-Driven Development: SPECIFY.

Your goal is to capture complete, unambiguous functional requirements for a specific
feature. This artifact becomes the contract between the user's intent and the technical
implementation.

## Deriving the feature name

The user context below begins with a feature name (the first word or short slug, e.g.
"inline-rename" or "split-view"). Use that slug as-is — do not transform it. All files
for this feature live under `docs/spec/features/<feature-name>/`.

If no feature name is provided, ask the user for one before proceeding.

## Steps

1. Extract the feature name from the user context (first token). Set:
   - FEATURE = <feature-name>
   - FEATURE_DIR = `docs/spec/features/<FEATURE>/`
2. Read `docs/spec/CONSTITUTION.md` (call read_file). If it doesn't exist, ask the
   user to run `/speckit.constitution` first.
3. Check whether `FEATURE_DIR/SPEC.md` already exists (call read_file). If it does,
   load it so you can amend rather than overwrite.
4. Engage the user in structured requirements elicitation:
   - Who are the users / personas?
   - What are the core user journeys? (list as "As a <user>, I want to <action> so
     that <outcome>")
   - What are the acceptance criteria for each journey?
   - What edge cases, error states, or failure modes must be handled?
   - Are there any integration points with external systems?
5. Produce `FEATURE_DIR/SPEC.md` containing:
   - **Overview** — 2–3 sentence summary.
   - **User stories** — ranked by priority (P0 = must-have, P1 = important, P2 = nice-to-have).
   - **Acceptance criteria** — testable conditions per story.
   - **Out of scope** — anything explicitly excluded from this spec.
6. Save the document to `FEATURE_DIR/SPEC.md` (create the directory if needed).
   Confirm with the user before finalising (they may want to adjust priorities or add
   missing stories).
7. End your reply noting which stories are ready for Phase 3
   (`/speckit.plan <FEATURE>`).

## User context

