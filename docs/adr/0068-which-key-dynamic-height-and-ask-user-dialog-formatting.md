# ADR 0068 — Which-Key Dynamic Height and Ask-User Dialog Formatting

**Date:** 2026-03-18
**Status:** Accepted

---

## Context

Two UI rendering issues affected usability:

### Which-key popup clipping

The which-key popup (shown after pressing `SPC` and waiting 500 ms) used a
hardcoded `Constraint::Length(10)` in the vertical layout. With 10 top-level
leader keys registered (`a` agent, `b` buffer, `d` diagnostics, `e` explorer,
`f` file, `g` git, `l` lsp, `m` markdown, `s` search, `w` window), plus 1
header line and 2 border rows, 13 rows were needed but only 10 were available.
The `g`, `m`, and `s` entries were silently clipped — users could not discover
project-wide search (`SPC s g`) from the popup.

Additionally the leader tree used `HashMap<char, KeyNode>`, which iterates in
non-deterministic order. The key listing appeared in a different order every
session, making muscle-memory harder to build.

### Ask-user dialog rendering

The agent `ask_user` tool dialog rendered the question text as a single
`Span::styled(...)`. Ratatui does not interpret `\n` inside a `Span` — it
treats the entire content as one logical line. Structured questions from the
agent (numbered items, lettered sub-options, line breaks) were flattened into
an unreadable wall of text.

The height calculation estimated wrapped rows from total character count
(`chars / inner_width`), which was inaccurate for text containing explicit
newlines — some lines were short, others wrapped, and the estimate could be
either too tall or too short.

---

## Decision

### Which-key: dynamic height + sorted keys

1. **`BTreeMap` replaces `HashMap`** for the leader tree and all `KeyNode`
   child maps in `src/keymap/mod.rs`. `BTreeMap` iterates in sorted key
   order, so entries always appear alphabetically (`a`, `b`, `d`, …, `w`).

2. **Dynamic `Constraint::Length`** in `src/ui/mod.rs`: the which-key popup
   height is computed as `options.len() + 3` (number of entries + 2 border
   rows + 1 header row). This grows automatically as new leader keys are
   added.

3. **Viewport height** in `src/editor/mod.rs`: the scroll-guard calculation
   that subtracts the which-key height from the terminal height was updated
   from the hardcoded `11` to use the same dynamic formula.

### Ask-user dialog: newline-aware rendering

1. **Split on `\n`** — the question text is split via `.lines()` into
   separate `Line` entries, so each newline in the agent's question becomes
   a visible line break in the dialog.

2. **Accurate row estimation** — each logical line's display height is
   computed as `ceil(chars / inner_width)`, then all per-line heights are
   summed. This accounts for short lines, empty lines, and long wrapped
   lines correctly.

3. **Single scrollable paragraph** — the question lines and option lines
   are combined into one `Paragraph` with `Wrap { trim: false }` and
   `scroll((offset, 0))`. If the content exceeds the dialog height, the
   paragraph scrolls so that the selectable options remain visible at the
   bottom.

4. **Dynamic dialog height** — the dialog height is computed from actual
   content rows (`question_rows + option_rows + borders`), clamped to the
   panel area, replacing the old character-count heuristic.

---

## Consequences

- **All leader keys visible** — every registered `SPC` binding now appears
  in the which-key popup regardless of how many are added in the future.
- **Deterministic ordering** — keys are always alphabetically sorted; the
  popup looks the same on every launch.
- **Readable agent questions** — structured questions with numbered items
  and sub-options render with proper line breaks and wrapping.
- **No new dependencies** — `BTreeMap` is in `std::collections`; all other
  changes are layout/rendering logic.
- **Minor perf note** — `BTreeMap` is O(log n) vs `HashMap` O(1) for
  lookups, but with ≤ 15 entries the difference is immeasurable.

### Files changed

| File | Change |
|------|--------|
| `src/keymap/mod.rs` | `HashMap` → `BTreeMap` for `leader_tree` and `KeyNode.children` |
| `src/ui/mod.rs` | Which-key: `Constraint::Length(10)` → dynamic; ask-user: newline-aware rendering with scroll |
| `src/editor/mod.rs` | Viewport height calculation uses dynamic which-key height |
