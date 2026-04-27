# Companion Diagnosis — 2026-04-25

## Problem

The companion window opens but shows "Waiting for Forgiven…" with no content, even when the Forgiven TUI is running with a file open.

## What we know

### JS layer — WORKING
The debug output added to `main.js` confirmed:
- `window.__TAURI_INTERNALS__` is present → we are inside Tauri
- `window.__TAURI__` is present (`withGlobalTauri: true` is injecting correctly)
- `window.__TAURI__.event` is present
- All four `listen()` calls register without error
- Final log line reads `listeners registered`

The frontend is correctly wired. When an event arrives it WILL be handled.

### Rust/connection layer — UNKNOWN
We added a `nexus-status` event that fires from Rust at every stage of the socket lifecycle:
- `searching… (attempt N)` — no socket found in `/tmp/`
- `connecting to <path>` — socket file found, about to connect
- `connected: <path>` — TCP handshake complete
- `connect failed: <err> — <path>` — socket found but refused
- `disconnected: <err>` — stream closed mid-session

**This status output has not yet been observed.** The build succeeded but the companion was not relaunched and tested against a live Forgiven instance before the session ended.

## Most likely root cause

The companion was tested while Forgiven was NOT running. The auto-scan (`socket_path()` in `nexus.rs`) looks for `forgiven-nexus-*.sock` in `/tmp/`. If the TUI is not open, no socket exists and the companion loops forever on `searching…`.

Secondary candidate: the companion was launched without `NEXUS_SOCKET` set and pgrep returns multiple PIDs (previous crash leftover sockets). The auto-scan picks the most-recently-modified socket — if that's a stale socket from a prior crash the connect will fail.

## Steps to diagnose tomorrow

1. Start Forgiven first:
   ```sh
   cd ~/Repos/forgiven && cargo run -- src/main.rs
   ```

2. Confirm socket exists:
   ```sh
   ls -lt /tmp/forgiven-nexus-*.sock
   ```

3. Launch the companion (no env var needed — auto-scan handles it):
   ```sh
   open "companion/src-tauri/target/debug/bundle/macos/Forgiven Previewer.app"
   ```

4. Read the status lines in the placeholder area of the companion window:
   - `searching…` → socket not found → TUI not running or socket path wrong
   - `connect failed` → socket found but TUI's accept-loop rejected it → check TUI logs
   - `connected` but no content → event routing broken (unlikely given JS is confirmed working)
   - `connected` + content appears → fixed!

5. If stuck on `searching…`, manually test the socket from a third terminal:
   ```sh
   nc -U /tmp/forgiven-nexus-$(pgrep -x forgiven | head -1).sock
   # Type in the editor — JSON lines should appear
   ```
   If nc gets JSON, the TUI side is fine and the companion's socket discovery is broken.

6. If you see `connect failed: connection refused`, the socket file exists but the TUI's accept-loop
   is not running. Check `~/.local/share/forgiven/forgiven.log` for Nexus bind errors.

## Files changed in this session (companion-side)

| File | Change |
|------|--------|
| `companion/ui/main.js` | Fixed `listen()` to use `window.__TAURI__.event.listen()` instead of broken `import('@tauri-apps/api/event')` |
| `companion/ui/main.js` | Added `dbg()` helper that appends to placeholder + console.log |
| `companion/ui/main.js` | Added `nexus-status` listener to display connection state |
| `companion/src-tauri/src/nexus.rs` | Added `app.emit("nexus-status", …)` at every lifecycle point |

## If the diagnosis points to the JS event system

Try emitting a test event from the Rust `setup()` closure immediately on startup:
```rust
// in lib.rs setup closure, after nexus::spawn():
let _ = app.handle().emit("nexus-status", "setup complete");
```
If the placeholder shows `status: setup complete` the full pipeline works and the only missing piece is a live socket connection.

## If the diagnosis points to socket discovery

Override auto-scan with an explicit path:
```sh
NEXUS_SOCKET=/tmp/forgiven-nexus-$(pgrep -x forgiven | head -1).sock \
  open "companion/src-tauri/target/debug/bundle/macos/Forgiven Previewer.app"
```
