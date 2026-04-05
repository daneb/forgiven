You are performing the CLARIFY step of Spec-Driven Development.

Your goal is to surface and resolve ambiguities in a feature's spec documents before
they cause implementation problems.

## Deriving the feature name

The user context below may begin with a feature name (slug). If present, scope your
work to `docs/spec/features/<feature-name>/`. If absent, scan all feature folders
under `docs/spec/features/` and clarify across all of them.

## Steps

1. Determine scope:
   - If a feature name is given: FEATURE_DIR = `docs/spec/features/<feature-name>/`
   - Otherwise: list all subdirectories under `docs/spec/features/` and process each.
2. Also read `docs/spec/CONSTITUTION.md` for project-wide constraints.
3. For each feature in scope, read all `.md` files present in its folder.
4. Identify and list every ambiguity, gap, or inconsistency you find:
   - Requirements that are under-specified or could be interpreted multiple ways.
   - Conflicts between the Constitution's constraints and the Spec's user stories.
   - Missing error-handling or edge-case coverage.
   - Assumptions baked into the Plan that haven't been validated with the user.
5. For each issue, propose one or more resolution options. Follow these formatting rules:
   - Label items with sequential letters (**A**, **B**, **C** …).
   - Label resolution options within each item as **A1**, **A2**, **B1**, **B2**, etc.
     (prefix with the parent letter) so they are never ambiguous alongside the closing
     question numbers.
   - Wrap all file paths, HTTP methods/routes, header names, error codes, story IDs
     (e.g. `US-004`), and decision-record IDs (e.g. `DR-104`) in backticks.
6. Present the full list to the user with a closing "Questions for Clarification"
   section. Number each question to match its item letter (e.g. **Question A**, **Question B**).
   Ask the user to choose a resolution option (by code, e.g. "A2") or provide free-form
   clarification for each item.
7. Once the user responds, update the relevant spec documents accordingly and confirm
   which documents were changed.

## User context

