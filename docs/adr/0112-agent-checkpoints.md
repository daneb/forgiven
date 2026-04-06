# ADR 0112 — Agent Checkpoints / Session Undo

## Status
Accepted

## Context

The agentic loop can make many file edits across a session. If the result is
unsatisfactory (wrong approach, broken compilation, unwanted changes) the user
currently has no single-command way to undo everything the agent did this
session. They must either use `git checkout` manually or undo each file
individually.

Competitors (Cursor, Windsurf, Zed) all offer some form of session-level revert.

## Decision

### Snapshot strategy

Before the agentic loop executes the **first** `write_file` or `edit_file` for
any given project-relative path in a session, read the current content of that
file from disk and emit a `StreamEvent::FileSnapshot { path, original }` event.

The panel's `poll_stream()` stores this in
`AgentPanel::session_snapshots: HashMap<String, String>` using
`entry().or_insert()` so only the pre-agent state (not a later intermediate
state) is kept.

### Revert

`AgentPanel::revert_session(project_root)`:
- Iterates `session_snapshots`, writing each original back to disk.
- Returns the list of restored paths so the editor can queue them as
  `pending_reloads` (identical mechanism to normal agent edits).
- Clears `session_snapshots` after restoration.

`AgentPanel::has_checkpoint()` returns `true` when the map is non-empty,
allowing the action handler to emit a meaningful "no checkpoint" status message
instead of silently doing nothing.

### Keybinding

`SPC a u` → `Action::AgentSessionRevert`

Mnemonic: **u**ndo the agent session.

### Lifecycle

- `new_conversation()` clears `session_snapshots`, so starting a new
  conversation resets the checkpoint.
- Snapshots accumulate across multiple `submit()` rounds within a single
  session; only the original pre-session content is kept per file.
- Revert after revert: after `SPC a u` the snapshot map is empty; a subsequent
  `SPC a u` shows "No checkpoint".

### Newly created files

When `write_file` targets a path that did not exist before the session, a
`StreamEvent::FileCreated { path }` event is emitted instead of `FileSnapshot`.
`AgentPanel` stores these in `session_created_files: Vec<String>`.

`revert_session()` deletes these files (`std::fs::remove_file`) after restoring
the snapshots. `has_checkpoint()` returns `true` when either `session_snapshots`
or `session_created_files` is non-empty.

The status message after revert distinguishes the two cases:
`"Session reverted: 3 files restored, 1 new file deleted"`.

### What is NOT covered

- Binary files. `std::fs::read_to_string` returns an empty string on UTF-8
  errors — binary files are treated as if they had no prior content and will be
  truncated on revert. This is acceptable for a code editor.
- Files outside the project root. `safe_path` already rejects `..` traversal
  in tool execution so these can never appear in snapshots.

## Alternatives considered

### `git stash` before first tool call

Pro: handles new files, binary files, submodules.
Con: requires git (not always present), stashes pollute the stash list, async
interaction with the git process adds latency and error surface at the hot path.
The in-memory approach is simpler and sufficient for the common case.

### Per-round snapshot (before every tool call)

Would allow per-round granular undo rather than full-session undo.
Deferred — session-level undo satisfies the immediate requirement and the UX
for partial revert is unclear.

## Consequences

- Every `write_file`/`edit_file` tool call now reads the file from disk once
  before executing (if it hasn't been snapshotted yet). This adds one syscall
  per unique file per session — negligible cost.
- `AgentPanel` grows two fields: `session_snapshots: HashMap<String, String>`
  and `session_created_files: Vec<String>`. Memory is bounded by the number and
  size of files touched in the session.
- `SPC a u` is now the single-command "undo everything the agent did" escape
  hatch — both modifications and new creations — visible under `SPC a`.
