# ADR 0035 — Agent Apply-Diff Overlay

**Status:** Accepted

---

## Context

Pressing `a` in Agent mode (when the input was empty) previously applied the first
code block from the latest Copilot reply by inserting it at the cursor position in
whatever buffer happened to be active. This was broken in two independent ways:

1. **Wrong file** — the block landed in the active buffer regardless of which file the
   agent was discussing. If the agent explained changes to `src/foo.rs` but the user
   had `README.md` open, the code would be inserted there.
2. **Wrong location** — insertion at cursor left the original file content intact above
   and below; the intent is almost always a *replacement* of the full file.

A minimal fix would be to silently replace the correct file, but that is equally
dangerous because users lose their ability to review what changes before committing
to them.

---

## Decision

### `Mode::ApplyDiff`

A new editor mode, `Mode::ApplyDiff`, shows a full-screen scrollable diff overlay
before any file is touched. The user confirms (`y` / `Enter`) or discards
(`n` / `Esc`). No change is made until explicit confirmation.

### File resolution (`extract_first_code_block_with_path`)

A new free function in `src/agent/mod.rs` returns both the code content *and* a
path hint extracted from the agent reply:

1. **Fence info string** — tokens in the opening fence that contain `/` or `\` (and
   are not URLs) are treated as a relative path hint.  Example:
   ` ```rust src/editor/mod.rs ` → hint is `src/editor/mod.rs`.
2. **Preceding prose** — up to 3 lines before the fence are scanned for
   `` `backtick-quoted/paths` ``.  Example:
   `"Update \`src/lib.rs\` as follows:"` → hint is `src/lib.rs`.

The `get_apply_candidate()` method on `AgentPanel` calls this function on the most
recent assistant message.

### Path resolution order (in `handle_agent_mode`)

When the user presses `Ctrl+A`:

1. If a path hint was found, it is joined with `cwd` to form an absolute path.
   The current content is read from:
   a. An already-open buffer (by canonicalised path comparison), or
   b. The file on disk (if it exists), or
   c. An empty string (new file — diff will be all-green).
2. If no path hint was found, the active buffer is used as the target.

### LCS diff (`lcs_diff`)

A module-level function in `src/editor/mod.rs` computes a line-level diff between
the current file content and the proposed code block using the Longest Common
Subsequence algorithm. Output is a `Vec<DiffLine>` where each element is one of:

```rust
pub enum DiffLine {
    Context(String),   // unchanged line (shown in dark-gray)
    Added(String),     // new line      (shown in green, prefixed "+")
    Removed(String),   // deleted line  (shown in red,   prefixed "-")
}
```

For very large inputs (> 2 000 lines on either side) the algorithm falls back to
all-removed / all-added to avoid quadratic memory usage.

### Overlay renderer (`render_apply_diff_overlay`)

The overlay is full-screen. It consists of:

- A 3-row header block (cyan border) with the target file path and keyboard hints.
- A body that renders the diff lines in colour, offset by `apply_diff_scroll`.
- A scroll indicator (`n/total`) in the bottom-right corner when the diff exceeds
  the visible area.

The status-bar mode indicator shows `DIFF` in cyan.

### Apply logic (`do_apply_diff`)

On confirmation (`y` / `Enter`):

- If a path was resolved: the proposed code is written to disk (appending a trailing
  newline if absent), any open buffer for that path is reloaded from disk via
  `Buffer::reload_from_disk()`, and parent directories are created if they do not
  exist.
- If no path (unsaved buffer): `Buffer::replace_all_lines()` replaces the buffer
  content in-memory; the buffer is marked modified.

### `Buffer::replace_all_lines`

A new method on `Buffer` (in `src/buffer/buffer.rs`) replaces the buffer's line
list in-memory without touching the file. It increments `lsp_version`, marks the
buffer modified, and clamps the cursor.

---

## Alternatives considered

**Silent auto-apply (no diff, no confirmation)**
Rejected. Applying an AI-generated change without review is risky. Even a small
hallucination in the code block could corrupt the target file.

**Inline diff inside the editor pane**
Side-by-side or inline editor diffs would require significant layout work and a
complex merge UI. A full-screen terminal diff overlay is sufficient for review and
far simpler to implement.

**Re-using the inline-edit diff overlay (ADR 0034)**
The `InlineEdit` overlay (from the commit message / inline edit features) shows a
diff for a sub-range of lines. Apply-diff always replaces the *entire* file, making
it a different concept. A dedicated mode avoids coupling the two flows.

---

## Consequences

**Positive**
- The correct file is always targeted, even when a different buffer is active.
- The user sees exactly what will change before committing.
- New files (not yet on disk) are handled — the diff shows all lines as added.
- Files not currently open in any buffer are handled — content is read from disk.
- Parent directories are created automatically if the agent proposes a new file in
  a new directory.
- The `Buffer::replace_all_lines` method is reusable for other in-memory replacement
  operations.

**Negative / trade-offs**
- One extra keypress (`y`) is required to apply; this is intentional.
- The LCS algorithm is O(m×n) in time and space. The 2 000-line cap keeps worst-case
  memory bounded at ~16 MB (2 000 × 2 000 × 4 bytes).
- `Mode::ApplyDiff` adds a 14th mode to the mode graph.

---

## Mode graph addition

```
Agent    ── Ctrl+A (code block present) ──► ApplyDiff
ApplyDiff ── y / Enter ──► Normal  (change applied)
          ── n / Esc   ──► Agent   (discarded)
          ── j / k     ──► (scroll down / up one line)
          ── Ctrl+D/U  ──► (scroll down / up half-page)
```

---

## Amendment — 2026-03-13

The trigger key was changed from `a` (bare key, empty input only) to `Ctrl+A`
(modifier chord, active at any time).

**Reason:** bare single-letter shortcuts that only fire on an empty input box
intercept the first character of any new message starting with that letter
(e.g. "add a test", "are you sure"). Moving to `Ctrl+A` eliminates the
accidental-trigger class of bugs while keeping a single-chord shortcut.
`Ctrl+Y` (yank full reply) and `Ctrl+K` (copy code block) were migrated for
the same reason; see the amendment in ADR 0041.
