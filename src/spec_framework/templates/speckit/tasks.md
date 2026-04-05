You are performing Phase 4 of Spec-Driven Development: TASKS.

Your goal is to decompose the plan into a sequenced, executable work breakdown that
an implementation agent can follow step-by-step without ambiguity.

## Deriving the feature name

The user context below begins with the feature name (the same slug used in the earlier
phases). All files for this feature live under `docs/spec/features/<feature-name>/`.

If no feature name is provided, ask the user for one before proceeding.

## Steps

1. Extract the feature name from the user context (first token). Set:
   - FEATURE = <feature-name>
   - FEATURE_DIR = `docs/spec/features/<FEATURE>/`
2. Read `FEATURE_DIR/PLAN.md` and `FEATURE_DIR/SPEC.md`. If either is missing, ask
   the user to run the earlier phases first.
3. Check whether `FEATURE_DIR/TASKS.md` already exists; load it if so.
4. Produce `FEATURE_DIR/TASKS.md` as an ordered task list. Each task must:
   - Have a short, imperative title (e.g. "Create database schema migration").
   - State its **inputs** (files to read/create) and **outputs** (files to write/modify).
   - List any **dependencies** (task numbers that must complete first).
   - Include a one-sentence **acceptance condition** (how to verify it's done).
   - Wrap all file paths, module names, function names, CLI commands, and HTTP
     routes in backticks (e.g. `src/auth/mod.rs`, `POST /mcp`, `cargo test`).
5. Rules for the task list:
   - Tasks must be atomic — each changes or creates exactly one logical unit of work.
   - Order tasks so each one builds on what's already there (no forward dependencies).
   - Reading a file before editing it does NOT count as a separate task.
   - Group related tasks with a `## Phase` heading (e.g. `## Phase 1 — Scaffolding`).
   - Number tasks globally and continuously across phases (T-01, T-02 …) so
     dependency references are unambiguous regardless of phase boundaries.
6. Save the document to `FEATURE_DIR/TASKS.md`. Present the full task list to the
   user and confirm they agree with the ordering and scope before proceeding.
7. End your reply noting readiness for Phase 5 (`/speckit.implement <FEATURE>`).

## User context

