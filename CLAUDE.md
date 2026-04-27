# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build           # debug
cargo build --release # optimised

# Quality checks (run in CI order)
make check            # fmt → lint → audit → deny → test

# Individual checks
make fmt              # check formatting (fails if reformatting needed)
make fmt-fix          # auto-format all source files
make lint             # cargo clippy --all-targets --all-features -- -D warnings
make test             # cargo test
make audit            # cargo-audit CVE scan
make deny             # cargo-deny licence/advisory check

# Install required dev tools (once)
make install-tools    # cargo-audit, cargo-deny

# Watch logs while running
tail -f ~/.local/share/forgiven/forgiven.log
```

Formatting rules are in `rustfmt.toml`: max line width 100, `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"`.

Lint rules are in `Cargo.toml` under `[lints]`: `unsafe_code = "forbid"`, `dead_code = "warn"`, `unused_imports = "warn"`, `clippy::correctness = "deny"`. CI promotes all warnings to errors via `-D warnings`.

## Architecture

### High-level structure

```
src/
├── main.rs           # tokio::main, CLI, logging setup, Editor::run()
├── editor/           # All application state and the main event loop
│   ├── mod.rs        # Editor struct (~30 fields), new(), setup_services(), cleanup()
│   ├── event_loop.rs # run() — the 50 ms poll loop; polls all receivers each tick
│   ├── input.rs      # handle_key() dispatch for all modes
│   ├── mode_handlers.rs # Per-mode key logic (Normal, Visual, Command, …)
│   ├── lsp.rs        # LSP integration helpers, notify_lsp_change()
│   ├── render.rs     # render() — calls ui::render() with a RenderContext
│   └── state.rs      # Shared sub-state types (HighlightCache, FoldCache, …)
├── agent/            # AI chat panel and agentic tool loop
│   ├── panel.rs      # AgentPanel struct, streaming SSE receive
│   ├── agentic_loop.rs # Tool-calling loop (up to max_agent_rounds)
│   ├── tools.rs      # Tool definitions (read_file, edit_file, run_command, …)
│   ├── streaming.rs  # SSE parser, delta accumulation
│   └── provider.rs   # HTTP clients for Copilot, Anthropic, OpenAI, Gemini, OpenRouter
├── buffer/           # Text buffer, cursor, undo/redo history
├── ui/               # Ratatui rendering (widgets, layout, markdown renderer)
│   ├── mod.rs        # render() entry point, RenderContext struct
│   ├── agent_panel.rs
│   ├── buffer_view.rs
│   └── markdown.rs   # CommonMark → Vec<Line<'static>>
├── lsp/              # LSP client transport (stdio child process, JSON-RPC 2.0)
├── mcp/              # MCP client (stdio and HTTP+SSE transports)
├── graphics/         # Terminal image protocol detection + VisualPane widget stub (Phase 1)
├── sidecar/          # Nexus UDS IPC server (Phase 3 — broadcasts events to Tauri sidecar)
├── treesitter/       # Incremental AST engine, text objects, fold/sticky-scroll queries
├── highlight/        # syntect-based syntax highlighting with per-viewport cache
├── markdown/         # Standalone markdown renderer (used by ui/markdown.rs)
├── search/           # ripgrep-backed project-wide search
├── config/           # TOML config loader (~/.config/forgiven/config.toml)
├── keymap/           # Mode enum, KeyHandler, which-key popup
└── explorer/         # File tree sidebar, directory scanning
```

### Event loop pattern

The main loop in `editor/event_loop.rs` runs at ≤20 Hz (50 ms `crossterm::event::poll` timeout). Every tick it:

1. Polls all in-flight `oneshot::Receiver<T>` fields with `.try_recv()` (non-blocking). Any `Ok` result sets `needs_render = true`.
2. Calls `flush_sidecar_events()` for the Nexus UDS sidecar.
3. Checks if any receiver is still in-flight; if so, forces another render tick (keeps progress visible).
4. If `needs_render`, calls `self.render()`.
5. Blocks up to 50 ms for a keyboard/paste/resize event; dispatches to `handle_key()`.

New async features follow this exact pattern: spawn a `tokio::task`, pipe the result back via `oneshot::channel`, store the receiver as `Option<oneshot::Receiver<T>>` on `Editor`, poll with `.try_recv()` in the event loop.

### Editor struct conventions

- `Option<oneshot::Receiver<T>>` — in-flight request (cleared on receipt)
- `Option<Instant>` — debounce timestamps (e.g. `last_edit_instant`, `last_sidecar_send`)
- `Option<Arc<Manager>>` — shared owned handles (MCP, sidecar)
- Debounce constant: 300 ms (completion, search, sidecar buffer updates); agent render cap: 100 ms

### Rendering

`editor/render.rs` builds a `RenderContext` (bundles all per-frame data) and passes it to `ui::render()`. Widgets receive `&Frame` + `area: Rect`; they never access `Editor` directly. Per-frame caches (e.g. `HighlightCache`, `MarkdownCache`, `FoldCache`) are stored on `Editor`, keyed on content version/dimensions, and invalidated on change.

### LSP / MCP transports

- **LSP**: spawns a child process (`std::process::Command`); two `std::thread` I/O threads (not tokio tasks) read/write via `lsp-server`. Responses are matched to pending requests via `HashMap<RequestId, oneshot::Sender<Value>>`.
- **MCP**: two transport modes — stdio (child process, tokio tasks) and HTTP+SSE (persistent GET `/sse` + POST endpoint). The manager is constructed in a background task and delivered via `oneshot` to `mcp_rx`.

### Sidecar IPC (Phase 3 — Nexus)

`src/sidecar/` implements a UDS server at `/tmp/forgiven-nexus-{pid}.sock`. `SidecarServer::send()` is fire-and-forget (unbounded mpsc channel → background accept-loop task). Events are newline-delimited JSON (`NexusEvent`). The editor sends `buffer_update` (debounced 300 ms), `cursor_move` (±3-line threshold), `mode_change` (per-tick diff), and `shutdown` (on quit).

### Terminal graphics (Phase 1 — Glimpse)

`src/graphics/detect.rs` probes for Kitty/Sixel/iTerm2 support during `setup_services()` using escape sequences with 200 ms timeouts. Result stored in `editor.image_protocol`. `VisualPane` is a stub Ratatui widget; `svg_to_png()` in `graphics/svg.rs` is a `todo!()` pending Phase 2 `resvg` integration.

### Agent tool loop

`agent/agentic_loop.rs` drives multi-round tool calling. Tools are defined in `agent/tools.rs` and dispatched in `agent/tool_dispatch.rs`. The open buffer is injected into the system prompt each round (context pressure — close large files before long agent sessions). Token counts are tracked per segment in `agent/token_count.rs`.

### Architecture Decision Records

All design decisions (including intentional exclusions like no multi-cursor and no integrated terminal) are in `docs/adr/`. ADRs 0001–0138 are present. Check the ADR index in `README.md` before proposing structural changes.
