# ADR 0090 — Visual indent/dedent, Markdown table rendering, MCP call log

**Date:** 2026-03-26
**Status:** Accepted

---

## Context

Three independent quality-of-life gaps were addressed in a single session:

1. **Visual indent/dedent** — Tab and Shift-Tab already worked in Insert mode for
   single-line indentation, but had no effect in Visual or VisualLine mode. Block
   indentation required manually inserting spaces on each line.

2. **Markdown table rendering** — `pulldown_cmark` parses GFM tables and emits
   `Tag::Table` / `Tag::TableHead` / `Tag::TableRow` / `Tag::TableCell` events, but
   the renderer treated them as unknown events and silently dropped the content,
   leaving blank output whenever the agent returned tabular data.

3. **MCP activity in the diagnostics overlay** — `SPC d` already showed MCP
   server health (connected / failed), but gave no visibility into what tool calls
   had actually been made, their results, or how long they took.

---

## Decision

### 1 — Visual indent / dedent (`buffer.rs`, `editor/input.rs`)

Two new methods on `Buffer`:

```rust
pub fn indent_selected_lines(&mut self, use_spaces: bool, tab_width: usize)
pub fn dedent_selected_lines(&mut self, tab_width: usize)
```

Both operate on `self.selection.normalized()` and clamp the end row to
`lines.len() - 1`. Indent prepends the configured indent string (spaces or tab).
Dedent removes one leading tab or up to `tab_width` leading spaces per line,
whichever applies.

`Tab` and `BackTab` are wired in both `handle_visual_input` and
`handle_visual_line_input` in `editor/input.rs`. Each call saves an undo snapshot
first and notifies LSP of the change.

### 2 — Markdown table rendering (`markdown/mod.rs`)

State fields added to `Renderer`:

```rust
in_table_cell: bool,
table_is_header_row: bool,
table_header: Vec<String>,
table_body: Vec<Vec<String>>,
table_current_row: Vec<String>,
table_current_cell: String,
```

`Event::Text` and `Event::Code` accumulate into `table_current_cell` while
`in_table_cell` is set (instead of the normal span pipeline). `SoftBreak` inside a
cell appends a space. On `End(TagEnd::Table)`, `flush_table()` is called:

- Computes natural column widths (max char-width across header + body, minimum 1).
- Scales columns proportionally if total exceeds available terminal width
  (`width - margin - border chars - padding`).
- Emits box-drawing border lines (`┌┬┐`, `├┼┤`, `└┴┘`) styled `DarkGray`.
- Header row: `Bold White`; body rows: `White`.
- Cells are truncated with `…` via a local `truncate_str` helper.

### 3 — MCP call log (`mcp/mod.rs`, `ui/mod.rs`, `ui/popups.rs`)

`McpCallRecord` is a new public struct:

```rust
pub struct McpCallRecord {
    pub server_name: String,
    pub tool_name: String,
    pub args_summary: String,   // truncated to 60 chars
    pub result_summary: String, // first line, truncated to 70 chars
    pub started_at: Instant,
    pub duration_ms: u64,
    pub is_error: bool,
}
```

`McpManager` gains a `call_log: Mutex<VecDeque<McpCallRecord>>` (cap 50, FIFO).
`call_tool()` records a `McpCallRecord` after every call (success or error).
`recent_calls() -> Vec<McpCallRecord>` exposes a snapshot for rendering.

`DiagnosticsData` gains a `mcp_call_log: Vec<McpCallRecord>` field, populated in
`editor/mod.rs` from `mcp_manager.recent_calls()`.

The `SPC d` diagnostics popup gains a new **MCP Activity** section (rendered by
`render_diagnostics_overlay` in `popups.rs`) showing the last 8 calls:

```
 MCP Activity
  ✓ search_web  searxng  42ms  3s ago
    query=ratatui table rendering  →  Found 5 results…
  ✗ read_file   filesystem  8ms  1m ago
    path=/nonexistent  →  error: No such file…
```

---

## Implementation notes

- `is_error` is heuristic: result starts with `"error"` or `"MCP tool error"`.
  This covers all current error paths without requiring a separate `Result` channel.
- `args_summary` summarises the raw JSON `arguments` value (the full
  `serde_json::Value` string representation, truncated). For large argument
  payloads this is enough for diagnostics without allocating a structured copy.
- Table column scaling uses integer arithmetic with `max(1)` guards so all columns
  remain visible even at narrow terminals.
- `lsp/mod.rs`: `Stdio::null()` added to the `kill` child-kill command to suppress
  spurious stderr noise on exit.

---

## Consequences

**Positive**
- Visual block indentation is now idiomatic (matches vim's `>` / `<` mapped to Tab /
  Shift-Tab), removing a common friction point.
- Agent responses containing tables are fully rendered instead of silently dropped.
- `SPC d` is now useful for debugging agent tool-call behaviour without needing to
  trawl log files.

**Negative / trade-offs**
- Table rendering buffers all cell content before emitting any lines — for extremely
  wide or deep tables this is memory-proportional, but in practice agent tables are
  small.
- `is_error` heuristic could produce false positives for tool results that happen to
  start with the word "error" in natural language.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0045](0045-mcp-client.md) | Original MCP client — `McpManager`, `call_tool` |
| [0048](0048-mcp-status-visualisation.md) | MCP server health in diagnostics overlay |
| [0049](0049-diagnostics-overlay.md) | `SPC d` diagnostics overlay, `DiagnosticsData` struct |
| [0070](0070-markdown-preview-visual-refresh.md) | Markdown renderer the table support extends |
| [0089](0089-large-file-split-editor-agent-ui.md) | Module split that separated `input.rs`, `popups.rs` |
