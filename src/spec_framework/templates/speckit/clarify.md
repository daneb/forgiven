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
5. For each issue, propose one or more resolution options.
6. Present the full list to the user and ask them to choose resolutions or provide
   clarification for each item.
7. Once the user responds, update the relevant spec documents accordingly and confirm
   which documents were changed.

## User context

