# ADR 0020 — LazyGit Integration

**Status:** Accepted

---

## Context

The editor needed a way to interact with Git without leaving the terminal session.
Options considered:

| Option | Pros | Cons |
|--------|------|------|
| **LazyGit subprocess** | Zero code for git logic; polished full TUI; actively maintained | Requires lazygit to be installed |
| Custom git panel (git2-rs) | No external dep | Months of work; inferior UX |
| Shell out to raw git | Simple | No interactive UI; output not TUI-friendly |

LazyGit already covers staging hunks, branch management, rebasing, stash, conflict
resolution, and more. Building even a fraction of that in-editor would be a poor use of
time and would always lag behind lazygit's quality.

---

## Decision

Integrate via the **suspend/resume subprocess pattern**:

1. `SPC g g` fires `Action::GitOpen` → `Editor::open_lazygit()`
2. The editor suspends its TUI:
   - `disable_raw_mode()`
   - `execute!(LeaveAlternateScreen)` — restores the user's original terminal
   - `terminal.show_cursor()`
3. `std::process::Command::new("lazygit").status()` spawns lazygit with inherited
   stdin/stdout/stderr — lazygit takes full control of the terminal.
4. When the user quits lazygit (`q`), the process exits and we resume:
   - `enable_raw_mode()`
   - `execute!(EnterAlternateScreen)` — switches back to our alternate screen
   - `terminal.clear()` — forces ratatui to repaint every cell (removes lazygit residue)
5. Every open buffer that has a file path is reloaded from disk, because git operations
   (pull, checkout, rebase, apply patch) may have changed file content.
6. Status message: `"Returned from lazygit"` on success, or an error message if lazygit
   is not installed or crashes.

### Keybinding

| Key | Action |
|-----|--------|
| `SPC g g` | Open lazygit |

The `SPC g` which-key group is reserved for future git commands (e.g. `SPC g b` for
blame, `SPC g l` for log) if needed.

### Error handling

If `lazygit` is not on `$PATH`, `Command::new("lazygit")` returns
`ErrorKind::NotFound`. The editor catches this and displays a friendly message:
> `lazygit not found — install it (e.g. brew install lazygit)`

The TUI is always restored before showing the message so the editor remains usable.

---

## Consequences

**Positive**
- Full git TUI (stage/unstage hunks, commits, branches, rebase, stash, log, diff)
  for zero lines of git logic in forgiven.
- lazygit improvements (new features, bug fixes) are free.
- The suspend/resume pattern is well-established in terminal editors (Helix, Kakoune).

**Negative / trade-offs**
- Requires lazygit to be separately installed. Not bundled.
- No deep integration: forgiven is unaware of the current branch, dirty files, etc.
  (These could be added later via `git2` or a status-bar `git rev-parse` call.)
- On some terminal emulators the alternate-screen switch flickers briefly.
