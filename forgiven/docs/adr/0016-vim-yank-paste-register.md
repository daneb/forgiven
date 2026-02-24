# ADR 0016 — Vim Yank / Paste Register

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

A modal editor without a working yank register is effectively broken for anyone
with vim muscle memory. The previous state was partial: `clipboard: Option<String>`
existed on `Editor` and `p`/`P` / `dd` / `yy` were wired up, but the following
were missing or broken:

* Motion-based yank (`yw`, `y$`) and delete (`dw`) were not implemented.
* Visual-mode `y`, `d`, `x`, `c` had no operator handling — pressing them did
  nothing.
* Visual-mode motion keys (`w`, `b`, `0`, `$`, `G`) could not extend the
  selection.
* The change operator (`cc`, `cw`) did not exist.
* Nothing was ever written to the OS system clipboard.
* **Bug:** `paste_after_cursor` / `paste_before_cursor` always called
  `self.lines.insert(row, text)` — inserting the entire text as a single entry
  in the lines vector.  A multi-line visual yank (`"line1\nline2\nline3"`) was
  therefore stored as one line with literal `\n` characters, appearing as
  garbled content on paste.
* **Bug:** Charwise yanks (`yw`, `y$`) were pasted as new rows, not inline at
  the cursor — incorrect behaviour for character-wise operations.

---

## Decision

### 1. `ClipboardType` — linewise vs charwise register

`Editor.clipboard` was changed from `Option<String>` to
`Option<(String, ClipboardType)>` where:

```rust
enum ClipboardType {
    Linewise,  // yy / dd / cc  → p/P inserts new rows
    Charwise,  // yw / y$ / visual y/d/x/c → p/P inserts inline
}
```

| Operation | ClipboardType |
|-----------|---------------|
| `yy`, `dd`, `cc` | `Linewise` |
| `yw`, `y$`, `dw`, `D`, visual `y`/`d`/`x`/`c` | `Charwise` |

### 2. New buffer paste methods (`src/buffer/buffer.rs`)

The old `paste_after_cursor` / `paste_before_cursor` were replaced with four
specialised methods:

| Method | Behaviour |
|--------|-----------|
| `paste_linewise_after(text)` | Split `text` on `\n`, insert each part as a new row **below** cursor |
| `paste_linewise_before(text)` | Same, insert **above** cursor |
| `paste_charwise_after(text)` | Advance one col, call `insert_text_block` (multi-line aware) |
| `paste_charwise_before(text)` | Call `insert_text_block` at current cursor position |

`insert_text_block` was already present and handled multi-line inline
insertion correctly — only the dispatch was wrong.

### 3. `PasteAfter` / `PasteBefore` now dispatch on type

```rust
match clip_type {
    ClipboardType::Linewise => buf.paste_linewise_after(&text),
    ClipboardType::Charwise => buf.paste_charwise_after(&text),
}
```

### 4. New buffer methods for motions (`src/buffer/buffer.rs`)

| Method | Description |
|--------|-------------|
| `word_end_col() → usize` | Exclusive end column for yw/dw (private) |
| `yank_word() → String` | Copy cursor → word end |
| `yank_to_line_end() → String` | Copy cursor → EOL |
| `delete_word() → String` | Remove cursor → word end, return text |
| `yank_selection() → Option<String>` | Copy selection span (single or multi-line) |
| `delete_selection() → Option<String>` | Remove selection, return text, place cursor at start |

### 5. New `Action` variants (`src/keymap/mod.rs`)

`DeleteWord`, `YankWord`, `YankToLineEnd`, `YankSelection`, `DeleteSelection`,
`ChangeLine`, `ChangeWord`.

### 6. Pending-key combos added

| Keys | Action |
|------|--------|
| `dw` | `DeleteWord` |
| `d$` | `DeleteToLineEnd` |
| `yw` | `YankWord` |
| `y$` | `YankToLineEnd` |
| `cc` | `ChangeLine` |
| `cw` | `ChangeWord` |
| `c$` | `DeleteToLineEnd` (then Insert) |

`'c'` was added to the pending-key prefix set alongside `d`, `g`, `y`.

### 7. Visual-mode operators

`handle_visual_mode` now handles:

* `y` → `YankSelection` (yank as Charwise, clear selection, Normal)
* `d` / `x` → `DeleteSelection` (delete as Charwise, Normal)
* `c` → delete selection as Charwise, enter Insert
* `w` / `b` / `0` / `$` / `G` → motion + `update_selection()`

### 8. System clipboard sync (`arboard` crate)

`Editor::sync_system_clipboard(text)` wraps `arboard::Clipboard::new()` +
`set_text()`.  All errors are logged at `DEBUG` level and silently ignored —
the internal register is always the source of truth.

Every yank and delete also calls `sync_system_clipboard`, so the OS clipboard
always mirrors the last yank.

---

## Consequences

### Positive

* Full vim yank / paste muscle memory works: `yy`, `dd`, `yw`, `dw`, `y$`,
  `D`, `p`, `P`, `cc`, `cw`, visual `y`/`d`/`x`/`c`.
* Multi-line visual yank + `p` now inserts as multiple correct lines (not a
  single garbled line).
* Charwise yank (`yw`, `y$`) + `p` now inserts inline at the cursor, not as
  a new row.
* Yanked text automatically appears in the OS clipboard.
* `sync_system_clipboard` failure never crashes the editor.

### Negative / trade-offs

* `p`/`P` do not read the OS clipboard; they only use the internal register.
  To paste text copied from outside the editor the user must use the terminal
  emulator's own paste shortcut (e.g. ⌘V / Shift+Ctrl+V).
* `yw` / `dw` follow vim's "word + trailing space" semantics, consistent with
  vim's `w` / `dw`.
* `c$` uses `DeleteToLineEnd` and does **not** auto-enter Insert — a dedicated
  `ChangeToLineEnd` action should be added in a follow-up.

---

## Alternatives Considered

| Option | Reason rejected |
|--------|-----------------|
| Single `paste_*` method checking for `\n` | Doesn't address charwise vs linewise semantics — `yw` would still create a new row |
| Implement `"+` register prefix | Complex two-key prefix parsing; arboard auto-sync delivers the same benefit with no extra keystrokes |
| Paste from OS clipboard on `p` | Breaks internal register predictability |
| Named registers (`"a`–`"z`) | Out of scope for MVP |
