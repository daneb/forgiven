# ADR 0064 — Filesystem Watcher: External Change Detection and Auto-Reload

**Date:** 2026-03-15
**Status:** Accepted

---

## Context

Forgiven already had a narrow auto-reload path: when the built-in Copilot agent wrote a file via its `write_file` / `edit_file` tools, it emitted a `StreamEvent::FileModified` event which the editor processed to reload the corresponding open buffer. This worked only for the agent's own tool calls.

Any other external write — a shell command, an MCP server writing directly to disk, another editor, or a separate AI process — went undetected. The open buffer would silently diverge from the on-disk content until the user manually reloaded (`:e`).

The `notify = "7"` crate was already present in `Cargo.toml` but completely unused.

---

## Decision

### Watcher initialisation (`src/editor/mod.rs`)

- Two new fields added to `Editor`:
  - `file_watcher: Option<RecommendedWatcher>` — the notify watcher handle (must be kept alive).
  - `watcher_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>` — the receiving end of the watcher's sync channel.
- `Editor::new()` was refactored from a single `Ok(Self { ... })` return into a `let mut editor = Self { ... };` pattern, allowing the watcher to be initialised after construction and stored back on the struct.
- Watcher startup is best-effort: failure emits a `WARN` log and leaves both fields `None`; the rest of the editor is unaffected.

### Watch registration (`open_file()`)

After a buffer is pushed to `self.buffers`, if the buffer has a `file_path`, it is registered with `watcher.watch(path, RecursiveMode::NonRecursive)`. This covers both files opened from the Explorer and files passed on the command line.

### Event polling (`run()` loop)

A new poll block was added to the main 50 ms tick loop, immediately after the existing `pending_reloads` drain. It:

1. Drains all pending events from `watcher_rx` using `try_recv()`.
2. Filters for `EventKind::Modify(_)` and `EventKind::Create(_)` (the latter catches write-via-rename patterns used by many editors and tools).
3. Canonicalizes each changed path and matches it against open buffers.
4. For each matching buffer:
   - **Not modified** → calls `buf.reload_from_disk()` and sets status `↺ name reloaded`.
   - **Has unsaved edits** → shows `⚠ external change to 'name' (unsaved — :e! to reload)` without touching the buffer, preserving the user's work.

### Unwatch on close

Both `Action::BufferClose` and `Action::BufferForceClose` call `watcher.unwatch(path)` on the closed buffer's file path, keeping watched paths in sync with open buffers.

---

## Consequences

- External changes to open buffers (from any source) are reflected within one 50 ms tick.
- Unsaved edits are never silently clobbered.
- The watcher degrades gracefully on platforms or environments where `notify` fails to initialise.
- No new dependencies — `notify = "7"` was already declared.
