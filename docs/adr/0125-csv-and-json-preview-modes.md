# ADR 0125 — CSV and JSON Preview Modes

**Status:** Accepted

---

## Context

Forgiven already has a read-only rendered preview mode for Markdown (`SPC m p`, ADR 0022).
CSV and JSON files received no special treatment — they rendered as plain syntax-highlighted
text, which is unhelpful for:

- **CSV** — raw comma-separated values with no column alignment are hard to scan
- **JSON** — minified or deeply nested JSON is unreadable; even pretty-printed JSON
  benefits from token-level colour (keys vs. values vs. structural punctuation)

Users commonly reach for external tools (`column -t`, `jq .`) or switch to a separate
terminal pane just to inspect these file types. The goal is to provide equivalent clarity
without leaving the editor.

---

## Decision

Two new preview modes following the `MarkdownPreview` pattern exactly:

| Mode | Keybinding | Status bar |
|------|-----------|------------|
| `Mode::CsvPreview` | `SPC m c` | `CSV` (LightGreen) |
| `Mode::JsonPreview` | `SPC m j` | `JSON` (LightYellow) |

Both modes are:
- **Read-only** — no cursor, no edits; `Esc`/`q` returns to Normal
- **Scrollable** — `j/k` (line), `Ctrl+D/U` (half-page), `g/G` (top/bottom)
- **Cached** — invalidated only when buffer content changes (`lsp_version`) or the
  active buffer switches; no redundant re-renders on cursor movement or other frames
- **Universally available** — not restricted to files with a specific extension; the
  user decides when to invoke the preview

### CSV renderer (`src/csv_preview.rs`)

Public API: `pub fn render(content: &str) -> Vec<Line<'static>>`

- Parsed by the `csv` crate (`csv = "1"`) — correctly handles quoted fields containing
  commas, embedded newlines, and escaped quotes
- First row treated as a header: rendered **bold cyan + underlined**
- Per-column width computed as `max(cell_chars)` across all rows, capped at 40 to
  prevent pathological wide tables from overflowing the viewport
- Columns separated by ` │ ` (dim); header row followed by a `─┼─` divider
- On parse error: returns whatever rows succeeded, plus a `⚠ CSV parse error: …` row —
  never panics or returns an empty screen

### JSON renderer (`src/json_preview.rs`)

Public API: `pub fn render(content: &str) -> Vec<Line<'static>>`

- Parsed by `serde_json` (already a dependency); re-serialised with
  `serde_json::to_string_pretty()` for consistent indentation
- Each output line is tokenised with a simple state machine tuned to
  serde_json's predictable pretty-printer format:

| Token type | Colour |
|------------|--------|
| Object keys | Bold Blue |
| String values | Green |
| Numbers | Yellow |
| `true` / `false` | Cyan |
| `null` | Red |
| Structural (`{`, `}`, `[`, `]`, `,`) | DarkGray |

- On parse error: red bold error banner (`⚠ JSON parse error: …`) followed by the
  raw file content — the buffer remains viewable even when invalid

### Shared infrastructure

The cache structs follow `MarkdownCache` exactly (`src/editor/state.rs`):

```rust
pub(crate) struct CsvCache  { pub buffer_idx: usize, pub lsp_version: i32, pub lines: Vec<Line<'static>> }
pub(crate) struct JsonCache { pub buffer_idx: usize, pub lsp_version: i32, pub lines: Vec<Line<'static>> }
```

`viewport_width` is omitted (neither renderer word-wraps at terminal width —
CSV columns are content-driven, JSON indentation is fixed).

The existing `handle_preview_mode()` in `src/editor/mode_handlers.rs` is fully generic;
`src/editor/input.rs` was extended to dispatch `CsvPreview | JsonPreview` to it alongside
`MarkdownPreview`.

The render pipeline in `src/editor/render.rs` adds two `else if` branches after the
existing Markdown block; all three produce `preview_lines_owned: Option<Vec<Line<'static>>>`
which the downstream `render_buffer()` path already handles without modification.

---

## Consequences

**Positive**
- CSV files are immediately scannable — header row highlighted, columns aligned, no
  external tool needed
- JSON files are navigable in colour; minified JSON is auto-pretty-printed on toggle
- Zero new dependencies beyond `csv = "1"` (serde_json already present)
- No behaviour change for any existing mode; all new code is isolated in two new modules
  and small additions to the existing mode dispatch / render pipeline

**Negative / trade-offs**
- The JSON tokeniser is line-oriented and tuned to serde_json's pretty-printer output;
  it does not handle all valid JSON formatting (e.g. hand-written JSON with keys and
  values on different lines will fall through to the bare-value path)
- Large CSV files (thousands of rows) compute column widths by scanning all rows on
  first render. For files up to tens of thousands of lines this is imperceptible; the
  cache ensures it only happens once per content version
- CSV preview does not yet support sorted columns or filtering — those are interactive
  features outside the scope of a read-only viewer
