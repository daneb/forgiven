# ADR 0141 — Companion Auto-launch and Runtime Toggle

**Date:** 2026-04-26
**Status:** Implemented

---

## Context

ADR 0139 (OpenSpec) and the Hybrid Reliability plan document a four-phase visual
system for forgiven.  Phases 2 and 3 are complete:

- **Phase 2 (Companion):** a borderless Tauri v2 window that renders Markdown and
  Mermaid diagrams received over IPC.
- **Phase 3 (Nexus):** a Unix domain socket at `/tmp/forgiven-nexus-{pid}.sock`
  that streams `buffer_update`, `cursor_move`, `mode_change`, and `shutdown`
  events from the TUI to the companion.

The remaining gap from Step 4.5 of the plan: the companion had to be launched
manually (e.g. `open Forgiven\ Previewer.app`) and there was no way to toggle it
from inside the editor.  This created two friction points:

1. Users who always want the companion had to start it by hand every session.
2. Users who sometimes wanted it had no keyboard shortcut — they had to leave the
   TUI, switch to a terminal, and run the app.

Neither config nor the `SPC` leader tree had any companion-related entry.

---

## Decision

### 1. `SidecarConfig` in `src/config/mod.rs`

A new `[sidecar]` TOML block is added to the editor config:

```toml
[sidecar]
auto_launch = false          # opt-in: spawn companion on editor startup
binary_path = "/path/to/forgiven-companion"  # omit to search $PATH
```

`SidecarConfig` is a `#[derive(Default)]` struct; all fields default to
`false`/`None` so existing configs are unaffected.

### 2. `companion_process: Option<std::process::Child>` on `Editor`

A single field stores the OS child process handle.  `std::process::Child` was
chosen over `tokio::process::Child` because:

- `kill()` is synchronous and returns immediately — no async overhead.
- The child lifecycle (spawn once, kill once) does not need tokio's full
  cancellation machinery.
- It fits the existing pattern of `Option<T>` fields for optional services.

### 3. `spawn_companion()` and `kill_companion()` helpers

```rust
fn spawn_companion(&mut self) {
    // Resolves NEXUS_SOCKET from SidecarServer::socket_path() (PID-scoped).
    // Uses config.sidecar.binary_path or falls back to "forgiven-companion" on $PATH.
    // Sets self.companion_process = Some(child) on success.
}

fn kill_companion(&mut self) {
    // Takes the child handle, calls kill(), clears the field.
}
```

The socket path is injected as `NEXUS_SOCKET` so the companion connects
immediately without the 2-second polling backoff in its auto-scan loop.

### 4. Auto-launch in `setup_services()`

After the Nexus UDS socket is bound, if `config.sidecar.auto_launch` is `true`,
`spawn_companion()` is called.  The socket is already listening at this point so
the companion will connect successfully on its first attempt.

### 5. Kill in `cleanup()`

`kill_companion()` is called in `cleanup()` after the `shutdown` Nexus event is
sent.  This is belt-and-suspenders: the companion exits on receiving `shutdown`,
but if it failed to connect (e.g. user launched it before the editor) the OS
process is still cleaned up.

### 6. `SPC p c` keybinding — `Action::CompanionToggle`

A new `SPC p` (preview) leader node is added to `build_leader_tree()`.  The
single binding `SPC p c` dispatches `Action::CompanionToggle`, which:

- If `companion_process.is_some()` → `kill_companion()`
- Otherwise → `spawn_companion()`

This is consistent with `SPC a a` (agent toggle) and `SPC e e` (explorer toggle).

---

## Alternatives considered

**Shell alias / wrapper script** — Users could alias `forgiven` to a shell
function that also starts the companion.  Rejected: couples the launch to the
shell profile rather than the editor config; no runtime toggle; no cleanup on
`:q`.

**Tauri auto-launch plugin** — The companion could register itself as a login
item via the `tauri-plugin-autostart` crate.  Rejected: wrong granularity —
auto-start at OS login is far more persistent than "start with the editor".

**Tokio process** — Using `tokio::process::Command` would allow the editor to
`await` process exit or get a `JoinHandle`.  Rejected: the companion is
fire-and-forget; we never need to await it.  The sync `std::process::Child::kill`
is simpler and fits the event-loop pattern.

**`SPC m c` under the markdown node** — The companion also renders Markdown, so
putting its toggle under `SPC m` was considered.  Rejected: `SPC m` is for
buffer-level Markdown features (preview, browser export, soft-wrap); the
companion is a process-level service.  A dedicated `SPC p` (preview) node is
clearer and leaves room for future preview-related bindings (e.g. ghost overlay
positioning in Step 6).

---

## Consequences

- Users can now add `[sidecar] auto_launch = true` to their config and the
  companion appears alongside the editor without any manual step.
- `SPC p c` gives a single keypress to show or hide the companion at any time.
- The companion is always killed on `:q`, so no orphan windows accumulate across
  sessions.
- `CONFIG.md` documents `[sidecar]` inline with all other config sections.
- The `SPC p` namespace is reserved for Step 6 (ghost overlay: `SPC p g`?).
- No behaviour change for users who do not set `[sidecar]`; the field defaults to
  `auto_launch = false` and `binary_path = None`.
