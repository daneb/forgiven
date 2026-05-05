# ADR 0147 — Drop Windows Support

**Date:** 2026-05-05
**Status:** Accepted

---

## Context

forgiven is a Unix-native TUI editor. Several architectural choices made
throughout development have hard dependencies on Unix primitives:

- **Nexus sidecar IPC** (`src/sidecar/`) uses Unix Domain Sockets
  (`tokio::net::UnixListener` / `UnixStream`). UDS is not available in the
  `tokio::net` module on Windows; the equivalent Windows primitive (named
  pipes) has a different API.
- **File permission display** (`src/editor/render.rs`) uses
  `std::os::unix::fs::PermissionsExt` for the `rwxrwxrwx` string in the file
  explorer.
- **Browser openers** (`src/editor/ai.rs`, `src/editor/mode_handlers.rs`) use
  `open` (macOS) and `xdg-open` (Linux); the Windows equivalent (`explorer`)
  was present as a dead branch with no user.
- **Socket paths** (`/tmp/forgiven-nexus-{pid}.sock`) are Unix filesystem
  conventions.

A Windows release build existed in `.github/workflows/release.yml` and was the
proximate cause of CI failures when the sidecar was introduced: the build broke
because `UnixListener` does not compile on the `x86_64-pc-windows-msvc` target.

The project has a single developer/user, who works exclusively on macOS and
Linux. There are no known Windows users, no Windows-specific feature requests,
and no plans to port the Nexus IPC layer to named pipes.

---

## Decision

Windows support is officially dropped. Specifically:

1. The `build-windows` CI job is removed from `release.yml`.
2. A `compile_error!` macro under `#[cfg(windows)]` is added to `src/main.rs`
   so that any future attempt to build on Windows produces a clear, intentional
   message rather than cryptic type errors.
3. All Windows-specific dead code is deleted:
   - Three `#[cfg(target_os = "windows")]` `"explorer"` opener branches in
     `src/editor/ai.rs`.
   - The `#[cfg(not(unix))]` permissions fallback in `src/editor/render.rs`.
   - The `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`
     attribute in `companion/src-tauri/src/main.rs`.
4. The sidecar module and all call sites that reference
   `crate::sidecar::SidecarServer` are gated with `#[cfg(unix)]` so the code
   remains structurally sound rather than relying solely on the compile_error
   guard.

Cross-platform defensive code that is harmless on Unix (backslash normalisation
in `hooks.rs` and `pickers.rs`, the `ps1`/`powershell` syntax-highlight alias)
is left in place — it imposes no cost and requires no maintenance.

---

## Consequences

**Positive**

- No more per-feature `#[cfg(unix)]` tax as new Unix primitives are introduced.
- The Windows CI job (6 min 29 s on the failing run) is removed from the
  release pipeline, cutting release build time.
- Dead code is deleted, reducing noise during future audits.

**Negative / Risks**

- A Windows user cannot build forgiven from source without patching the
  compile_error guard. This is intentional and the error message says so.
- If the project later attracts Windows users, re-introducing support will
  require porting the Nexus IPC layer to named pipes — non-trivial work.
  The `#[cfg(unix)]` gates left in place make the scope of that work visible.
