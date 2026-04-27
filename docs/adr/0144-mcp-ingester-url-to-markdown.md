# ADR 0144 — MCP Ingester: URL → Markdown via `SPC i u`

## Status

Accepted

## Context

forgiven has a companion window (Tauri sidecar, ADR 0141) that renders markdown
from the current buffer. A natural extension is to fetch external URLs as
markdown and view them without leaving the editor. The MCP ecosystem already
provides a `fetch` tool that returns page content as markdown-compatible text,
making a dedicated HTTP client unnecessary.

## Decision

Implement a URL ingestion pipeline under the `SPC i` leader namespace:

- `SPC i u` opens a URL input popup (`Mode::IngesterUrl`)
- Enter dispatches `Action::IngesterFetchUrl`, which calls
  `crate::ingester::fetch_url(mcp, url)` in a background tokio task
- The task sends the result back via `oneshot::channel` (same pattern as every
  other async receiver on `Editor`)
- On success the event loop opens the markdown as a scratch buffer named
  `[ingested]` and enters `Mode::MarkdownPreview`
- If the companion is open the content is simultaneously forwarded via
  `NexusEvent::buffer_update` so the Tauri window renders it too

## Why MCP fetch over raw reqwest

- Zero new dependencies — `McpManager::call_tool` is already wired
- The MCP `fetch` tool handles JS-heavy pages, redirects, and encoding;
  the editor stays thin
- Consistent with the agent's existing tool-call approach

## Why a scratch buffer

`Mode::MarkdownPreview` renders the current buffer's content. Opening a new
in-memory buffer is the minimal approach — no temp files, no disk I/O, no LSP
noise. The buffer name `[ingested]` is a conventional signal (like Vim's
`[Scratch]`) that the content is ephemeral.

## Dual output (TUI + companion)

When the companion is open a `buffer_update` Nexus event is sent with
`content_type = "markdown"` and `file_path = None`. The companion renders it
identically to a normal buffer update. `file_path = None` prevents the companion
from attempting local image rewriting on fetched content.

## `SPC i` namespace allocation

`SPC i` was unoccupied. The letter `i` mnemonically maps to "ingest" and leaves
room for future sub-commands (e.g. `SPC i f` for file ingestion, `SPC i c` for
clipboard).

## Consequences

- New `src/ingester/mod.rs` module (~15 lines)
- New `Mode::IngesterUrl` and two `Action` variants
- `ingester_url_buf: String` + `ingester_rx: Option<oneshot::Receiver<…>>`
  fields on `Editor` (minimal footprint)
- Requires at least one MCP server with a `fetch` tool configured; graceful
  "MCP not connected" message otherwise
