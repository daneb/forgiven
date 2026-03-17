# ADR 0065 — Terminal Redraw on Resume (Resize, SIGCONT, Ctrl+L)

**Date:** 2026-03-16
**Status:** Accepted

---

## Context

After suspending a laptop (lid close) and resuming, forgiven's terminal display was corrupt: text was invisible or incomplete until the user navigated around enough to trigger incremental repaints. The root cause is that the terminal emulator invalidates its cell buffer on suspend/resume, but the editor had no mechanism to detect this and issue a full repaint.

Three distinct triggers can cause the terminal cell grid to go stale:

1. **Process resume (SIGCONT)** — the OS sends `SIGCONT` when a suspended process is continued (laptop wake, `fg` after `Ctrl+Z`). The terminal has already discarded its buffer contents, so everything must be repainted from scratch.
2. **Terminal resize (SIGWINCH / `Event::Resize`)** — some terminal emulators and multiplexers (iTerm2, tmux, kitty) emit a resize event on resume even when dimensions haven't changed. The existing event match had a `_ => {}` catch-all that silently discarded these.
3. **User-initiated (Ctrl+L)** — the universal terminal convention for "force redraw". An essential escape hatch for any case the automatic detection misses.

The run loop previously called `ratatui`'s incremental `render()` without ever issuing a full `terminal.clear()`, so stale cells from before the suspend were never overwritten.

---

## Decision

### `force_clear` flag

A `bool` local variable `force_clear` was added to `run()`. When set, a `self.terminal.clear()` call is issued immediately before the next `self.render()` call, then the flag is cleared. This ensures the full cell grid is repainted rather than only the cells that changed since the last frame.

### SIGCONT handler (`src/editor/mod.rs`)

A background tokio task is spawned once at the start of `run()`, gated on `#[cfg(unix)]` so Windows is unaffected. It listens on `tokio::signal::unix::signal(SignalKind::from_raw(18))` (SIGCONT = 18 on Linux and macOS) and forwards a `()` message over a `tokio::sync::mpsc::unbounded_channel` for each signal received. The run loop drains this channel with `try_recv()` each tick and sets `force_clear = true` on any message. No new dependency is required — `tokio::signal::unix` is already available via `tokio = { features = ["full"] }`.

### `Event::Resize` handler

A new arm was added to the crossterm event match:

```rust
Event::Resize(_, _) => {
    force_clear = true;
    needs_render = true;
},
```

This handles the common case where the terminal emulator *does* emit a resize event on resume, as well as genuine window resizes, which previously also suffered from stale-cell artefacts.

### Ctrl+L force-redraw

`Ctrl+L` is intercepted in the event match before `handle_key` is called, so it works identically in every mode (Normal, Insert, Agent, Command, etc.):

```rust
if key.code == KeyCode::Char('l') && key.modifiers == KeyModifiers::CONTROL {
    force_clear = true;
} else {
    self.handle_key(key)?;
}
needs_render = true;
```

---

## Consequences

- Forgiven automatically repaints fully on laptop wake without any user interaction.
- Terminal resize events are handled correctly; ratatui lays out to the new dimensions on the same tick.
- Ctrl+L provides a reliable manual escape hatch consistent with vim, htop, and other TUI conventions.
- The SIGCONT listener degrades silently if signal registration fails (the `if let Ok(...)` guard), leaving the rest of the editor unaffected.
- Windows builds are unaffected — the SIGCONT path is entirely `#[cfg(unix)]`; `Event::Resize` and Ctrl+L work on all platforms.
- No new dependencies.
