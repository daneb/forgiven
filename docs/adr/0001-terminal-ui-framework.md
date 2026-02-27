# ADR 0001 — Terminal UI Framework: ratatui + crossterm

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

forgiven is a terminal-based code editor. The UI must render efficiently inside a raw
terminal (no GUI toolkit), handle arbitrary screen sizes, and redraw on every event
without tearing. The Rust ecosystem offers several options:

- **ratatui** (fork of tui-rs) — retained-mode widget library with a frame-diff renderer
- **crossterm** — cross-platform raw-mode input + ANSI output backend
- **termion** — Unix-only alternative backend
- **cursive** — higher-level TUI with event loop included

## Decision

Use **ratatui 0.29** for all widget layout and rendering, backed by **crossterm 0.28**
for terminal raw mode, keyboard events, and cross-platform ANSI output.

## Rationale

- ratatui is the actively-maintained community successor to tui-rs with a stable API
  and a large widget library (Block, Paragraph, Layout, etc.)
- crossterm is cross-platform (macOS, Linux, Windows) and the default backend for ratatui
- The `Frame`-based API (build widget tree → diff → flush) avoids cursor flicker and
  produces clean redraws even on slow terminals
- `Paragraph::new(lines).wrap(Wrap { trim: false })` handles the agent chat panel's
  word-wrapped output without a bespoke text-layout engine
- `Layout::default().direction(Direction::Horizontal)` gives us the editor/agent
  side-by-side split in two lines of code

## Consequences

- The editor event loop (`editor.run()`) is driven by `crossterm::event::poll` with a
  16 ms timeout to enable background polling (LSP messages, streaming tokens)
- All rendering happens inside `terminal.draw(|frame| { … })` closures — the frame is
  not valid outside that closure
- Because ratatui owns the terminal cell buffer, we cannot use `println!` for debug
  output; all logging goes to `/tmp/forgiven.log` via `tracing-subscriber`
