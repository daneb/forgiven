# ADR 0002 — Async Runtime and Event Loop Design

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

The editor must concurrently:

1. Process keyboard input (blocking crossterm poll)
2. Read JSON-RPC messages from multiple LSP server processes (blocking stdio reads)
3. Stream HTTP SSE tokens from the Copilot Chat API
4. Drive the ratatui render loop at ~60 fps

These requirements clash: crossterm's `event::read()` blocks, LSP stdio reads block,
and async HTTP streaming requires a non-blocking async executor.

## Decision

Use **tokio** (multi-threaded runtime via `#[tokio::main]`) as the async executor.
Structure the event loop as a **16 ms poll loop** inside an `async fn run()`:

```
loop {
    if crossterm::event::poll(Duration::from_millis(16))? {
        let key = crossterm::event::read()?;
        self.handle_key(key)?;
    }
    self.drain_lsp_messages();
    self.poll_agent_stream();
    self.render()?;
}
```

LSP I/O runs in **dedicated background `std::thread`s** (not tokio tasks) because:
- `BufReader::read_line()` on LSP stdout is inherently blocking
- Tokio tasks must not block; mixing blocking reads with tokio would starve the executor

Async code (Copilot HTTP, token exchange) runs in proper **tokio tasks** via
`tokio::spawn`.

The bridge between the sync event loop and async tasks uses:
- `tokio::sync::oneshot` for request/response (LSP pending requests)
- `tokio::sync::mpsc::unbounded_channel` for streaming tokens (Copilot SSE)
- `tokio::task::block_in_place` to call `.await` at the sync/async boundary when the
  user presses Enter to submit a chat message

## Consequences

- The 16 ms loop gives ~60 fps renders and sub-frame LSP/stream polling
- `block_in_place` in `handle_agent_mode` blocks the current tokio thread while the
  `submit()` future runs; this is acceptable because it only happens on Enter key, not
  every frame
- LSP writer threads receive outgoing messages through a `std::sync::mpsc` sender;
  the LSP reader threads push decoded notifications/responses through a second channel
  back to the editor loop
- No `Arc<Mutex<…>>` shared state — all mutable editor state lives on the `Editor`
  struct and is accessed only from the single event-loop thread
