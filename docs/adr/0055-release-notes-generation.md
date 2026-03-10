# ADR 0055 — Release Notes Generation

**Date:** 2026-03-10
**Status:** Accepted

---

## Context

After merging work, users need to write release notes for changelogs, GitHub releases, or project updates. Pulling commit messages out of LazyGit or the terminal for this is manual and error-prone. The editor already has AI one-shot completion (introduced for commit messages in ADR 0047); the same mechanism can summarise a range of commits into structured release notes.

The user should be able to specify how many commits to include (default 10), and the output should be formatted markdown grouped by change type.

---

## Decision

### Keybinding

| Key       | Action                                        |
|-----------|-----------------------------------------------|
| `SPC g n` | Generate AI release notes from last N commits |

### Flow

1. `SPC g n` → editor enters `Mode::ReleaseNotes` (count-entry phase).
2. A popup shows `Commits to include: 10_`. The user types a number (1–200) and presses `Enter`.
3. `git log --format=%H%n%s%n%b%n--- -N` is run synchronously to capture hashes, subjects, and bodies.
4. A `tokio::spawn` task calls `acquire_copilot_token()` then `one_shot_complete()` (1024 max tokens) in the background.
5. The popup transitions to a "generating" phase; `Esc` cancels.
6. On completion the popup shows the formatted release notes (scroll with `j`/`k`).
7. `y` copies the full text to the system clipboard; `Esc`/`q` closes.

### Three-phase single mode

`Mode::ReleaseNotes` covers all three phases, distinguished at runtime:

| Condition | Phase |
|---|---|
| `release_notes_rx.is_none() && release_notes_buffer.is_empty()` | 1 — count entry |
| `release_notes_rx.is_some()` | 2 — generating |
| `release_notes_rx.is_none() && !release_notes_buffer.is_empty()` | 3 — displaying |

### AI prompt

```
System: You are a technical writer creating release notes for a software project.
        Given a list of git commits, produce clean, user-friendly release notes in
        markdown. Group related changes under headings like '### Features',
        '### Bug Fixes', '### Improvements'. Be concise but descriptive. Omit merge
        commits and trivial chore changes. Output markdown only.

User:   Generate release notes from these {count} commits:
        ```
        {git log output}
        ```
```

`max_tokens` raised to 1024 (vs 256 for commit messages) to accommodate longer output. The `one_shot_complete` function gained a `max_tokens: u32` parameter; all call sites updated.

---

## Implementation

| File | Change |
|---|---|
| `src/keymap/mod.rs` | Added `Mode::ReleaseNotes`; `Action::GitReleaseNotes`; `SPC g n` binding |
| `src/agent/mod.rs` | Added `max_tokens: u32` parameter to `one_shot_complete` |
| `src/editor/mod.rs` | State fields: `release_notes_count_input`, `release_notes_rx`, `release_notes_buffer`, `release_notes_scroll`; methods: `start_release_notes()`, `trigger_release_notes_generation()`, `handle_release_notes_mode()`; run-loop polling; needs-render guard |
| `src/ui/mod.rs` | Added `ReleaseNotesView` struct; `release_notes: Option<&ReleaseNotesView>` param on `UI::render`; `render_release_notes_popup()`; status-bar label `"RELEASE"` / colour `LightCyan` |

No new dependencies.

---

## Consequences

- **Positive**: Release notes can be drafted in seconds from inside the editor without switching context.
- **Positive**: The count-input phase gives the user explicit control over the scope of the notes.
- **Positive**: Clipboard copy (`y`) makes it trivial to paste the notes into GitHub, a CHANGELOG, or a Slack message.
- **Positive**: Uses the same background-task + oneshot-channel pattern as ADR 0047, keeping the main thread non-blocking.
- **Negative**: Quality depends on commit message discipline — terse or cryptic commit subjects produce poor release notes.
- **Negative**: `git log --format=%H%n%s%n%b%n---` omits merge-commit context; for projects using merge-only workflows the diff itself would be more informative (not implemented).
