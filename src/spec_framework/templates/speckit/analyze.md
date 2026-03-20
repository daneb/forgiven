You are performing the ANALYZE step of Spec-Driven Development.

Your goal is to audit the codebase for consistency with the spec and flag any drift
between what was specified and what was implemented.

## Deriving the feature name

The user context below may begin with a feature name (slug). If present, scope your
analysis to `docs/spec/features/<feature-name>/`. If absent, analyze all feature
folders under `docs/spec/features/`.

## Steps

1. Determine scope:
   - If a feature name is given: FEATURE_DIR = `docs/spec/features/<feature-name>/`
   - Otherwise: list all subdirectories under `docs/spec/features/` and analyze each.
2. Read `docs/spec/CONSTITUTION.md` for project-wide constraints.
3. For each feature in scope, read all `.md` files in its folder (SPEC.md, PLAN.md,
   TASKS.md) to understand the intended design.
4. Scan the codebase (use list_directory and read_file as needed) and compare the
   implementation against:
   - **Architecture conformance** — does the module/directory layout match `PLAN.md`?
   - **Spec coverage** — is every P0/P1 user story implemented? List any gaps.
   - **Data model fidelity** — do the actual data structures match the planned model?
   - **Acceptance criteria** — for each story, is the acceptance condition met?
   - **Tech stack** — are there any undocumented dependencies or deviations?
5. Produce a report with three sections:
   - ✅ **Conformant** — areas that match the spec.
   - ⚠️  **Drift** — areas that deviate from the spec (with severity: minor / major).
   - ❌ **Missing** — specified work that has not been implemented yet.
6. For each drift or missing item, suggest the minimal corrective action.
7. Ask the user whether to update the spec to reflect intentional deviations, or to
   implement the missing/drifted items.

## User context

