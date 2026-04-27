# ADR 0145 — MCP Fetch Server: Requirements and Choices

## Status

Accepted

## Question

Step 5 (ADR 0144) calls `McpManager::call_tool("fetch", ...)`. Does the user
need to build their own MCP server to support this?

**Short answer: No.** The official community server `@modelcontextprotocol/server-fetch`
provides exactly the required tool and can be configured in one line. Alternatively
a bundled Python fallback is available in `mcp_servers/fetch_server.py` for users
who prefer no Node dependency.

---

## Requirements

### R1 — Tool name

The MCP server MUST expose a tool named exactly `fetch`. The `McpManager` routes
`call_tool("fetch", ...)` to whichever connected server registered that tool name.

### R2 — Argument schema

The tool MUST accept at least:

| Argument | Type | Description |
|----------|------|-------------|
| `url` | string | The URL to fetch |
| `max_length` | integer (optional) | Truncate response to this many characters |

forgiven sends: `{"url": "<url>", "max_length": 50000}`

### R3 — Return format

The tool MUST return a non-empty string. The string SHOULD be Markdown-compatible
text derived from the page content (not raw HTML). forgiven pipes the result
directly into its Markdown renderer — angle-bracket soup will render poorly.

### R4 — Failure signalling

When the URL cannot be fetched, the tool SHOULD return a non-empty error string
rather than an empty string. forgiven treats an empty response as an error
(`anyhow::bail!`) and shows a status-bar message.

---

## Options

### Option A — Official community server (recommended)

`@modelcontextprotocol/server-fetch` is maintained by the MCP project itself.
It fetches URLs, strips HTML to readable text, and returns Markdown.

**Config** (`~/.config/forgiven/config.toml`):

```toml
[[mcp.servers]]
name    = "fetch"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-fetch"]
```

**Requirements:** Node ≥ 18 + npx (already required for the companion build).

**Pro:** Zero maintenance — upgrades via `npx -y`.  
**Con:** Cold-start latency on first run while npm downloads the package (~2 s).
         Subsequent runs use the npx cache and start in ~200 ms.

### Option B — Bundled Python server (no Node dependency)

A lightweight Python stdio server can be bundled in `mcp_servers/fetch_server.py`
(consistent with the existing `llmlingua_server.py` pattern).

Implementation uses only Python stdlib (`urllib.request`, `html.parser`):
- `urllib.request.urlopen(url)` — HTTP GET with redirect following
- A minimal `HTMLParser` subclass strips tags and extracts visible text
- Response is returned as plain text (Markdown-compatible for most pages)

**Config:**

```toml
[[mcp.servers]]
name    = "fetch"
command = "python3"
args    = ["/path/to/forgiven/mcp_servers/fetch_server.py"]
```

**Requirements:** Python 3 (stdlib only, no pip install needed).  
**Pro:** No Node, no network on first run, self-contained.  
**Con:** HTML → Markdown conversion is simpler than the official server's output.
         JavaScript-rendered content is not supported.

### Option C — Build a custom server

Only necessary if:
- The user needs authenticated requests (Bearer tokens, cookies)
- The user wants to scrape JS-rendered pages (requires Playwright/Puppeteer)
- The user needs custom content transformation

For standard public URLs Option A or B is sufficient.

---

## Decision

1. **Recommend Option A** (`@modelcontextprotocol/server-fetch`) as the default
   in documentation and `CONFIG.md`. Node is already a dev dependency for the
   companion, so this adds no new hard requirement.

2. **Ship Option B** (`mcp_servers/fetch_server.py`) as a zero-dependency
   fallback. Follows the existing llmlingua server pattern.

3. The ingester itself (`src/ingester/mod.rs`) is tool-agnostic — it calls
   `"fetch"` by name. Any server satisfying R1–R4 works, including custom ones.

---

## Consequences

- Add `mcp_servers/fetch_server.py` (Python stdlib, no pip deps)
- Update `CONFIG.md` with a `## [mcp] — Fetch server` quick-start section
- Update `README.md` quick-start to mention `SPC i u` and link to config
- No changes to Rust code — the implementation in ADR 0144 already handles
  the tool-agnostic call correctly
