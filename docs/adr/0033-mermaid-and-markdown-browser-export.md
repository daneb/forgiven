# ADR 0033 — Mermaid Diagrams and Markdown Browser Export

**Status:** Accepted

---

## Context

ADR 0022 introduced a terminal-native markdown renderer (`src/markdown/mod.rs`) that converts
CommonMark to styled ratatui `Line`s.  It handles headings, lists, blockquotes, inline
formatting and fenced code blocks well.

A natural follow-on question is: **can Mermaid diagram definitions embedded in markdown be
rendered inside the terminal?**

Mermaid (```` ```mermaid ```` fenced blocks) is increasingly common in technical documentation —
architecture diagrams, sequence flows, ER diagrams, Gantt charts.  The existing renderer treats
them as ordinary code blocks: bordered box, green-coloured source text, no diagram output.

---

## The Challenge of Terminal Diagram Rendering

Terminals are character-cell grids.  Every "pixel" is one monospace glyph.  Rendering an
arbitrary directed graph, sequence diagram or Gantt chart in that medium requires solving
several hard sub-problems simultaneously.

### 1 — No mature Rust library

The Rust ecosystem has no stable, maintained crate that parses the full Mermaid DSL and emits
ASCII or Unicode-art output.  Mermaid itself is a JavaScript library that delegates layout and
rendering to browser-based SVG.  Porting that pipeline to Rust would be a multi-month project
with a high maintenance burden as the Mermaid spec evolves.

### 2 — Terminal graphics protocols are fragile

Terminals that support inline images (Sixel, iTerm2 image protocol, Kitty graphics protocol)
can display arbitrary bitmaps, which could in theory show a rendered diagram.  However:

- **Coverage is low** — the most widely used terminals (macOS Terminal.app, most SSH sessions,
  tmux without patches) do not support any graphics protocol.
- **ratatui / crossterm have no native support** — adding it would require a significant new
  dependency (`ratatui-image` or equivalent) and bespoke fallback handling for unsupported
  terminals.
- **Async complexity** — generating a bitmap requires either an external process or a headless
  browser, introducing latency, IPC, and new failure modes into the render loop.

### 3 — External process round-trip

The reference Mermaid CLI (`mmdc`, Node.js) can produce SVG or PNG from a diagram definition.
Using it as a subprocess would require:

1. Detecting whether `mmdc` is installed (and surfacing a useful error if not).
2. Writing diagram source to a temp file, invoking `mmdc`, reading the output — all
   synchronously or via a background task that feeds back into the render loop.
3. Converting SVG/PNG to a terminal graphics protocol payload and writing it at the correct
   cursor position within the ratatui layout — a position that can change on every resize.
4. Caching invalidation: re-invoking `mmdc` when the source changes.

This is a substantial feature in its own right, tightly coupled to terminal capabilities the
editor does not currently model.

### 4 — ASCII art quality

Tools like `graph-easy` (Perl) can render simple flowcharts as ASCII art.  The output is
typically harder to read than the source Mermaid definition for anything beyond a handful of
nodes.  Coverage of Mermaid diagram types (sequence, ER, Gantt, git graph, pie, mindmap…) is
partial or absent.  The cognitive value rarely exceeds just reading the source.

---

## Decision

### Part A — Enhanced Mermaid code block display in the TUI preview

Rather than attempting to render diagrams, the TUI preview makes Mermaid blocks visually
distinct from ordinary code blocks to communicate their nature clearly:

| Property | Ordinary code block | Mermaid block |
|----------|--------------------|--------------------|
| Header colour | DarkGray | **Yellow** |
| Header label | `  ╭─ <lang> ` | `  ╭─ mermaid diagram ─` |
| Body text colour | Green | DarkGray (dimmed) |
| Footer hint | *(none)* | `  diagram · open in a browser to render` *(italic, DarkGray)* |

The dimmed body signals that the text is a diagram spec, not runnable code.  The italic hint
below the closing border gives users a clear next step without adding any complexity to the
render pipeline.

Implementation: three targeted changes to the `Event::Start(Tag::CodeBlock)`,
`Event::Text` (when `in_code_block`), and `Event::End(TagEnd::CodeBlock)` arms of
`src/markdown/mod.rs`.  No new dependencies; no new state beyond checking
`self.code_lang == "mermaid"`.

### Part B — Markdown browser export (`SPC m b`)

A new action `Action::MarkdownOpenBrowser` (keybinding `SPC m b`) renders the current buffer
to a self-contained HTML file and opens it in the system default browser.

**Pipeline:**

1. Current buffer content is fed to `pulldown_cmark::Parser` with `Options::all()`.
2. `pulldown_cmark::html::push_html` emits a complete HTML body — the same parser already
   in use, just a different output target.
3. The body is wrapped in a minimal HTML page with a legible stylesheet (system font stack,
   800 px max-width, code blocks, blockquotes).
4. Mermaid.js 11 is loaded from CDN (`cdn.jsdelivr.net`).  A small inline script converts
   `pulldown-cmark`'s `<pre><code class="language-mermaid">` output — which is valid CommonMark
   but not what Mermaid.js expects — into `<div class="mermaid">` elements, then calls
   `mermaid.run()`.
5. The HTML is written to `$TMPDIR/forgiven_<stem>.html`.
6. The platform opener is spawned **detached** (`open` on macOS, `xdg-open` on Linux,
   `explorer` on Windows).  The TUI continues running; no suspend/restore cycle is needed.

**Why this approach over alternatives:**

- Zero new Rust dependencies — `pulldown-cmark::html` is already in the dependency tree.
- No install requirement for the user — Mermaid.js loads from CDN on demand.
- Full Mermaid support — the browser renders every diagram type correctly.
- The TUI is uninterrupted — `SPC m b` is instant from the user's perspective; the browser
  opens asynchronously.
- A temp file with a stable name (`forgiven_<stem>.html`) means re-running `SPC m b` after
  edits refreshes the same browser tab on most platforms.

---

## Consequences

**Positive**
- Mermaid diagrams in documentation are clearly labelled in the TUI preview; users are never
  confused by a wall of green source text.
- Full, correctly rendered Mermaid output is one keypress away via `SPC m b`.
- The browser export is useful for all markdown, not just files containing Mermaid — it
  doubles as a general "share / print" path.
- No complexity added to the TUI render loop; no new async tasks; no new dependencies.

**Negative / trade-offs**
- `SPC m b` requires an internet connection to render Mermaid (CDN fetch).  Users working
  offline will see the raw diagram source in the browser instead of a rendered diagram.
  A future option could bundle a local Mermaid.js asset.
- The temp file is never cleaned up automatically.  On most operating systems the OS temp
  directory is purged on reboot; this is considered acceptable for a preview artefact.
- The HTML page does not live-update as the buffer changes.  Users must press `SPC m b`
  again to refresh — consistent with the mental model of an explicit export action.
