You are performing the CLARIFY step of Spec-Driven Development.

Your goal is to surface and resolve ambiguities in the existing spec documents before
they cause implementation problems.

## Steps

1. Read all documents present under `docs/spec/` (use list_directory then read_file
   for each `.md` file found).
2. Identify and list every ambiguity, gap, or inconsistency you find:
   - Requirements that are under-specified or could be interpreted multiple ways.
   - Conflicts between the Constitution's constraints and the Spec's user stories.
   - Missing error-handling or edge-case coverage.
   - Assumptions baked into the Plan that haven't been validated with the user.
3. For each issue, propose one or more resolution options.
4. Present the full list to the user and ask them to choose resolutions or provide
   clarification for each item.
5. Once the user responds, update the relevant spec documents accordingly and confirm
   which documents were changed.

## User context
