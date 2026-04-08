# ADR 0122 — No duplicate file buffers

## Status

Accepted

## Context

A user could open the same file multiple times (e.g. via `:e`, the file picker, or the buffer picker). Each call to `open_file` pushed a new `Buffer` onto the buffer list without checking whether a buffer for that path already existed.

This caused a confusing UX bug: saving one copy of the buffer did not mark the other copy as clean, so `:q` would still warn about unsaved changes even after the user had explicitly saved.

## Decision

The same absolute file path must not appear in more than one buffer simultaneously. When `open_file` is called for a path that is already open, the editor switches to the existing buffer instead of creating a new one.

Paths are compared after canonicalisation (`std::fs::canonicalize`) to handle relative vs absolute paths and symlinks. For paths that do not yet exist on disk (new-file workflow), the raw path is used as a fallback.

## Consequences

- Opening a file that is already in the buffer list is a no-op that simply switches focus — no silent duplicate is created.
- The unsaved-changes-on-quit bug described above is eliminated.
- Users who intentionally want two independent copies of the same content must create a new scratch buffer and paste manually (an acceptable trade-off; the common case is accidental duplication).
