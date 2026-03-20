# ADR 0076 — Mermaid Diagram Browser Preview

**Date:** 2026-03-20
**Status:** Accepted

---

## Context

The agent panel frequently produces Mermaid diagrams inside fenced ` ```mermaid `
code blocks (architecture diagrams, flowcharts, sequence diagrams from spec-kit
plans, etc.). There was no way to render them from within the editor — the only
option was to manually copy the source and paste it into an external service such
as mermaid.live, which requires an account and does not work offline.

Two additional pain points drove this ADR:

1. **Parenthesis syntax breakage** — AI models commonly generate node labels that
   contain bare parentheses, e.g. `K[UseHttpMetrics (Prometheus)]`. Mermaid treats
   the inner `(` and `)` as subgraph syntax and fails to parse the diagram.

2. **Multiple diagrams per reply** — a single agent response may contain several
   diagrams (e.g. two architectural views). A one-shot copy/open approach would
   always show only the first.

---

## Decision

Add a **`Ctrl+M` keybinding in Agent mode** that:

1. Extracts Mermaid-only fenced blocks from the last assistant reply.
2. Auto-fixes unquoted parentheses in square-bracket node labels.
3. Writes a self-contained HTML file to the system temp directory.
4. Opens it immediately in the default browser.
5. Cycles through multiple diagrams on repeated presses (like `Ctrl+K` cycles
   code blocks), and resets on each new reply.

No new crate dependencies are introduced. The browser is already opened by the
`SPC m b` markdown export pathway (ADR 0033) — this reuses the same
`open`/`xdg-open`/`explorer` pattern.

---

## Implementation

### `src/agent/mod.rs`

**New field on `AgentPanel`:**

```rust
/// Cycle index for the Ctrl+M view-mermaid-diagram command.
pub mermaid_block_idx: usize,
```

Initialised to `0` in `AgentPanel::new()`; reset to `0` alongside
`code_block_idx` in the `StreamEvent::Done` handler so the index always starts
from the first diagram after each reply.

**New static method `extract_mermaid_blocks`:**

```rust
pub fn extract_mermaid_blocks(text: &str) -> Vec<String>
```

A variant of `extract_code_blocks` that inspects the opening fence's language
tag (```` ```mermaid ````) and only captures blocks whose tag
`eq_ignore_ascii_case("mermaid")`. Returns the raw diagram source without fence
lines.

### `src/editor/mod.rs`

**`fix_mermaid_parens(source: &str) -> String`** (free function)

A character-level scanner (no `regex` dependency) that finds `[…]` bracket
groups containing `(` or `)` whose content is not already quoted, and wraps the
label in double quotes:

```
A[UseHttpMetrics (Prometheus)]   →   A["UseHttpMetrics (Prometheus)"]
```

The fix is applied per-line. Already-quoted labels (`["…"]`) are left untouched.

**`fn open_mermaid_in_browser(&mut self)`** (method on `Editor`)

1. Calls `AgentPanel::extract_mermaid_blocks()` on the last reply.
2. Selects the block at `mermaid_block_idx % len` and advances the index.
3. Runs `fix_mermaid_parens` on the selected source.
4. Renders a self-contained HTML page:
   - Dark background (`#1e1e2e`), centered layout.
   - `<pre class="mermaid">` block containing the fixed source.
   - Mermaid.js loaded via ESM CDN (`mermaid@11/dist/mermaid.esm.min.mjs`),
     `theme: 'dark'`, `startOnLoad: true`.
5. Writes to `/tmp/forgiven_mermaid_N.html` (N = 1-based diagram index).
6. Opens with `open` (macOS) / `xdg-open` (Linux) / `explorer` (Windows).
7. Sets status bar: `Mermaid diagram N/Total opened in browser  (Ctrl+M for next)`.

**Keybinding** added in `handle_agent_mode()`:

```rust
KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    self.open_mermaid_in_browser();
},
```

---

## Consequences

**Positive**
- One keystroke renders any AI-generated Mermaid diagram in a full browser tab —
  no copy/paste, no external account required.
- Auto-quoting of parenthesised labels silently fixes the most common AI
  generation bug without user intervention.
- Cycling (`Ctrl+M` again for next diagram) handles multi-diagram replies.
- Zero new `Cargo.toml` dependencies.
- `cargo build` and all 11 tests pass clean.

**Negative / trade-offs**
- Requires internet access on first render to fetch Mermaid.js from the CDN.
  A future ADR could bundle the JS asset at compile time via `include_str!` to
  support fully offline use.
- The paren-fix only handles `[…]` square-bracket labels. Labels inside `(…)`,
  `{…}`, or `>…]` shapes that contain nested parens are not yet fixed.
- Temp files (`/tmp/forgiven_mermaid_N.html`) are not cleaned up automatically;
  they are overwritten on the next render of the same diagram index.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0033](0033-mermaid-diagrams-markdown-browser-export.md) | Markdown browser export — same `open`/`xdg-open` pattern reused here |
| [0041](0041-agent-panel-copy-code-block.md) | `Ctrl+K` code-block copy — same cycling index pattern (`code_block_idx`) |
| [0070](0070-markdown-preview-visual-refresh.md) | Markdown preview — context for the existing browser export pipeline |
