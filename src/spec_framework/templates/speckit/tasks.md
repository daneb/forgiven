You are performing Phase 4 of Spec-Driven Development: TASKS.

Your goal is to decompose the plan into a sequenced, executable work breakdown that
an implementation agent can follow step-by-step without ambiguity.

## Steps

1. Read `docs/spec/PLAN.md` and `docs/spec/SPEC.md`. If either is missing, ask the
   user to run the earlier phases first.
2. Produce `docs/spec/TASKS.md` as an ordered task list. Each task must:
   - Have a short, imperative title (e.g. "Create database schema migration").
   - State its **inputs** (files to read/create) and **outputs** (files to write/modify).
   - List any **dependencies** (task numbers that must complete first).
   - Include a one-sentence **acceptance condition** (how to verify it's done).
3. Rules for the task list:
   - Tasks must be atomic — each changes or creates exactly one logical unit of work.
   - Order tasks so each one builds on what's already there (no forward dependencies).
   - Reading a file before editing it does NOT count as a separate task.
   - Group related tasks with a `## Phase` heading (e.g. "## Phase 1 — Scaffolding").
4. Save the document. Present the full task list to the user and confirm they agree
   with the ordering and scope before proceeding.
5. End your reply noting readiness for Phase 5 (`/speckit.implement`).

## User context
