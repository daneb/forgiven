# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build           # debug
cargo build --release # optimised
make build            # cargo build --release

# Quality checks (run in CI order)
make check            # fmt → lint → audit → deny → test

# Individual checks
make fmt              # check formatting (fails if reformatting needed)
make fmt-fix          # auto-format all source files
make lint             # cargo clippy --all-targets --all-features -- -D warnings
make test             # cargo test
make audit            # cargo-audit CVE scan
make deny             # cargo-deny licence/advisory check

# Run a single test (partial name match, -- --nocapture to see output)
cargo test test_name_substring
cargo test config::tests::active_model_copilot -- --nocapture

make install          # build release binary and install to ~/.local/bin

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
│   ├── mod.rs        # Editor struct, new(), setup_services(), cleanup()
│   ├── event_loop.rs # run() — the 50 ms poll loop; polls all receivers each tick
│   ├── input.rs      # handle_key() dispatch for all modes
│   ├── mode_handlers.rs # Per-mode key logic (Normal, Visual, Command, …)
│   ├── actions.rs    # Action enum dispatch (MarkdownPreview, SplitVertical, …)
│   ├── render.rs     # render() — calls ui::render() with a RenderContext
│   ├── state.rs      # Shared sub-state types (HighlightCache, FoldCache, …)
│   ├── lsp.rs        # LSP integration helpers, notify_lsp_change()
│   ├── file_ops.rs   # File I/O, buffer lifecycle, open_markdown_in_browser()
│   ├── pickers.rs    # Buffer and file picker modes
│   ├── search.rs     # In-file search mode (/)
│   ├── folding.rs    # AST-based code folding (za, zM, zR)
│   ├── text_objects.rs # Tree-sitter text object selection
│   └── surround.rs   # Surround operations (ds, cs, ys)
├── buffer/           # Text buffer, cursor, undo/redo history
├── ui/               # Ratatui rendering (widgets, layout, markdown renderer)
│   ├── mod.rs        # render() entry point, RenderContext struct
│   ├── buffer_view.rs
│   ├── popups.rs     # Overlay widgets (diagnostics, rename, delete, file info)
│   └── markdown.rs   # CommonMark → Vec<Line<'static>>
├── lsp/              # LSP client transport (stdio child process, JSON-RPC 2.0)
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
2. Checks if any receiver is still in-flight; if so, forces another render tick (keeps progress visible).
3. If `needs_render`, calls `self.render()`.
4. Blocks up to 50 ms for a keyboard/paste/resize event; dispatches to `handle_key()`.

New async features follow this exact pattern: spawn a `tokio::task`, pipe the result back via `oneshot::channel`, store the receiver as `Option<oneshot::Receiver<T>>` on `Editor`, poll with `.try_recv()` in the event loop.

### Editor struct conventions

- `Option<oneshot::Receiver<T>>` — in-flight request (cleared on receipt)
- `Option<Instant>` — debounce timestamps (e.g. `last_edit_instant`)
- Debounce constant: 300 ms (search, LSP notifications)

### Rendering

`editor/render.rs` builds a `RenderContext` (bundles all per-frame data) and passes it to `ui::render()`. Widgets receive `&Frame` + `area: Rect`; they never access `Editor` directly. Per-frame caches (e.g. `HighlightCache`, `MarkdownCache`, `FoldCache`) are stored on `Editor`, keyed on content version/dimensions, and invalidated on change.

### LSP transport

Spawns a child process (`std::process::Command`); two `std::thread` I/O threads (not tokio tasks) read/write via `lsp-server`. Responses are matched to pending requests via `HashMap<RequestId, oneshot::Sender<Value>>`.

### Architecture Decision Records

All design decisions (including intentional exclusions like no multi-cursor and no integrated terminal) are in `docs/adr/`. ADRs 0001–0150 are present. ADRs marked `Superseded` are historical; check the status header before treating an ADR as authoritative. Check `docs/adr/README.md` before proposing structural changes.
