# ADR 0148 — Companion: Plain Binary Distribution

**Date:** 2026-05-07
**Status:** Accepted

---

## Context

The DMG produced by the release workflow contained two items:

- `forgiven` — the editor universal binary
- `Forgiven Previewer.app` — the companion Tauri app bundle

`resolve_companion_binary()` in `src/editor/mod.rs` searches for the companion
binary in three steps:

1. Explicit config override (`sidecar.binary_path`).
2. `forgiven-companion` in the **same directory** as the running `forgiven` binary.
3. `$PATH` fallback.

When a user dragged both items from the DMG into `/Applications`, step 2 failed
because the companion binary lives inside the `.app` bundle at
`Forgiven Previewer.app/Contents/MacOS/forgiven-companion`, not at
`/Applications/forgiven-companion`. Step 3 also failed because neither
`/Applications` nor the interior of an `.app` bundle is on `$PATH`.

The result: the companion silently failed to launch. The workaround was to set
`sidecar.binary_path` to the full path inside the `.app` bundle — poor UX.

---

## Decision

The companion is distributed as a **plain binary** (`forgiven-companion`)
alongside `forgiven` in the DMG, not wrapped in a `.app` bundle.

Changes:

1. **Release workflow** (`.github/workflows/release.yml`): the companion build
   step switches from `npm run build -- --bundles app` (which produces the `.app`
   bundle) to `cargo build --release` (which produces the raw binary). The DMG
   staging now copies `forgiven-companion` directly instead of the `.app` bundle.
2. **Resolution**: after this change, step 2 of `resolve_companion_binary()`
   succeeds because both binaries land in the same directory regardless of where
   the user installs them.
3. **`make install`** already copies the raw `forgiven-companion` binary to
   `~/.local/bin/` — no change needed there.

The frontend (HTML/JS/CSS) is still embedded at compile time by
`tauri_build::build()` via the `frontendDist: "../ui"` config pointer. The Tauri
binary runs fine without its `.app` wrapper — it creates a GUI window when
spawned.

---

## Consequences

**Positive**

- The companion launches correctly regardless of where the user places the
  binaries (drag to `/Applications`, `~/.local/bin`, or anywhere else).
- No `.app` bundle means a simpler DMG layout — two files, no nested structure.
- `binary_path` config override is no longer needed for the common case.
- The `make install` and DMG distribution paths now behave identically.

**Negative / Risks**

- Without an `.app` wrapper, the companion has no Dock icon, no Launch Services
  registration, and the menu bar shows the raw process name. These are acceptable
  trade-offs for an auxiliary window spawned by the editor.
- If the companion ever needs to be a standalone, launchable application (e.g.
  double-click from Finder), the `.app` bundle would need to return. The
  packaging change is trivial to reverse — it only affects the CI workflow.
