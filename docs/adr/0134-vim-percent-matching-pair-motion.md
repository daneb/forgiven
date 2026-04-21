# ADR 0134 — Vim `%` Matching-Pair Motion

**Date:** 2026-04-21
**Status:** Accepted

## Context

Forgiven's Normal-mode keymap covered most everyday Vim motions — character find (`f`/`t`), word motions (`w`/`b`), line-start/end (`0`/`$`), and file-top/bottom (`gg`/`G`) — but lacked `%`, Vim's bracket-jump motion. `%` is used constantly when navigating code: quickly jumping between the opening and closing of a function body, a conditional block, or a parenthesised expression saves many keystrokes compared to searching or counting `j`/`k` presses.

No existing ADR or source code addressed bracket matching as a navigation primitive. The surround-operations module (ADR 0110) contained a related `find_surround_on_line()` helper, but it is scoped to the current line and finds the *innermost enclosing* pair — not the match for a bracket already under the cursor, and not across line boundaries.

## Decision

### New `Action` variant (`src/keymap/mod.rs`)

Added `JumpMatchingPair` to the `Action` enum, in a new *Bracket navigation* section.

### Key binding (`src/keymap/mod.rs`)

Bound `%` as a single-key Normal-mode action in the direct binding table:

```
KeyCode::Char('%') => Action::JumpMatchingPair,
```

No pending-key machinery is needed — `%` takes no argument.

### Buffer method (`src/buffer/buffer.rs`)

Added `find_matching_pair(&self) -> Option<(usize, usize)>`:

**Algorithm:**

1. Inspect the character at `cursor.col` on the current line.
2. If it is an open bracket (`(`, `[`, `{`), scan **forward** across lines, tracking nesting depth. Return the position where depth reaches zero.
3. If it is a close bracket (`)`, `]`, `}`), scan **backward** across lines, tracking nesting depth. Return the position where depth reaches zero.
4. If the cursor is not on any bracket, scan **forward on the current line only** for the next bracket character and then apply rule 2 or 3 from that position — matching Vim's behaviour where `%` on a non-bracket line finds and jumps from the next bracket on that line.

The `bracket_kind()` inner function maps any bracket character to its `(open, close, is_forward)` triple, keeping the match logic DRY.

Multi-line scanning reads each line into a `Vec<char>` on demand; no pre-allocation of the whole buffer is required.

### Action handler (`src/editor/actions.rs`)

Added a match arm after `FindCharBackward`:

```rust
Action::JumpMatchingPair => {
    self.with_buffer(|buf| {
        if let Some((row, col)) = buf.find_matching_pair() {
            buf.move_cursor_to(row, col);
        }
    });
},
```

`move_cursor_to` already existed on `Buffer` and clamps both row and column safely.

## Consequences

- `%` works across line boundaries for `()`, `[]`, and `{}` with correct nesting (e.g. `%` on the `(` of `foo(bar(x), baz)` lands on the outer `)`, not the inner one).
- Vim also extends `%` to HTML/XML tags and to `/*`/`*/` comment pairs via the *matchit* plugin. Neither is implemented; the supported set is bracket pairs only.
- The motion is read-only (no clipboard, no undo snapshot needed).
- Visual-mode extension (`v%` to select to the matching bracket) is not wired up; it is a natural follow-on that can reuse `find_matching_pair` once Visual anchor/extend logic is factored out.
