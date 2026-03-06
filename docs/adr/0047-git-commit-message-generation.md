# ADR 0046: Git Commit Message Generation

**Date:** 2026-03-06
**Status:** Accepted

## Context

Users frequently need to write commit messages after making changes. Writing a good commit message (imperative subject line, concise body) takes effort and is easy to skip. The editor already has Copilot integration via the agent panel; extending that to generate commit messages is a natural fit.

The feature should be standalone — no dependency on LazyGit or any external tool — and the user must be able to review and edit the generated message before it is committed.

Two source modes are needed:

- **Staged diff** (`SPC g s`) — useful when preparing a new commit
- **Last commit** (`SPC g l`) — useful for amending message quality or copying commit style

## Decision

### Keybindings

| Key       | Action                              |
|-----------|-------------------------------------|
| `SPC g s` | Generate commit message from staged diff (`git diff --staged`) |
| `SPC g l` | Generate commit message from last commit (`git show HEAD --stat -p`) |

### Flow

1. User presses `SPC g s` or `SPC g l`.
2. The diff is captured synchronously (fast shell command).
3. If the diff is empty, a status message is shown and the popup does not open.
4. The editor enters `Mode::CommitMsg` and shows a "Generating…" status.
5. A `tokio::spawn` task calls `acquire_copilot_token()` then `one_shot_complete()` in the background.
6. The main loop polls `commit_msg_rx` each tick; on receipt the popup is populated.
7. The user edits the message freely; **Enter** commits (`git commit -m <msg>`), **Esc** discards.

### New code

- **`src/agent/mod.rs`**
  - `pub async fn acquire_copilot_token() -> Result<String>` — loads OAuth token and exchanges it, usable without an `AgentPanel` reference.
  - `pub async fn one_shot_complete(api_token, model_id, system, user) -> Result<String>` — non-streaming single-turn Copilot call.

- **`src/keymap/mod.rs`**
  - `Mode::CommitMsg` added to the `Mode` enum.
  - `Action::GitCommitStaged` and `Action::GitCommitLast` added to the `Action` enum.
  - Keybindings registered under the `SPC g` leader.

- **`src/editor/mod.rs`**
  - State fields: `commit_msg_buffer: String`, `commit_msg_rx: Option<oneshot::Receiver<Result<String>>>`, `commit_msg_from_staged: bool`.
  - `start_commit_msg(from_staged: bool)` — kicks off the background task.
  - `handle_commit_msg_mode(key)` — handles Esc, Enter, Backspace, Char input.
  - `commit_msg_rx` polled in the main run loop (same pattern as `search_rx`).

- **`src/ui/mod.rs`**
  - `UI::render` gains a `commit_msg: Option<&str>` parameter.
  - `render_commit_msg_popup` — centred popup (80 cols wide, 4–12 content lines) with a hint line and `LightYellow` border.
  - Status bar: `Mode::CommitMsg` → label `"COMMIT"`, colour `LightYellow`.

## Consequences

- Users can generate a quality commit message in one keypress without leaving the editor.
- The Copilot token is acquired lazily inside the background task, so there is no blocking on the main thread.
- The git diff is captured synchronously because it is always fast (< 10 ms in practice); for very large repos this could be revisited.
- The popup is deliberately plain text (no markdown rendering) so multi-line editing stays predictable.
