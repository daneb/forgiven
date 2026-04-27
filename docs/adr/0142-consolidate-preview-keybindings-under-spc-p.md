# ADR 0142 — Consolidate Preview Keybindings Under `SPC p`

**Date:** 2026-04-26
**Status:** Implemented

---

## Context

The leader key tree had grown to ten top-level namespaces. Two of them overlapped
in purpose:

- **`SPC m`** ("markdown/preview") — `p` toggle preview, `b` open browser, `w` soft wrap
- **`SPC p`** ("preview/companion") — `c` toggle companion window (added in ADR 0141)

Both namespaces are about rendering and viewing buffer content. Splitting them
meant the which-key popup showed two separate entries for what is conceptually one
feature group, and users had to remember which preview action lived under which
letter.

Additionally, `README.md` listed `SPC m c` (CSV preview) and `SPC m j` (JSON
preview) as if they were shipped features. Neither appeared in the `Action` enum
or the keymap — they were aspirational entries carried forward from ADR 0125
planning notes.

---

## Decision

Remove `SPC m` entirely. Move its three bindings into `SPC p`:

| Old | New | Action |
|-----|-----|--------|
| `SPC m p` | `SPC p p` | Toggle inline markdown preview |
| `SPC m b` | `SPC p b` | Render to HTML, open in browser |
| `SPC m w` | `SPC p w` | Toggle soft wrap |
| *(new in ADR 0141)* | `SPC p c` | Toggle companion window |

The `SPC p` node description changes from "preview/companion" to "preview" —
a single, self-explanatory namespace for all buffer-rendering concerns.

Remove the unimplemented `SPC m c` and `SPC m j` entries from `README.md`.
They belong in a future ADR when CSV/JSON preview modes are actually built.

ADRs that reference `SPC m p` / `SPC m b` / `SPC m w` (0022, 0033, 0070, 0076,
0125, 0137) are left unchanged — they record what was decided at the time, not
the current binding. `docs/reference.md` and `README.md` are the authoritative
sources for current keybindings and are updated here.

---

## Alternatives considered

**Keep both namespaces, add a `SPC m` alias for `SPC p`** — aliases add
invisible indirection and duplicate which-key entries. The fix is simpler.

**Use `SPC b` for the unified node** — `SPC b` is already the buffer namespace.

**Rename to `SPC v` (view)** — shorter name, but `SPC p` is already in use and
renaming would invalidate ADR 0141 immediately after shipping it.

---

## Consequences

- The which-key popup loses one top-level entry (`m`), reducing noise.
- All preview-related actions are reachable under a single mnemonic (`p` for
  preview).
- Existing muscle memory for `SPC m p` / `SPC m b` / `SPC m w` requires
  relearning — the namespace letter changes from `m` to `p`, the sub-key stays
  the same.
- CSV and JSON preview modes (ADR 0125) will be added under `SPC p` when
  implemented (`SPC p c` is taken by the companion, so candidates are `SPC p j`
  and `SPC p v`).
