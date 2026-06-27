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

              a terminal code editor  ·  MIT License
```

> **Alpha** — under active development. Expect rough edges and breaking changes.
> Bug reports welcome via [GitHub Issues](https://github.com/danebalia/forgiven/issues).

A Vim-modal, terminal-native code editor for macOS and Linux. Written in Rust. No AI, no
network calls, no telemetry.

---

## Design philosophy

**Small surface, high confidence.** Every feature addition is weighed against the
complexity it introduces. The editor should remain fast, auditable, and maintainable by a
single developer. Deliberate exclusions are documented as ADRs so the reasoning is preserved.

**Terminal-native.** Forgiven runs inside your shell. It does not replicate what
`tmux`/`zellij` already do. The TUI is a first-class citizen of the terminal ecosystem,
not a GUI wearing a terminal costume.

**Safety-first.** `unsafe` code is forbidden project-wide. Dependencies are audited with
`cargo-audit` and `cargo-deny` on every push. Zero telemetry. No outbound network calls.

**macOS + Linux only.** The editor relies on Unix primitives and is developed and tested
exclusively on macOS and Linux (ADR 0147).

---

## Quick start

```bash
# Prerequisites: Rust toolchain (stable)
cargo build --release
./target/release/forgiven path/to/file.rs

# Install to ~/.local/bin
make build && install -m755 target/release/forgiven ~/.local/bin/forgiven

# Open a scratch buffer
forgiven
```

---

## Features

### Modal editing (Vim-style)

| Mode | Enter |
|------|-------|
| **Normal** | `Esc` from any mode |
| **Insert** | `i / a / I / A / o / O` |
| **Visual / Visual-line** | `v / V` |
| **Command** | `:` |
| **PickFile / PickBuffer** | `SPC f f` / `SPC b b` |
| **Explorer** | `SPC e e` |
| **InFileSearch** | `/` |
| **MarkdownPreview** | `SPC p p` |
| **Search** (ripgrep) | `SPC s g` |

### Editing operations

- `h/j/k/l`, `w/b`, `0/^/$`, `gg/G` — motion
- `x`, `dd/D/dw`, `cc/cw`, `dt{c}/df{c}`, `ct{c}/cf{c}` — delete / change
- `yy/yw/y$`, `yt{c}/yf{c}`, `p/P` — yank and paste
- `f{c}/t{c}`, `F{c}/T{c}` — character jumps
- `u` / `Ctrl+R` — snapshot undo / redo
- `%` — jump to matching bracket / delimiter
- `Tab` / `Shift+Tab` — indent / dedent in Visual mode
- Numeric count prefixes: `3dd`, `5j`, etc.
- `:w`, `:q`, `:wq`, `:q!`, `:e <file>`, `:bn`, `:bp`

### Tree-sitter text objects

AST-aware in Normal and Visual mode. Supported: **Rust, Python, JavaScript, TypeScript
(+ TSX), Go, JSON, Bash**.

| Sequence | Meaning |
|----------|---------|
| `vif` / `vaf` | Select function body / entire function |
| `vic` / `vac` | Select class/struct/impl body / entire node |
| `vib` / `vab` | Select inner / outer `{}` block |
| `dif`, `yif`, `cif` | Delete / yank / change — same suffixes |

### Language Server Protocol

- Auto-connects user-configured LSP servers on startup
- Inline diagnostics gutter (● errors, warnings)
- Hover, go-to-definition, references, rename, document symbols

### Other

- **File explorer** (`SPC e e`) — lazy tree, create / rename / delete, toggle hidden
- **Project-wide search** (`SPC s g`) — ripgrep with live results and file-glob filter
- **Markdown preview** (`SPC p p`) — CommonMark; `SPC p b` opens in browser with Mermaid
- **Soft wrap** (`SPC p w`) — toggle line reflow at viewport edge
- **Vertical split** (`SPC w v`) — side-by-side buffers
- **Lazygit** (`SPC g g`) — full-screen Git UI overlay
- **Diagnostics overlay** (`SPC d d`) — LSP status, recent log lines
- **Code folding** (`za` / `zM` / `zR`) — AST-based, Tree-sitter powered
- **Surround** (`ds` / `cs` / `ys`) — delete / change / add surrounding delimiters

---

## Configuration

`~/.config/forgiven/config.toml` (XDG-aware; sensible defaults if absent):

```toml
# ── Editor ────────────────────────────────────────────────────────────────
tab_width  = 4     # spaces per tab
use_spaces = true  # expand tabs to spaces

# ── LSP ──────────────────────────────────────────────────────────────────
[[lsp.servers]]
language = "rust"
command  = "rust-analyzer"
args     = []

[[lsp.servers]]
language = "python"
command  = "pylsp"
args     = []
```

---

## Keybinding reference

Full tables in **[docs/reference.md](docs/reference.md)**.

### Leader key (`SPC`)

| Prefix | Binding | Action |
|--------|---------|--------|
| `SPC b` | `b/n/p/d/D` | List / next / prev / close / force-close buffer |
| `SPC f` | `f/n/s/e` | Find file / new / save / edit config |
| `SPC q` | `q` | Quit |
| `SPC l` | `h/d/r/f/s` | LSP hover / definition / rename / references / symbols |
| `SPC e` | `e/f/h` | Toggle / focus explorer / toggle hidden files |
| `SPC g` | `g` | Lazygit |
| `SPC p` | `p/b/w` | Markdown preview / open in browser / soft wrap |
| `SPC s` | `g` | Project-wide ripgrep search |
| `SPC w` | `v/w/c` | Vertical split / focus next pane / close split |
| `SPC d` | `d/l` | Diagnostics overlay / open log file |

---

## Safety & security

**Zero telemetry.** No analytics, no crash reporting, no network calls whatsoever.

**`unsafe` is forbidden** project-wide:

```toml
[lints.rust]
unsafe_code = "forbid"
```

**Dependency auditing** runs on every push:

```bash
make audit   # cargo-audit CVE scan
make deny    # cargo-deny licence + advisory check
```

Full details in [SECURITY.md](SECURITY.md).

---

## Development & testing

```bash
# Install required dev tools (once)
make install-tools   # cargo-audit, cargo-deny

# Run all checks in CI order
make check           # fmt → lint → audit → deny → test

# Individual checks
make fmt             # check formatting (fails if reformatting needed)
make fmt-fix         # auto-format
make lint            # clippy --all-targets --all-features -D warnings
make test            # cargo test
make audit           # CVE scan
make deny            # licence / advisory check

# Run a single test (partial name match)
cargo test test_name_substring -- --nocapture

# Watch logs while running
tail -f ~/.local/share/forgiven/forgiven.log
```

Formatting rules: `rustfmt.toml` — max line width 100, `imports_granularity = "Crate"`,
`group_imports = "StdExternalCrate"`.

CI gates: fmt, clippy (zero warnings `-D warnings`), audit (zero advisories), deny (zero
violations), full test suite — all must pass before merge.

### Optional runtime tools

| Tool | Install | Required for |
|------|---------|--------------|
| `rg` | `brew install ripgrep` | Project-wide search (`SPC s g`) |
| `lazygit` | `brew install lazygit` | Git UI (`SPC g g`) |
| `rust-analyzer` | `rustup component add rust-analyzer` | Rust LSP |

---

## Architecture

Source layout and design decisions are documented in [`docs/adr/`](docs/adr/)
(ADRs 0001–0150). Key modules:

```
src/
├── main.rs           # tokio::main, CLI, logging
├── editor/           # Editor struct, event loop, input dispatch, render
├── buffer/           # Text buffer, cursor, snapshot undo/redo
├── ui/               # Ratatui widgets and render entry point
├── lsp/              # LSP client (stdio, JSON-RPC 2.0)
├── treesitter/       # Incremental AST engine, text objects, folding
├── highlight/        # syntect syntax highlighting, per-viewport cache
├── explorer/         # File tree sidebar
├── search/           # ripgrep project-wide search
├── markdown/         # CommonMark → ratatui Lines
├── keymap/           # Mode enum, KeyHandler, which-key popup
└── config/           # TOML config loader
```

The event loop runs at ≤20 Hz (50 ms poll). Async results come back via
`oneshot::Receiver<T>` fields polled each tick with `.try_recv()` — no blocking,
no separate render thread.

---

## License

MIT — see [LICENSE](LICENSE).
