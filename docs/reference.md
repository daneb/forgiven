# forgiven — Editor Reference

Complete reference for keybindings, editing operations, and UI modes.

---

## Modal editing — modes overview

| Mode | How to enter |
|------|-------------|
| **Normal** | `Esc` from any mode |
| **Insert** | `i`, `a`, `I`, `A`, `o`, or `O` in Normal mode |
| **Visual** | `v` in Normal mode |
| **Visual-line** | `V` in Normal mode |
| **Command** | `:` in Normal mode |
| **PickBuffer** | `SPC b b` |
| **PickFile** | `SPC f f` |
| **Explorer** | `SPC e e` |
| **InFileSearch** | `/` in Normal mode |
| **Agent** | `SPC a f` |
| **Search** | `SPC s g` |
| **MarkdownPreview** | `SPC m p` |
| **ApplyDiff** | `Ctrl+A` in Agent mode |

---

## Normal mode

| Key | Action |
|-----|--------|
| `i/a/I/A/o/O` | Enter Insert mode (at / after / line-start / line-end / new-below / new-above) |
| `h/j/k/l` | Move left / down / up / right (no line-wrap) |
| `w/b` | Word forward / backward |
| `0/^/$` | Line start / first non-blank / line end |
| `gg/G` | File top / bottom |
| `x` | Delete char at cursor |
| `dd/D/dw` | Delete line / to EOL / word (into clipboard) |
| `dt{c}` / `df{c}` | Delete till (exclusive) / find (inclusive) next occurrence of `{c}` |
| `yy/yw/y$` | Yank line / word / to EOL |
| `yt{c}` / `yf{c}` | Yank till / find next occurrence of `{c}` |
| `cc/cw` | Change line / word |
| `ct{c}` / `cf{c}` | Change till / find next occurrence of `{c}` (delete + Insert) |
| `f{c}` / `t{c}` | Move cursor to / before next occurrence of `{c}` on line |
| `F{c}` / `T{c}` | Move cursor to / after previous occurrence of `{c}` on line |
| `v` + `i/a` + `f/c/b` | Visual-select text object (inner/outer function/class/block) |
| `d` + `i/a` + `f/c/b` | Delete text object (e.g. `daf` = delete outer function) |
| `y` + `i/a` + `f/c/b` | Yank text object |
| `c` + `i/a` + `f/c/b` | Change text object (delete + Insert) |
| `p/P` | Paste after / before cursor |
| `u/Ctrl+R` | Undo / redo |
| `v/V` | Visual / Visual-line selection |
| `/` | In-file search (enter `InFileSearch` mode) |
| `n/N` | Next / previous search match |
| `:` | Command mode |
| `SPC` | Leader key (see [Leader key bindings](#leader-key-bindings-spc)) |

Numeric count prefixes are supported: `3dd`, `5j`, etc.

---

## Insert mode

| Key | Action |
|-----|--------|
| `Esc` | Return to Normal mode |
| `Tab` | Accept ghost-text completion (if visible) |
| `Backspace/Delete` | Delete before / after cursor |
| Arrows | Move cursor |

---

## Visual / Visual-line mode

| Key | Action |
|-----|--------|
| `h/j/k/l` / arrows | Extend selection |
| `w/b` | Extend selection by word |
| `0/^/$` | Extend selection to line start / first non-blank / line end |
| `G` | Extend selection to file bottom |
| `i` + `f/c/b` | Replace selection with inner text object (function/class/block) |
| `a` + `f/c/b` | Replace selection with outer text object |
| `y` | Yank selection |
| `d/x` | Delete selection |
| `c` | Delete selection and enter Insert mode |
| `Tab` / `Shift+Tab` | Indent / dedent selected lines |
| `Esc` | Cancel |

---

## Command mode (`:`)

| Command | Action |
|---------|--------|
| `:w` | Save current buffer |
| `:q` | Quit (fails if unsaved changes) |
| `:wq` | Save and quit |
| `:q!` | Force quit without saving |
| `:e <file>` | Open file |
| `:bn` / `:bp` | Next / previous buffer |

---

## Tree-sitter text objects

AST-aware text objects powered by [Tree-sitter](https://tree-sitter.github.io/). Works in
Normal mode (operate immediately) and Visual mode (select first, then operate).

| Sequence | Meaning |
|----------|---------|
| `vif` / `vaf` | Visual-select function **body** / **entire** function (incl. signature) |
| `vic` / `vac` | Visual-select class/struct/impl **body** / **entire** node |
| `vib` / `vab` | Visual-select **inner** / **outer** `{}` block |
| `dif` / `daf` | Delete inner / outer function |
| `yif` / `yaf` | Yank inner / outer function |
| `cif` / `caf` | Change inner / outer function (delete + enter Insert) |
| `dic` / `dac` | Delete inner / outer class/struct/impl |
| `dib` / `dab` | Delete inner / outer block |

The same `i`/`a` + `f`/`c`/`b` suffix applies to `y`, `d`, and `c` operators uniformly.
In Visual mode, pressing `i` or `a` followed by a kind character replaces the current selection.

Supported languages: **Rust**, **Python**, **JavaScript**, **TypeScript**, **TypeScript TSX**,
**Go**, **JSON**, **Bash**. Falls back gracefully (status message) for unsupported file types.

---

## Leader key bindings (`SPC`)

Which-key popup shows available bindings after a 500 ms pause.

| Prefix | Binding | Action |
|--------|---------|--------|
| `SPC b` | `b/n/p/d` | List / next / previous / close buffer |
| `SPC f` | `f/n/s` | Find file / new file / save |
| `SPC q` | `q` | Quit |
| `SPC l` | `h/d/r/f/s` | LSP hover / definition / rename / references / symbols |
| `SPC a` | `a/f/n` | Toggle / focus agent panel / new conversation |
| `SPC e` | `e/f/h` | Toggle / focus file explorer / toggle hidden files |
| `SPC g` | `g/n` | Open lazygit / generate release notes |
| `SPC m` | `p/b` | Markdown preview toggle / open in browser |
| `SPC s` | `g` | Search text in project (ripgrep) |
| `SPC d` | — | Diagnostics overlay (LSP, MCP, token usage) |

---

## File explorer (`Mode::Explorer`)

Open with `SPC e e`. The explorer is a left-sidebar tree with lazy directory loading.

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

Hidden by default: `target/`, `node_modules/`, `dist/`, `build/`, and dotfiles.

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

---

## In-file search (`Mode::InFileSearch`)

Enter with `/` in Normal mode.

| Key | Action |
|-----|--------|
| *(type)* | Build search pattern |
| `Backspace` | Delete last character |
| `Enter` | Run search, return to Normal mode; `n`/`N` jump between matches |
| `Esc` | Cancel, return to Normal mode |

---

## Project-wide search (`Mode::Search`, `SPC s g`)

Opens a centred popup overlay. Query field accepts ripgrep regex with smart-case.
Results update live with a 300 ms debounce; up to 500 matches displayed.

| Key | Action |
|-----|--------|
| *(type)* | Update search query (or glob if glob field focused) |
| `Tab` | Switch focus between query and file-glob fields |
| `↑` / `k` | Select previous result |
| `↓` / `j` | Select next result |
| `Enter` | Open selected file at matched line |
| `Esc` | Close panel, return to Normal mode |

The **File filter** field accepts an optional glob pattern (e.g. `*.rs`, `src/**/*.ts`).

---

## Markdown preview (`Mode::MarkdownPreview`)

Toggle with `SPC m p`. Full CommonMark rendering: headings, bold/italic, inline code,
fenced code blocks, lists, blockquotes, horizontal rules. Mermaid blocks shown with a
hint to open in browser. Status bar shows `PREVIEW` in Magenta when active.

| Key | Action |
|-----|--------|
| `j/k` or `↓/↑` | Scroll down / up one line |
| `Ctrl+D` / `Ctrl+U` | Scroll down / up half-page |
| `g` / `G` | Jump to top / bottom |
| `q` or `Esc` | Exit preview, return to Normal mode |

`SPC m b` — render the current buffer to HTML and open in the system browser;
Mermaid diagrams are rendered via Mermaid.js.

---

## Agent panel (`Mode::Agent`)

Open with `SPC a a` or focus with `SPC a f`. Supports streaming SSE responses,
scrollable history with full CommonMark rendering.

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Alt+Enter` | Insert newline in message |
| `Backspace` | Delete last character |
| `j` / `k` | Scroll history up / down |
| `Ctrl+C` | **Abort** running stream (safe at any point) |
| `Ctrl+K` | Copy next code block from last reply (cycles through all blocks) |
| `Ctrl+M` | Open next Mermaid diagram from last reply in browser (cycles; auto-fixes parens) |
| `Ctrl+Y` | Yank full last reply to system clipboard |
| `Ctrl+A` | Open apply-diff overlay for the last code block |
| `Ctrl+P` | Attach a file to the next message (context picker) |
| `Ctrl+T` | Cycle model; loads model list from API on first press |
| `Esc` | Blur panel, return to editor |

---

## Apply-diff overlay (`Mode::ApplyDiff`)

Full-screen LCS diff overlay. Opened via `Ctrl+A` in Agent mode.

| Key | Action |
|-----|--------|
| `y` / `Enter` | Apply change to target file / buffer |
| `n` / `Esc` | Discard, return to agent panel |
| `j` / `k` | Scroll down / up one line |
| `Ctrl+D` / `Ctrl+U` | Scroll down / up half-page |
