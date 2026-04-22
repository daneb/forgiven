# ADR 0136 — Blockquote Gutter Split Panic Fix

**Status:** Implemented
**Date:** 2026-04-22

---

## Context

Submitting a message beginning with `>>` in the agent panel caused forgiven to exit
silently and immediately. The terminal was cleanly restored (the `Drop` impl on `Editor`
calls `cleanup()` before unwinding), so the exit looked identical to a deliberate quit.
No error appeared in the log because no panic hook was installed.

### Root-cause trace

1. The user types `>> here` and presses Enter.
2. `AgentPanel::submit()` pushes a `ChatMessage { role: User, content: ">> here" }` to
   `self.messages` and spawns the agentic loop.
3. On the very next render tick `render_agent_panel` detects that the message count
   changed (`cache.msg_count != cur_msg_count`), invalidates the render cache, and calls
   `render_message_content(">> here", width, hl)`.
4. `render_message_content` calls `crate::markdown::render(">> here", ...)` via
   pulldown-cmark.
5. pulldown-cmark parses `>> here` as a **depth-2 nested blockquote** — CommonMark treats
   `>> text` identically to `> > text`.
6. The renderer enters `flush_para` with `self.blockquote_depth = 2`.
7. Inside `flush_para`, the new blockquote-bar colouring code added in ADR 0133 computed
   the split offset as:

   ```rust
   let bar_len = MARGIN.len() + 3 * self.blockquote_depth; // "│  " = 3 chars per level
   ```

   For `depth = 2` this evaluates to `4 + 6 = 10`.

8. The actual prefix emitted by `para_prefixes()` for depth 2 is `"    │  │  "`:

   | byte | value | char |
   |------|-------|------|
   | 0–3  | 0x20  | (4 spaces — MARGIN) |
   | 4    | 0xe2  | ╴first byte of │╶ |
   | 5    | 0x94  | ╴second byte of │╶ |
   | 6    | 0x82  | ╴third byte of │╶ |
   | 7–8  | 0x20  | (2 spaces) |
   | 9    | 0xe2  | ╴first byte of second │╶ |
   | **10**  | **0x94**  | **← second byte of second │** |
   | 11   | 0x82  | ╴third byte of second │╶ |
   | 12–13 | 0x20 | (2 spaces) |

9. `str::split_at(10)` panics: *"byte index 10 is not a char boundary; it is inside
   '│' (bytes 9..12)"*.
10. The panic unwinds the stack, `Drop` restores the terminal, the process exits.

The comment `"│  " = 3 chars per level` was correct for **character** count — `│` is
one char, plus two spaces — but the code used the char count as a **byte** offset. `│`
is U+2502 BOX DRAWINGS LIGHT VERTICAL, which encodes to **3 bytes** in UTF-8, making
the unit `"│  "` **5 bytes** wide, not 3.

### Why it was hard to spot

- The panic was swallowed by the `Drop` cleanup and not logged (no panic hook).
- Depth 1 happened to be unaffected: `4 + 3 = 7` falls on the space after the single `│`
  (byte 7 = 0x20), a valid boundary.
- Only even depths ≥ 2 triggered the fault; odd depths ≥ 3 were also fine due to the
  modular arithmetic of 3-byte chars vs 5-byte units.

---

## Decision

### 1. Fix the byte-offset formula

Replace the incorrect char-count constant with a byte-accurate calculation:

```rust
// Before (bug):
let bar_len = MARGIN.len() + 3 * self.blockquote_depth; // "│  " = 3 chars per level

// After (fix):
// "│  " = 5 bytes (│ is 3 bytes in UTF-8, plus 2 ASCII spaces).
// bar_len = byte offset right after the last │, before the trailing spaces.
// Formula: MARGIN (4 bytes) + 5 * depth - 2 (drop the 2 trailing spaces).
let bar_len = MARGIN.len() + "│  ".len() * self.blockquote_depth - 2;
```

Verification for the affected depths:

| depth | old `bar_len` | new `bar_len` | prefix bytes | boundary valid? |
|-------|--------------|--------------|-------------|----------------|
| 1 | 7 | 7 | 9 | ✓ (same) |
| 2 | **10** ✗ | **12** ✓ | 14 | ✓ |
| 3 | 13 | 17 | 19 | ✓ |
| 4 | **16** ✗ | **22** ✓ | 24 | ✓ |

`"│  ".len()` is evaluated at compile time by Rust and equals 5. Subtracting 2 is safe
because `self.blockquote_depth > 0` is checked immediately above this code, so
`5 * depth ≥ 5 > 2`.

### 2. Install a panic hook in `main`

Added `std::panic::set_hook` before the editor starts. The hook records the file, line,
and message via `tracing::error!`, which writes to the persistent log file. Future panics
will leave a trace even when `Drop` restores the terminal before the process exits.

### 3. Add regression tests

Added unit tests in `src/markdown/mod.rs` covering:

- Single-level blockquote (depth 1) renders without panic.
- Double-nested blockquote (`>> text`) renders without panic — this was the crash case.
- Triple-nested blockquote (depth 3) renders without panic.
- Blockquote output contains the expected text content.

---

## Consequences

**Fixed:** Submitting any message that pulldown-cmark parses as a nested blockquote
(depth ≥ 2) no longer panics. The most common trigger was a message starting with `>>`.

**Improved observability:** Future panics are logged to
`~/.local/share/forgiven/forgiven.log` with file and line number, making silent crash
diagnoses much faster.

**No behaviour change for depth 1:** The computed `bar_len` is identical for depth 1
(both formulas give 7).

---

## Implementation

| File | Change |
|------|--------|
| `src/markdown/mod.rs` | Fix `bar_len` formula; add regression tests |
| `src/main.rs` | Install `std::panic::set_hook` to log panics |
