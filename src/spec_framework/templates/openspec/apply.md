---
clears_context: true
---

You are performing the **APPLY** step of OpenSpec.

Your goal is to implement every task in `tasks.md` exactly as specified, then archive the completed change into the `openspec/specs/` living library.

## Deriving the change name

The user context below begins with the change name slug. All artefacts live under `openspec/changes/<change-name>/`.

If no change name is given, ask for one before proceeding.

## Rules

- Set `CHANGE` = the slug from user context, `CHANGE_DIR` = `openspec/changes/<CHANGE>/`.
- Read `CHANGE_DIR/tasks.md` first. Execute tasks in `T-NN` order. Do not skip or reorder.
- Before editing any file, call `read_file` to see its current contents.
- Work without narration — no step-by-step commentary. Write a final summary only after all tasks are complete.
- If a task's acceptance condition cannot be met, stop and ask the user rather than guessing.
- Follow the tech stack and architecture defined in `CHANGE_DIR/design.md`. Do not introduce new dependencies without asking.
- Match coding conventions already present in the codebase — style, naming, error handling, test patterns.
- Read `docs/spec/CONSTITUTION.md` (if it exists) as a hard constraint throughout.

## After completing all tasks

1. Write a concise summary: what was built, file by file.
2. List any deviations from `tasks.md` and why they were necessary.
3. Suggest verification steps: specific tests to run, manual checks, edge cases.
4. Archive: copy `CHANGE_DIR/` into `openspec/specs/<CHANGE>/`.
5. Update (or create) `openspec/specs/INDEX.md` — append one row to the table:

   ```
   | `<CHANGE>` | <YYYY-MM-DD> | <one-sentence summary of what shipped> |
   ```

   Create the file with a header row if it does not yet exist:

   ```markdown
   # OpenSpec — Shipped Changes

   | Change | Date | Summary |
   |--------|------|---------|
   ```

6. Note readiness for the next change (`/openspec.propose <next-change-name>`).

## User context