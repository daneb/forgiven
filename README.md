```
                               ┃┃┃
                               ┃┃┃
                               ┃┃┃
           ━━━━━━━━━━━━━━━━━━━━╋╋╋━━━━━━━━━━━━━━━━━━━━
                               ┃┃┃
                               ┃┃┃
                               ┃┃┃
                               ┃┃┃
                               ┃┃┃

███████╗ ██████╗ ██████╗  ██████╗ ██╗██╗   ██╗███████╗███╗   ██╗
██╔════╝██╔═══██╗██╔══██╗██╔════╝ ██║██║   ██║██╔════╝████╗  ██║
█████╗  ██║   ██║██████╔╝██║  ███╗██║██║   ██║█████╗  ██╔██╗ ██║
██╔══╝  ██║   ██║██╔══██╗██║   ██║██║╚██╗ ██╔╝██╔══╝  ██║╚██╗██║
██║     ╚██████╔╝██║  ██║╚██████╔╝██║ ╚████╔╝ ███████╗██║ ╚████║
╚═╝      ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝  ╚══════╝╚═╝  ╚═══╝

              an AI-first terminal code editor  ·  MIT License
```

> **Alpha release** — forgiven is under active development. Expect rough edges,
> breaking keybinding changes, and missing polish. Feedback and bug reports are
> welcome via [GitHub Issues](https://github.com/danebalia/forgiven/issues).

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
- **Explorer** — file tree navigation with create / rename / delete
- **RenameFile** — inline name editor with confirmation (`Enter`) or cancel (`Esc`)
- **DeleteFile** — delete confirmation popup (`y` = confirm, `n`/`Esc` = cancel)
- **NewFolder** — inline folder name editor with confirmation (`Enter`) or cancel (`Esc`)
- **InFileSearch** — `/` search with `n`/`N` next/prev match navigation

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
| `SPC e` | `e/f/h` | Toggle / focus file explorer / toggle hidden files |
| `SPC g` | `g` | Open lazygit |
| `SPC m` | `p/b` | Markdown preview toggle / open in browser |
| `SPC s` | `g` | Search text in project (ripgrep) |

### Language Server Protocol
- Auto-connects to `rust-analyzer` and `copilot-language-server` on startup
- Inline diagnostics gutter (● errors, warnings)
- Hover, go-to-definition, references, rename, document symbols

### GitHub Copilot integration
- Ghost-text inline completions (streamed, Tab to accept)
- **Agent chat panel** (`SPC a a`) — streaming SSE responses, scrollable history with
  full CommonMark rendering
- **Diff+apply** (`a` in Agent mode) — full-screen LCS diff overlay targeting the correct
  file; `y`/`Enter` to apply, `n`/`Esc` to discard

### Syntax highlighting
- `syntect` with Base16 Ocean Dark theme; highlights the visible viewport only
- Incremental cache keyed on buffer version — no re-highlight on cursor movement

### File explorer
- Left-sidebar tree (`SPC e e`); lazy directory loading
- `j`/`k` or arrows navigate; `Enter`/`l` expands a dir or opens a file
- `n` — new file (pre-fills Command mode with the target directory path)
- `m` — new folder (inline popup, `Enter` confirms, `Esc` cancels)
- `r` — rename selected entry (inline popup, `Enter` confirms, `Esc` cancels)
- `d` — delete selected entry (confirmation popup, `y` confirms, `n`/`Esc` cancels)
- `h` — toggle hidden files (`SPC e h` from Normal mode)
- `R` — reload/refresh the tree from disk
- `Esc`/`Tab` — blur explorer and return to editor
- Hides `target/`, `node_modules/`, `dist/`, `build/` and dotfiles by default

### Project-wide search (`SPC s g`)

- Opens a centred popup overlay in `SEARCH` mode
- **Query** field: text to search (ripgrep regex, smart-case)
- **File filter** field: optional glob pattern (e.g. `*.rs`, `src/**/*.ts`) — `Tab` switches focus
- Results update live with a 300 ms debounce; up to 500 matches displayed
- `↑`/`↓` or `j`/`k` navigate the list; `Enter` opens the file at the matched line
- `Esc` closes the panel and returns to Normal mode

### In-file search (`/`)
- `/` enters search mode; type a pattern and press `Enter` to highlight all matches
- `n` / `N` jump to next / previous match in Normal mode
- `Esc` cancels the search prompt without running

### Markdown (`SPC m p` / `SPC m b`)
- `SPC m p` — toggle a read-only rendered preview for any buffer
- Full CommonMark: headings, bold/italic, inline code, fenced code blocks, lists,
  blockquotes, horizontal rules; Mermaid blocks shown with a hint to open in browser
- `SPC m b` — render the current buffer to HTML and open in the system browser;
  Mermaid diagrams are rendered via Mermaid.js (CDN)
- Status bar shows `PREVIEW` in Magenta when preview is active

### Other
- lazygit full-screen overlay (`SPC g g`)
- System clipboard integration via `arboard`
- Log output to `/tmp/forgiven.log` (never pollutes the TUI)

---

![forgiven editor](main.png)

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
| `rg` (ripgrep) | `brew install ripgrep` / `cargo install ripgrep` | Project-wide search (`SPC s g`) |
| `lazygit` | `brew install lazygit` / distro package | Git UI (`SPC g g`) |
| `rust-analyzer` | `rustup component add rust-analyzer` | Rust LSP |
| `mmdc` | `npm install -g @mermaid-js/mermaid-cli` | Mermaid diagram rendering (`SPC m d`) |

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
| `/` | In-file search (enter `InFileSearch` mode) |
| `n/N` | Next / previous search match |
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

### File explorer (`Mode::Explorer`)

| Key | Action |
|-----|--------|
| `j/k` or `↓/↑` | Move cursor down / up |
| `Enter` or `l` | Expand directory / open file (returns to Normal mode) |
| `n` | New file — pre-fills Command mode with `e <dir>/` |
| `m` | New folder — opens new-folder popup |
| `r` | Rename selected entry (opens rename popup) |
| `d` | Delete selected entry (opens confirmation popup) |
| `h` | Toggle hidden files visibility |
| `R` | Reload / refresh tree from disk |
| `Esc` or `Tab` | Blur explorer, return to editor |

### Rename popup (`Mode::RenameFile`)

| Key | Action |
|-----|--------|
| *(type)* | Edit the filename |
| `Backspace` | Delete last character |
| `Enter` | Confirm rename |
| `Esc` | Cancel, return to explorer |

### Delete confirmation (`Mode::DeleteFile`)

| Key | Action |
|-----|--------|
| `y` or `Y` | Confirm deletion (permanent) |
| `n`, `N` or `Esc` | Cancel, return to explorer |

### New folder popup (`Mode::NewFolder`)

| Key | Action |
|-----|--------|
| *(type)* | Edit the folder name |
| `Backspace` | Delete last character |
| `Enter` | Confirm — creates the directory (and any missing parents) |
| `Esc` | Cancel, return to explorer |

### In-file search (`Mode::InFileSearch`)

| Key | Action |
|-----|--------|
| *(type)* | Build search pattern |
| `Backspace` | Delete last character |
| `Enter` | Run search, return to Normal mode; `n`/`N` jump between matches |
| `Esc` | Cancel, return to Normal mode |

### Markdown preview (`Mode::MarkdownPreview`)

| Key | Action |
|-----|--------|
| `j/k` or `↓/↑` | Scroll down / up one line |
| `Ctrl+D` / `Ctrl+U` | Scroll down / up half-page |
| `g` / `G` | Jump to top / bottom |
| `q` or `Esc` | Exit preview, return to Normal mode |

### Agent panel (`Mode::Agent`)

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Esc` | Blur panel, return to editor |
| `Ctrl+C` | Cancel streaming response |
| `j/k` | Scroll history |

### Search panel (`Mode::Search`, `SPC s g`)

| Key | Action |
|-----|--------|
| *(type)* | Update search query (or glob if glob field focused) |
| `Tab` | Switch focus between query and file-glob fields |
| `↑` / `k` | Select previous result |
| `↓` / `j` | Select next result |
| `Enter` | Open selected file at matched line |
| `Esc` | Close panel, return to Normal mode |

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
│   ├── search/              # Project-wide ripgrep search (SPC s g)
│   │   └── mod.rs
│   └── ui/                  # Terminal rendering (ratatui)
│       └── mod.rs
├── docs/
│   └── adr/                 # Architecture Decision Records (0001 – 0035)
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
| `rg` (ripgrep) | Project-wide text search — `rg` must be on `$PATH`; install via `brew install ripgrep` or `cargo install ripgrep` |
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
| [0024](docs/adr/0024-project-wide-text-search.md) | Project-wide Text Search |
| [0025](docs/adr/0025-explorer-hidden-files-toggle.md) | Explorer Hidden Files Toggle |
| [0026](docs/adr/0026-copilot-stream-resilience.md) | Copilot Stream Resilience |
| [0027](docs/adr/0027-agent-round-limits-and-continuation-prompts.md) | Agent Round Limits and Continuation Prompts |
| [0028](docs/adr/0028-model-selection-persistence.md) | Model Selection Persistence |
| [0029](docs/adr/0029-task-panel-for-work-tracking.md) | Task Panel for Work Tracking |
| [0030](docs/adr/0030-in-file-search-and-replace.md) | In-File Search and Replace |
| [0031](docs/adr/0031-agent-task-creation.md) | Agent-Driven Plan Strip |
| [0032](docs/adr/0032-recent-files-in-file-picker.md) | Recent Files in the Find File Picker |
| [0033](docs/adr/0033-mermaid-and-markdown-browser-export.md) | Mermaid Diagrams and Markdown Browser Export |
| [0034](docs/adr/0034-explorer-file-deletion.md) | Explorer File Deletion |
| [0035](docs/adr/0035-agent-apply-diff.md) | Agent Apply-Diff Overlay |
| [0036](docs/adr/0036-multi-line-agent-input.md) | Multi-line Agent Panel Input |
| [0037](docs/adr/0037-think-block-rendering.md) | Think-Block Rendering in the Agent Panel |
| [0038](docs/adr/0038-unified-model-selection.md) | Unified Model Selection: Removing the `model_picker_enabled` Filter |
| [0039](docs/adr/0039-agent-status-indicator.md) | Agent Status Indicator: Live Phase Tracking in the Agent Panel Title |
| [0040](docs/adr/0040-context-gauge.md) | Context Gauge: Token Usage Display in the Agent Panel Title |
| [0041](docs/adr/0041-agent-panel-copy-code-block.md) | Agent Panel Copy Code Block (`c` key) |
| [0042](docs/adr/0042-agent-paste-summary.md) | Agent Panel Paste Summary |
| [0043](docs/adr/0043-vertical-split-screen.md) | Vertical Split Screen |
| [0044](docs/adr/0044-explorer-new-folder.md) | Explorer New Folder |
| [0045](docs/adr/0045-mcp-client.md) | MCP Client Integration |
| [0046](docs/adr/0046-agent-retry-visibility.md) | Agent Retry Visibility |
| [0047](docs/adr/0047-git-commit-message-generation.md) | Git Commit Message Generation |

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

## Security & Privacy

forgiven makes **no background network calls**. The only outbound connections
are to GitHub's official Copilot endpoints and only when you actively use
Copilot features:

| Endpoint | Triggered by |
|----------|-------------|
| `api.github.com/copilot_internal/v2/token` | First Copilot action per session |
| `api.githubcopilot.com/models` | `Ctrl+T` in agent panel |
| `api.githubcopilot.com/chat/completions` | Sending a message to the agent |

No telemetry. No analytics. No crash reporting. The agent is sandboxed to your
project root — it cannot read or write files outside the directory you opened.

The CI pipeline runs `cargo-audit` (CVE scanning), `cargo-deny` (licence
checks), and GitHub code scanning on every push. `unsafe` code is forbidden
project-wide via `Cargo.toml`.

Full details — including how to audit the codebase yourself — are in
[SECURITY.md](SECURITY.md).

---

## License

MIT — see [LICENSE](LICENSE).
