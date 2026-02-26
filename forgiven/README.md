# Forgiven Editor

An AI-first, terminal-based code editor with GitHub Copilot agent integration, inspired
by Emacs / Spacemacs key philosophy and Vim modal editing.

---

## Features

### Modal editing (Vim-style)
- **Normal** — navigation, operators, leader-key commands
- **Insert** — full text insertion and deletion
- **Visual / Visual-Line** — character and line-wise selection with yank/delete
- **Command** — colon commands (`:w`, `:q`, `:wq`, `:q!`, `:e <file>`, `:bn`, `:bp`)
- **PickBuffer / PickFile** — fuzzy-style buffer and file pickers

### Navigation & editing
- `h/j/k/l`, arrows, `w/b`, `0/^/$`, `gg/G`
- `x` delete char, `dd/D/dw` delete line/EOL/word, `cc/cw` change
- `yy/yw/y$` yank; `p/P` paste; multi-line block yank/paste
- `u` undo, `Ctrl+R` redo (snapshot-based history)
- Numeric count prefix: `3dd`, `5j`, etc.

### Spacemacs-style leader key (`SPC`)
Which-key popup shows available bindings after a 500 ms pause.

| Prefix | Binding | Action |
|--------|---------|--------|
| `SPC b` | `b/n/p/d` | List / next / previous / close buffer |
| `SPC f` | `f/n/s` | Find file / new file / save |
| `SPC q` | `q` | Quit |
| `SPC l` | `h/d/r/f/s` | LSP hover / definition / rename / references / symbols |
| `SPC a` | `a/f` | Toggle / focus agent panel |
| `SPC e` | `e/f` | Toggle / focus file explorer |
| `SPC g` | `g` | Open lazygit |
| `SPC m` | `p` | Markdown preview toggle |

### Language Server Protocol
- Auto-connects to `rust-analyzer` and `copilot-language-server` on startup
- Inline diagnostics gutter (● errors, warnings)
- Hover, go-to-definition, references, rename, document symbols

### GitHub Copilot integration
- Ghost-text inline completions (streamed, Tab to accept)
- **Agent chat panel** (`SPC a a`) — streaming SSE responses, code-apply, scrollable
  history with full CommonMark rendering

### Syntax highlighting
- `syntect` with Base16 Ocean Dark theme; highlights the visible viewport only
- Incremental cache keyed on buffer version — no re-highlight on cursor movement

### File explorer
- Left-sidebar tree (`SPC e e`); lazy directory loading
- `Enter` expands dirs / opens files; `Esc/Tab` blurs back to editor
- Hides `target/`, `node_modules/`, `dist/`, `build/` and dotfiles

### Markdown (`SPC m p`)
- Toggle a read-only rendered preview for any buffer
- Full CommonMark: headings, bold/italic, inline code, fenced code blocks, lists,
  blockquotes, horizontal rules
- Available for any buffer; status bar shows `PREVIEW` in Magenta

### Other
- lazygit full-screen overlay (`SPC g g`)
- System clipboard integration via `arboard`
- Log output to `/tmp/forgiven.log` (never pollutes the TUI)

---

## Quick Start

```bash
# Build
cargo build --release

# Open a project directory
./target/release/forgiven /path/to/project

# Open specific files
./target/release/forgiven src/main.rs

# Start with a scratch buffer
./target/release/forgiven
```

### Optional runtime dependencies

| Tool | Install | Required for |
|------|---------|--------------|
| `mmdc` | `npm install -g @mermaid-js/mermaid-cli` | Mermaid diagram rendering (`SPC m d`) |
| `lazygit` | `brew install lazygit` / distro package | Git UI (`SPC g g`) |
| `rust-analyzer` | `rustup component add rust-analyzer` | Rust LSP |

---

## Keybinding Reference

### Normal mode

| Key | Action |
|-----|--------|
| `i/a/I/A/o/O` | Enter Insert mode (at / after / line-start / line-end / new-below / new-above) |
| `h/j/k/l` | Move left / down / up / right (no line-wrap) |
| `w/b` | Word forward / backward |
| `0/^/$` | Line start / first non-blank / line end |
| `gg/G` | File top / bottom |
| `x` | Delete char at cursor |
| `dd/D/dw` | Delete line / to EOL / word (into clipboard) |
| `yy/yw/y$` | Yank line / word / to EOL |
| `cc/cw` | Change line / word |
| `p/P` | Paste after / before cursor |
| `u/Ctrl+R` | Undo / redo |
| `v/V` | Visual / Visual-line selection |
| `:` | Command mode |
| `SPC` | Leader key (see table above) |

### Insert mode

| Key | Action |
|-----|--------|
| `Esc` | Return to Normal mode |
| `Tab` | Accept ghost-text completion (if visible) |
| `Backspace/Delete` | Delete before / after cursor |
| Arrows | Move cursor |

### Visual / Visual-line mode

| Key | Action |
|-----|--------|
| `h/j/k/l` / arrows | Extend selection |
| `y` | Yank selection |
| `d/x` | Delete selection |
| `Esc` | Cancel |

### Agent panel (`Mode::Agent`)

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Esc` | Blur panel, return to editor |
| `Ctrl+C` | Cancel streaming response |
| `j/k` | Scroll history |

---

## Project Structure

```
forgiven/
├── src/
│   ├── main.rs              # Entry point, CLI parsing, project-root setup
│   ├── agent/               # Copilot agent chat panel (streaming SSE, tool calls)
│   │   ├── mod.rs
│   │   └── tools.rs
│   ├── buffer/              # Buffer management
│   │   ├── buffer.rs        # Core text buffer, cursor, edit operations
│   │   ├── cursor.rs        # Cursor position
│   │   └── history.rs       # Snapshot undo/redo
│   ├── config/              # TOML config loader
│   │   └── mod.rs
│   ├── editor/              # Main event loop and editor state
│   │   └── mod.rs
│   ├── explorer/            # File explorer tree sidebar
│   │   └── mod.rs
│   ├── highlight/           # Syntax highlighting (syntect)
│   │   └── mod.rs
│   ├── keymap/              # Modal keybinding system + which-key
│   │   └── mod.rs
│   ├── lsp/                 # LSP client (rust-analyzer, copilot-language-server)
│   │   └── mod.rs
│   ├── markdown/            # CommonMark → ratatui Lines renderer
│   │   └── mod.rs
│   ├── mermaid/             # Mermaid block detection + mmdc subprocess
│   │   └── mod.rs
│   └── ui/                  # Terminal rendering (ratatui)
│       └── mod.rs
├── docs/
│   └── adr/                 # Architecture Decision Records (0001 – 0023)
└── Cargo.toml
```

---

## Dependencies

### Runtime crates

| Crate | Version | Purpose |
|-------|---------|---------|
| `ratatui` | 0.30 | TUI framework — layout, widgets, rendering |
| `crossterm` | 0.28 | Cross-platform terminal backend for ratatui |
| `tokio` | 1 | Async runtime (full feature set) |
| `serde` | 1 | Serialisation derive macros |
| `serde_json` | 1 | JSON encode/decode (LSP messages, Copilot API) |
| `toml` | 0.8 | Config file parsing |
| `clap` | 4 | CLI argument parsing (derive API) |
| `anyhow` | 1 | Ergonomic error propagation |
| `thiserror` | 2 | Typed error enum derive |
| `tracing` | 0.1 | Structured logging |
| `tracing-subscriber` | 0.3 | Log filtering and file output |
| `notify` | 7 | File system watching |
| `unicode-width` | 0.2 | Display-width of Unicode characters |
| `unicode-segmentation` | 1 | Grapheme cluster iteration |
| `lsp-types` | 0.97 | LSP protocol type definitions |
| `lsp-server` | 0.7 | LSP server transport primitives |
| `url` | 2 | URI handling for LSP |
| `reqwest` | 0.12 | HTTP client for Copilot API (JSON + streaming) |
| `futures-util` | 0.3 | Async stream utilities (SSE response streaming) |
| `syntect` | 5 | Syntax highlighting engine (Base16 Ocean Dark) |
| `arboard` | 3 | System clipboard read/write |
| `pulldown-cmark` | 0.12 | CommonMark parser for markdown rendering |

### Dev crates

| Crate | Version | Purpose |
|-------|---------|---------|
| `pretty_assertions` | 1 | Coloured diff output in test failures |

### Optional runtime tools (not in Cargo.toml)

| Tool | Purpose |
|------|---------|
| `lazygit` | Full-screen Git UI overlay (`SPC g g`) |
| `rust-analyzer` | Rust language server |
| `copilot-language-server` | GitHub Copilot LSP server |

---

## Architecture Decision Records

All design decisions are documented in [`docs/adr/`](docs/adr/).

| ADR | Title |
|-----|-------|
| [0001](docs/adr/0001-terminal-ui-framework.md) | Terminal UI Framework |
| [0002](docs/adr/0002-async-runtime-and-event-loop.md) | Async Runtime and Event Loop |
| [0003](docs/adr/0003-lsp-integration-architecture.md) | LSP Integration Architecture |
| [0004](docs/adr/0004-copilot-authentication.md) | Copilot Authentication |
| [0005](docs/adr/0005-copilot-inline-completions-ghost-text.md) | Copilot Inline Completions / Ghost Text |
| [0006](docs/adr/0006-agent-chat-panel.md) | Agent Chat Panel |
| [0007](docs/adr/0007-vim-modal-keybindings.md) | Vim Modal Keybindings |
| [0008](docs/adr/0008-normal-mode-editing-operations.md) | Normal Mode Editing Operations |
| [0009](docs/adr/0009-syntax-highlighting-syntect.md) | Syntax Highlighting (syntect) |
| [0010](docs/adr/0010-file-explorer-tree-sidebar.md) | File Explorer Tree Sidebar |
| [0011](docs/adr/0011-agentic-tool-calling-loop.md) | Agentic Tool-Calling Loop |
| [0012](docs/adr/0012-agent-ux-context-and-file-refresh.md) | Agent UX: Context and File Refresh |
| [0013](docs/adr/0013-project-folder-argument.md) | Project Folder Argument |
| [0014](docs/adr/0014-agent-model-selection.md) | Agent Model Selection |
| [0015](docs/adr/0015-file-creation-and-explorer-enhancements.md) | File Creation and Explorer Enhancements |
| [0016](docs/adr/0016-vim-yank-paste-register.md) | Vim Yank / Paste Register |
| [0017](docs/adr/0017-multi-line-yank-delete-visual-line.md) | Multi-line Yank / Delete / Visual Line |
| [0018](docs/adr/0018-horizontal-scroll-viewport-fix.md) | Horizontal Scroll Viewport Fix |
| [0019](docs/adr/0019-snapshot-undo-redo.md) | Snapshot Undo / Redo |
| [0020](docs/adr/0020-lazygit-integration.md) | Lazygit Integration |
| [0021](docs/adr/0021-render-loop-performance.md) | Render Loop Performance |
| [0022](docs/adr/0022-markdown-rendering.md) | Markdown Rendering (Agent Panel + Editor Preview) |
| [0023](docs/adr/0023-which-key-render-timer.md) | Which-Key Popup Render Timer |

---

## Development

```bash
# Debug build
cargo build

# Watch logs while running
tail -f /tmp/forgiven.log

# Run tests
cargo test
```

---

## License

To be determined.
