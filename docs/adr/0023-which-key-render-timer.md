# ADR 0023 — Which-Key Popup Render Timer

**Status:** Accepted

---

## Context

Pressing `SPC` starts a leader-key sequence and starts a 500 ms timer.  Once the
timer fires, `should_show_which_key()` returns `true` and the which-key popup is
rendered — showing the available bindings for the current prefix.

The popup worked on the very first `SPC` press after launch, then silently stopped
appearing on every subsequent press.

---

## Root Cause

The render loop gates all drawing behind a `needs_render` flag to avoid burning CPU
on idle frames (ADR-0021).  After `SPC` is pressed, one render fires immediately
(from the key-event path), but then `needs_render` reverts to `false`.  The loop
polls for events with a 50 ms timeout; with no further key presses and no background
task in-flight, nothing set `needs_render = true` again.

`should_show_which_key()` — which checks `elapsed() > 500 ms` and arms the popup —
is only called from inside `render()`.  Because `render()` was never called again,
the popup never appeared.

The first-press coincidence: an LSP notification happened to arrive ~500 ms after
`SPC`, which set `needs_render = true`, which triggered `render()`, which triggered
`should_show_which_key()`.  This was accidental; once LSP settled it stopped happening.

---

## Decision

Add a cheap, non-mutating predicate to `KeyHandler`:

```rust
/// True when a leader sequence is active but the which-key popup has not yet
/// been shown — i.e. the 500 ms timer is still pending.
pub fn which_key_pending(&self) -> bool {
    self.sequence_start.is_some() && !self.show_which_key
}
```

Include it in the `needs_render` guard in `run()`:

```rust
if self.copilot_auth_rx.is_some()
    || self.pending_completion.is_some()
    || self.key_handler.which_key_pending()
{
    needs_render = true;
}
```

The loop iterates every ≤50 ms.  While `which_key_pending()` is true (sequence
started, popup not yet shown), `needs_render` is forced true each tick.  Within
50 ms of the 500 ms deadline `render()` is called, `should_show_which_key()` arms
the flag, and the popup appears.  Once `show_which_key` is set, `which_key_pending()`
returns false and the extra render pressure stops.

---

## Consequences

**Positive**
- Which-key popup now appears reliably on every `SPC` press, not just the first.
- `which_key_pending()` is a pure `&self` read — zero side-effects, safe to call
  every loop tick.
- The 50 ms poll interval means the popup appears within 550 ms worst-case, which
  is imperceptible from the intended 500 ms.
- No change to the which-key display or dismissal logic.

**Negative / trade-offs**
- During the ~500 ms window after `SPC`, the loop renders every 50 ms instead of
  only on events.  This is ~10 extra frames and negligible CPU cost.
