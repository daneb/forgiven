# ADR 0070 — Markdown Preview Visual Refresh

**Date:** 2026-03-18
**Status:** Accepted

---

## Context

The markdown renderer (`src/markdown/mod.rs`) was functional but visually dense.
The preview mode (`SPC m b`) and agent-panel chat both consumed the same renderer,
so the rough appearance affected both surfaces. Key issues:

- **Sigil noise** — H1–H4 headings were prefixed with `▌▍▎▏` characters. These
  added visual clutter without communicating hierarchy clearly.
- **Narrow body margin** — prose was indented 2 spaces, leaving content crowded
  against the left edge.
- **Heavy code-block chrome** — fenced code blocks were wrapped in `╭─ lang ─` /
  `╰─────────────────────────` box-drawing, making every block visually heavy
  regardless of its size.
- **Backticks in inline code** — inline code was rendered as `` `code` ``; the
  cyan colour already provides the visual signal, the literal backticks were redundant.
- **Fixed-width horizontal rules** — `Event::Rule` emitted a hardcoded 35-char
  `──────────` string rather than filling the available viewport width.
- **No space before headings** — H1/H2 headings received a trailing blank line but
  no leading blank line, collapsing visual separation between sections.

---

## Decision

All changes are confined to `src/markdown/mod.rs`. The public API (`render(content,
width) -> Vec<Line<'static>>`) is unchanged.

### 1. Body margin: 2 → 4 spaces

A `const MARGIN: &str = "    ";` (4 spaces) replaces all inline `"  "` literal
prefixes throughout the renderer: body paragraphs, headings, code blocks, list
bullets, blockquote gutters, tool-call lines, and horizontal rules.

### 2. Headings: sigils removed, underlines added, spacing improved

| Level | Before | After |
|-------|--------|-------|
| H1 | `▌ Title` (yellow bold) | `Title` (yellow bold) + full-width `═══` underline (dim yellow) |
| H2 | `▍ Title` (cyan bold) | `Title` (cyan bold) + full-width `───` underline (dim cyan) |
| H3 | `▎ Title` (green bold) | `Title` (green bold), no underline |
| H4+ | `▏ Title` (white bold) | `Title` (white bold) |

A blank line is inserted **before** each heading when the output is non-empty
(i.e. not the very first element in the document), providing clear section breaks.

The underline width is computed as `width - MARGIN.len()`, so it fills the
available viewport regardless of terminal size.

### 3. Code blocks: box drawing removed, `│` gutter kept

Before:
```
  ╭─ rust ──────────────────
  │ fn main() {}
  ╰─────────────────────────
```

After:
```
      rust                   ← dim italic language label (omitted if unnamed)
      │ fn main() {}         ← DarkGray gutter, White text
                             ← single trailing blank line
```

The `╭─` header and `╰─` footer are removed. The language label is rendered as a
dim italic annotation above the content rather than as a box header. The `│` gutter
character provides structural separation with minimal weight.

For mermaid blocks the gutter and label are rendered in yellow, and the hint
`· open in a browser to render` is preserved below the content.

### 4. Inline code: backticks removed

`Event::Code` no longer wraps the text in backtick characters. The cyan colour
(`Color::Cyan`) is sufficient to distinguish inline code from prose.

### 5. Horizontal rules: full-width

`Event::Rule` now renders `"─".repeat(width - MARGIN.len() * 2)` instead of a
fixed 35-character string, respecting the actual terminal width.

### 6. Blockquote prefix updated

The blockquote prefix changes from `"  │"` to `"    │  "` (using `MARGIN`) for
consistency with the new body margin.

---

## Consequences

- Both the markdown preview (`SPC m b`) and the agent-panel chat history benefit
  from improved readability.
- The renderer is still a single-pass pulldown-cmark event loop with no new
  dependencies.
- The `render()` signature is unchanged — no call sites need updating.
- Heading anchor text (used internally by the Mermaid browser-export feature,
  ADR 0033) is unaffected because it is derived from the raw buffer content, not
  the rendered output.

### Files changed

| File | Change |
|------|--------|
| `src/markdown/mod.rs` | `MARGIN` constant; heading flush rewrite; code block chrome; inline code; horizontal rules |

---

## Related ADRs

- **ADR 0033** — Mermaid diagrams + markdown browser export (`SPC m b`)
- **ADR 0037** — `<think>` block rendering (uses `render_message_content` → same renderer)
