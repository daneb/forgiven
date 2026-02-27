# ADR 0032 — Recent Files in the Find File Picker

**Status:** Accepted

---

## Context

The Find File picker (`SPC f f` → `Mode::PickFile`) scanned all project files and presented
them alphabetically with fuzzy search. Every session started from scratch: there was no
memory of which files had been opened previously.

In practice the majority of navigations are to a small set of recently-touched files.
Forcing the user to type a query every time — even for files they opened moments ago —
added unnecessary friction, particularly when re-visiting `/etc/hosts` or similar paths
that sit outside a project tree and therefore don't appear in the scan at all.

A secondary issue surfaced when the copy-paste regression fix (`EnableBracketedPaste`) was
added: opening system files via the picker exposed the lack of any recency signal.

---

## Decision

### Project-scoped recent files list

A `recent_files: Vec<PathBuf>` field is added to `Editor`. It holds at most **5** entries,
most-recent first, scoped to the current working directory. Entries are stored as canonical
absolute paths so deduplication is reliable across relative/absolute open calls.

Scoping to `cwd` keeps the list meaningful: opening files in project A does not pollute
the picker when the editor is next launched from project B.

### Persistence

Recents are written to `~/.local/share/forgiven/recent_files.txt` as a newline-separated
list of absolute paths. The file is created on first open (including its parent directory).

The format is intentionally plain text — no JSON, no serde. It is human-readable,
trivially editable, and requires no extra dependencies.

On startup, `load_recents()` reads the file, drops any paths that no longer exist on disk,
and caps the result at 5. If the file is absent or unreadable the editor starts with an
empty list rather than failing.

### Recording opens

`open_file()` canonicalises the path after a successful buffer push, removes any existing
entry for that path, prepends it to `recent_files`, truncates to 5, then calls
`save_recents()`. Failures to save are silently swallowed — persistence is best-effort and
must never block an open.

### Sentinel-based section layout

When the picker query is empty, `refilter_files()` injects two synthetic sentinel entries
into `file_list` (`Vec<(PathBuf, Vec<usize>)>`) rather than changing the type:

| Sentinel | Value | Rendered as |
|---|---|---|
| Header | `PathBuf::new()` (empty) | `─── Recent ───` cyan bold on dark tinted bg |
| Footer | `PathBuf::from("\x01")` | `────────────────` dimmer divider line |

Recent files (filtered to `cwd`) appear between the two sentinels. All project files
follow, deduplicated against the recent list so no file appears twice.

When a query is typed, the normal fuzzy-filter path runs unchanged — sentinels are never
injected, and the Recent section disappears entirely.

The helper `is_picker_sentinel(path)` centralises sentinel detection and is called from
navigation, Enter handling, and the file count display.

### Navigation and selection

Up/Down arrows skip sentinel entries using a `while`-loop that advances until a real path
is found. Enter is guarded by the same check so pressing Enter on a divider row is a no-op.
The initial cursor position is set to index 1 (the first recent file) when recents are
present, 0 otherwise.

### Rendering

`render_file_picker()` in `src/ui/mod.rs` handles the two sentinel cases at the top of the
results loop with `continue`, keeping the existing per-file render logic untouched.

The header uses `Color::Cyan` bold on `Color::Rgb(20, 35, 50)` to match the picker's
`LightCyan` border and remain legible on transparent/dark terminal backgrounds. The footer
uses a muted `Color::Rgb(30, 80, 110)` on the same background tint so it closes the
section without competing visually with the header.

The file count shown in the query box title excludes sentinels:
```rust
files.iter().filter(|(p, _)| !p.as_os_str().is_empty()).count()
```

---

## Consequences

**Positive**
- The most common navigations (re-open a recent file) require zero keystrokes beyond
  `SPC f f` + `Enter`
- Recents persist across sessions without any user configuration
- Project-scoping keeps the list focused and avoids cross-project noise
- The `FileList` type alias and all downstream render/nav code are unchanged — sentinels
  are transparent to anything that doesn't explicitly check for them
- No new dependencies; plain-text persistence is human-editable

**Negative / trade-offs**
- Using sentinel values inside a `Vec<(PathBuf, Vec<usize>)>` is an implicit convention.
  Any future code that iterates `file_list` without using `is_picker_sentinel()` will
  encounter unexpected empty/`\x01` paths. A typed enum wrapper would be cleaner but
  would require changing the `FileList` type alias and every call site.
- Recent files are not surfaced when a query is active. Boosting recent-file scores in the
  fuzzy ranking is a possible future improvement.
- The persistence path (`~/.local/share/forgiven/`) is Unix-specific. Windows support
  would require `%APPDATA%` or a cross-platform dirs crate.

**Future enhancements**
- Boost recent files in fuzzy score when a query is typed
- Show a timestamp or relative age alongside each recent entry (e.g. `2h ago`)
- Per-project persistence keyed by a hash of the project root, allowing global storage
  with per-project retrieval when `cwd` changes inside a running session

---

## Files Changed

| File | Change |
|---|---|
| `src/editor/mod.rs` | `recent_files` field; `load_recents()`, `save_recents()`, `recents_path()`, `is_picker_sentinel()` helpers; `open_file()` recording; `refilter_files()` sentinel injection and `cwd` filtering; navigation sentinel skipping |
| `src/ui/mod.rs` | Sentinel rendering in `render_file_picker()`; file count excludes sentinels |

---

## Related

- **ADR 0010** — File Explorer Tree Sidebar (companion file-navigation feature)
- **ADR 0024** — Project-Wide Text Search (same `Mode`-based overlay pattern)
