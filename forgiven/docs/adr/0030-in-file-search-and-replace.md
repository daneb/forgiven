# ADR 0030 — In-File Search and Replace

**Status:** Accepted

---

## Context

While ADR 0024 introduced project-wide text search (`SPC s g`), users also need traditional
vim-style in-file search and substitution:

- `/` — incremental search within the current buffer, highlighting all matches
- `n` / `N` — jump to next/previous match, wrapping at buffer boundaries
- `:s/pattern/replacement` — replace the current match
- `:s/pattern/replacement/g` — replace all occurrences in the file

This functionality is essential for quick local edits without leaving the file or switching to
project-wide search. The implementation should:

- Cache match positions to avoid redundant scans during navigation
- Update LSP diagnostics after replacements
- Mark the buffer as modified when changes occur
- Follow vim conventions for command syntax and keybindings

---

## Decision

### Search Mode — `Mode::InFileSearch`

A new modal state `Mode::InFileSearch` is introduced alongside existing modes (Normal, Insert,
Visual, Command, etc.). Pressing `/` in Normal mode transitions to InFileSearch and displays
an input prompt at the bottom of the screen, similar to Command mode.

**Input handling:**
- Characters are appended to `in_file_search_buffer: String` in the Editor
- `Backspace` deletes the last character
- `Enter` confirms the search, transitions back to Normal mode, and jumps to the first match
  after the cursor
- `Esc` cancels and returns to Normal mode without performing a search

### Search Storage in Buffer

The `Buffer` struct gains three new fields to track search state:

```rust
search_pattern: Option<String>           // Current search query (case-insensitive)
search_matches: Vec<(usize, usize, usize)>  // (row, col, len) of each match
current_match_idx: Option<usize>         // Index into search_matches for n/N navigation
```

**Match Caching:**  
When `set_search_pattern()` is called, the buffer scans all lines once using
`line.to_lowercase().match_indices(pattern.to_lowercase())` and stores every match position.
This cache remains valid until:
- A new search is initiated, or
- The buffer is modified (text inserted/deleted), which calls `clear_search()`.

### Navigation — `n` and `N`

Two new actions are added to the keymap:

- `InFileSearchNext` (bound to `n` in Normal mode)
- `InFileSearchPrev` (bound to `N` in Normal mode)

**Behavior:**
- `n` advances `current_match_idx`, wraps to `0` at the end, and moves the cursor to the
  match position
- `N` decrements `current_match_idx`, wraps to `matches.len() - 1` at the beginning
- Both commands trigger a scroll to ensure the match is visible in the viewport
- If no search is active (`search_pattern.is_none()`), the actions are no-ops

### Substitution Commands — `:s/find/replace[/g]`

Command mode (`:`) is extended to recognize two new patterns:

1. `:s/pattern/replacement` — single-match replace  
   Parses the command, calls `buf.replace_current(pattern, replacement)`, which:
   - Locates the first match at or after the cursor
   - Replaces the matched text using `line.replacen(pattern, replacement, 1)`
   - Updates `cursor.col` to the end of the replacement
   - Marks the buffer as modified
   - Triggers LSP `did_change` notification
   - Returns `Ok(true)` to display "1 replacement made"

2. `:s/pattern/replacement/g` — global replace  
   Calls `buf.replace_all(pattern, replacement)`, which:
   - Iterates over all lines and uses `line.replace(pattern, replacement)`
   - Counts the number of lines modified
   - Marks the buffer as modified
   - Triggers LSP notification
   - Returns `Ok(count)` to display "N replacements made"

**Edge cases:**
- If the pattern is not found, the status bar displays "Pattern not found"
- Both operations are case-insensitive (matching the search behavior)
- Search highlight cache is **not** cleared after replacement, allowing subsequent `n`/`N`
  navigation — the cache will be cleared on the next buffer edit

### UI Rendering

**Search Mode Prompt:**  
When `mode == Mode::InFileSearch`, the status bar at the bottom of the screen displays:
```
/ search_text_
```
The cursor is rendered at the end of the input field.

**Match Highlighting:**  
During normal buffer rendering, if `buf.search_pattern.is_some()`:
- All cached `search_matches` are highlighted with a distinct background color (e.g., yellow)
- The `current_match_idx` (if set) may optionally receive a different highlight (e.g., orange)
  to distinguish the active match

### Keybindings Summary

| Key | Mode | Action |
|-----|------|--------|
| `/` | Normal | `InFileSearchStart` — enter InFileSearch mode |
| `Enter` | InFileSearch | Confirm search, jump to first match, return to Normal |
| `Esc` | InFileSearch | Cancel and return to Normal |
| `n` | Normal | `InFileSearchNext` — jump to next match |
| `N` | Normal | `InFileSearchPrev` — jump to previous match |
| `:s/a/b` | Command | Replace current match |
| `:s/a/b/g` | Command | Replace all matches |

---

## Consequences

**Positive**
- Vim-familiar workflow — users can leverage muscle memory from vim/neovim
- Match caching makes `n`/`N` navigation instant, even in large files
- Substitution commands integrate cleanly with existing command mode and LSP notifications
- Search and replace are entirely in-buffer operations (no external dependencies like `rg`)
- Clear separation between in-file search (`/`) and project-wide search (`SPC s g`)

**Negative / trade-offs**
- **Case-insensitive only:** the current implementation uses `to_lowercase()` for all
  searches; vim's `\c` (ignore-case) and `\C` (match-case) flags are not supported.
- **Literal strings only:** no regex support — patterns are matched literally using
  `str::match_indices()` and `str::replace()`. Future regex support would require adding
  the `regex` crate and modifying `set_search_pattern()` and `replace_*()` to compile and
  match `Regex` objects.
- **Cache invalidation on any edit:** inserting a single character clears the entire search
  cache. For very large files, re-scanning after every edit could be expensive, though in
  practice most files are small enough that this is negligible.
- **No incremental match preview:** the search only executes on `Enter`, not on every
  keystroke. This differs from modern editors (VS Code, Neovim with `incsearch`) but
  simplifies the implementation and avoids flicker.
- **No search history:** pressing `/` always starts with an empty buffer. Storing and
  recalling previous searches (e.g., with `↑`/`↓` arrow keys) would require a history stack
  in the Editor.

**Future enhancements**
- Add `:s/pattern/replacement/c` (confirm each replacement interactively)
- Support regex patterns by integrating the `regex` crate
- Add case-sensitive search option (e.g., `/\C`)
- Implement search history navigation in InFileSearch mode
- Incremental search (jump to first match on every keystroke, not just `Enter`)

---

## Files Changed

| File | Change |
|------|--------|
| `src/keymap/mod.rs` | Added `Mode::InFileSearch`, `Action::InFileSearchStart`, `InFileSearchNext`, `InFileSearchPrev`; bound `/`, `n`, `N` |
| `src/buffer/buffer.rs` | Added `search_pattern`, `search_matches`, `current_match_idx` fields; implemented `set_search_pattern()`, `search_next()`, `search_prev()`, `replace_current()`, `replace_all()`, `clear_search()` |
| `src/editor/mod.rs` | Added `in_file_search_buffer: String`, `handle_in_file_search_mode()`, integrated substitution commands into `execute_command()` |
| `src/ui/mod.rs` | Added rendering for `Mode::InFileSearch` prompt and match highlighting |

---

## Related

- **ADR 0007** — Vim Modal Keybindings (established the Mode enum and modal architecture)
- **ADR 0008** — Normal Mode Editing Operations (movement and editing commands)
- **ADR 0024** — Project-wide Text Search (established the distinction between in-file and
  project search)
