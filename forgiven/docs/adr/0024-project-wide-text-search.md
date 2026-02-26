# ADR 0024 — Project-wide Text Search

**Status:** Accepted

---

## Context

Users need to locate text across all files in a project — a workflow central to every
mature editor (VS Code `Ctrl+Shift+F`, Neovim Telescope `live_grep`, Emacs `rg`
counsel).  Key requirements:

- Incremental: results update as the user types, with minimal latency.
- Filterable: optional glob pattern restricts the search to a file subset (e.g. `*.rs`,
  `src/**/*.ts`).
- Non-blocking: the search must not freeze the TUI event loop.
- Jump-to-location: pressing Enter opens the file and places the cursor at the matched
  line.

---

## Decision

### Transport layer — ripgrep via login shell

All external tool invocations in forgiven use `$SHELL -l -c "cmd"` (established in
`run_search()` in `src/search/mod.rs`) so that npm-/nvm-/brew-installed binaries are
discoverable.  ripgrep is invoked as:

```
rg --line-number --column --no-heading --color=never --smart-case \
   --max-filesize=1M \
   --glob=!.git/** --glob=!target/** --glob=!node_modules/** \
   --glob=!dist/** --glob=!build/** --glob=!*.lock \
   [--glob='<user-glob>'] \
   '<query>' .
```

The output format `path:line:col:content` is parsed with `str::splitn(4, ':')`.
Results are capped at 500 entries.  rg exit-code 1 (no matches) is treated as success;
exit-code 2 (bad pattern / binary error) is reported as an error.

### Async debounce — oneshot channel

A 300 ms debounce (matching the inline-completion debounce) prevents a new rg invocation
on every keystroke:

```rust
last_search_instant: Option<Instant>  // reset on every input change
search_rx: Option<oneshot::Receiver<Result<Vec<SearchResult>>>>  // in-flight task
```

When the debounce elapses (`run()` loop polls at ≤50 ms), `fire_search()` spawns a
`tokio::spawn` task, delivers results through the `oneshot` channel, and the loop polls
`search_rx.try_recv()` to consume them without blocking.

`search_rx.is_some()` is added to the `needs_render` guard so the TUI redraws
immediately when results arrive, even with no user activity.

### Mode — `Mode::Search` full-screen overlay

A new `Mode::Search` follows the same early-return overlay pattern as `Mode::PickFile`.
The render path exits at the top of `UI::render()` and calls `render_search_panel()`
instead of the normal editor layout.

### UI — centred three-section popup

```
┌─ Search in Project ───────────────────────────────────────┐
│ > query_                                                   │
└───────────────────────────────────────────────────────────┘
  File filter (glob) — Tab to focus
│                                                           │
└───────────────────────────────────────────────────────────┘
 5 results
│ ►  src/main.rs:42:  fn main() {                           │
│    src/lib.rs:17:   other_fn()                            │
│    …                                                      │
└── Tab=switch  ↑/↓ navigate  Enter open  Esc close ────────┘
```

The popup is `min(90, terminal_width)` columns wide and 80% of terminal height, centred
on screen.  Two input fields (query and file-glob filter) are at the top; a scrollable
results list fills the rest.  `Tab` cycles focus between the two fields.

`SearchFocus { Query, Glob }` tracks which field receives keystrokes.
`SearchStatus { Idle, Running, Done, Error(String) }` drives the status text in the
results block title.

### Keybinding — `SPC s g`

A new `SPC s` leader prefix ("search") is added to the which-key tree with a single
child: `g` → `SearchOpen`.  This mirrors grep-oriented search shortcuts common in
Spacemacs and VS Code.

---

## Consequences

**Positive**
- Live project search with 300 ms debounce; zero TUI-blocking.
- Optional glob narrows results to specific file types/directories on the fly.
- Follows established patterns (oneshot channel, login-shell invocation, early-return
  overlay mode) — no new architectural mechanisms needed.
- Enter opens the matched file and moves the cursor directly to the matched line via
  `buf.goto_line(line + 1)` (1-based).
- `needs_render` forced true while a search is in-flight, so the "searching…" indicator
  updates promptly.

**Negative / trade-offs**
- Requires `rg` (ripgrep) to be installed; a missing binary produces a status-bar error
  message from rg's stderr rather than a silent no-op.
- Results capped at 500 — sufficient for all practical queries but could miss matches in
  pathological cases.
- No regex syntax validation before spawning — malformed patterns propagate the rg
  exit-code 2 error as a `SearchStatus::Error`.

---

## Files Changed

| File | Change |
|------|--------|
| `src/search/mod.rs` | NEW — `SearchState`, `SearchResult`, `SearchFocus`, `SearchStatus`, `run_search()` |
| `src/keymap/mod.rs` | Added `Mode::Search`, `Action::SearchOpen`, `SPC s g` binding |
| `src/editor/mod.rs` | Added search fields, `handle_search_mode()`, `fire_search()`, `on_search_input_changed()`, run-loop debounce/poll |
| `src/ui/mod.rs` | Added `render_search_panel()`, `Mode::Search` early-return and status-bar label |
| `src/main.rs` | Added `mod search;` |
