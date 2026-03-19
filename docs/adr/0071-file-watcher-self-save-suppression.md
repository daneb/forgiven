# ADR 0071 — File Watcher Self-Save Suppression

**Date:** 2026-03-18
**Status:** Accepted

---

## Context

ADR 0064 introduced a filesystem watcher (`notify = "7"`) that monitors all open
buffers for external changes. When a `Modify` or `Create` event is received for an
open buffer's path, the run loop either:

- reloads the file silently (if the buffer has no unsaved edits), or
- shows `⚠ external change to 'file' (unsaved — :e! to reload)` (if the buffer is
  modified).

A false-positive case was discovered: **saving the file from within forgiven
triggers the watcher**. The OS emits a `Modify` event for the file immediately
after `std::fs::write` completes. This event is queued in the `mpsc` channel and
processed on the next event-loop tick. If the buffer happens to have `is_modified =
true` at that moment (e.g. a concurrent external modification had been enqueued
before the save cleared the flag, or the OS event arrives during the same tick as
the save), the warning fires for a save the user initiated themselves.

In practice this manifested most frequently with files in cloud-synced directories
(iCloud Drive, Dropbox) where sync clients may also touch the file immediately
after a write, or with rapid edit-save cycles where the watcher event arrives in
the same batch as the save action.

---

## Decision

Track paths written by the editor itself and suppress watcher events for those
paths within a 500 ms window.

### Data structure

A `self_saved: HashMap<PathBuf, Instant>` field is added to `Editor`. It records
the canonical path and the timestamp of every file the editor writes to disk.

### Recording saves

The three save sites in `src/editor/mod.rs` are updated to insert into `self_saved`
immediately after a successful `buf.save()`:

| Action | Handler |
|--------|---------|
| `Action::FileSave` (`SPC f s`) | `self.self_saved.insert(path, Instant::now())` |
| `:w` / `:write` | same |
| `:wq` | same |

To satisfy the borrow checker, the file path is extracted from the buffer before
the `self.self_saved.insert()` call (the buffer's mutable borrow must be released
first).

### Suppression in the watcher handler

At the start of each run-loop tick, stale entries are pruned:

```rust
let suppress_window = Duration::from_millis(500);
self.self_saved.retain(|_, t| t.elapsed() < suppress_window);
```

When draining the watcher channel, each event path is canonicalized and checked
against the (also canonicalized) keys in `self_saved`. Events matching a recently
self-saved path are silently dropped before reaching the buffer-update logic:

```rust
let self_saved = self.self_saved.keys().any(|saved| {
    saved.canonicalize().unwrap_or_else(|_| saved.clone()) == canonical
});
if !self_saved {
    paths.push(p);
}
```

### Why 500 ms?

The OS typically delivers filesystem notifications within a few milliseconds of the
write completing. 500 ms is more than sufficient to absorb any notification latency
while being short enough that genuine external changes made immediately after a save
are still caught on the next cycle.

---

## Consequences

- Saving a file from within forgiven no longer triggers the "external change"
  warning or a silent reload.
- Genuine external changes (another process modifying the file while you have
  unsaved edits) still trigger the warning correctly — the suppression window is
  path-and-time specific, so unrelated writes are unaffected.
- `self_saved` has at most one entry per open buffer and is pruned every tick;
  the memory and CPU overhead is negligible.
- Canonicalization failures (e.g. the file no longer exists by the time the
  check runs) fall back to the raw path, which is the same behaviour as the
  existing watcher canonicalization code.

### Files changed

| File | Change |
|------|--------|
| `src/editor/mod.rs` | `self_saved` field on `Editor`; three save sites updated; watcher handler filters self-saves and prunes stale entries |

---

## Related ADRs

- **ADR 0064** — Filesystem watcher (introduced the watcher and reload logic)
