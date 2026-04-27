# Forgiven — "Hybrid Reliability" Visual System

**Vision:** Rich visual feedback (Markdown, Mermaid, HTML) without sacrificing the terminal-first experience.

- **Inline First:** Static visuals render inside the TUI buffer using modern terminal protocols.
- **Sidecar Second:** Interactive or high-fidelity content renders in a lightweight detached Tauri companion window.
- **Context Driven:** MCP fetches and pre-processes external content into TUI-optimised Markdown.

---

## Status

| Phase | Name | Status |
|-------|------|--------|
| Phase 1 | Glimpse — Inline TUI Graphics | Deferred — companion covers the need |
| Phase 2 | Companion — Tauri Sidecar | **Complete** |
| Phase 3 | Nexus — IPC via UDS | **Complete** |
| Phase 4 | Ingester — MCP Integration | Not started |

---

## Phase 1 — "Glimpse" (Inline TUI Graphics)

**Status: Deferred.** The Companion webview (Phase 2) renders Mermaid diagrams and markdown with full fidelity — the complexity of the Kitty/Sixel pipeline is not justified. Foundation scaffolding is retained in `src/graphics/` in case it becomes useful later.

### Scaffolding retained

- `src/graphics/detect.rs` — `ImageProtocol` enum + `detect_protocol()` (runs at startup)
- `src/graphics/visual_pane.rs` — `VisualPane` widget stub
- `src/graphics/svg.rs` — `svg_to_png()` stub
- `editor.image_protocol: Option<ImageProtocol>` — stored during `setup_services()`
- Cargo deps: `ratatui-image = "2"`, `resvg = "0.44"`, `tiny-skia = "0.11"`

---

## Phase 2 — "Companion" (Tauri Sidecar)

**Status: Complete.**

**Goal:** A secondary borderless window for interactive/high-fidelity content.

### Architecture

```
companion/                        ← Tauri v2 project root
├── src-tauri/
│   ├── Cargo.toml                ← tauri 2.x, tokio, serde_json
│   ├── tauri.conf.json           ← borderless window, CSP, no decorations
│   ├── capabilities/default.json ← shell/event permissions
│   └── src/
│       ├── main.rs               ← Tauri app entry point
│       └── nexus.rs              ← UDS client, forwards events to webview
└── ui/
    ├── index.html                ← Webview shell, loads marked.js + theme CSS
    ├── main.js                   ← listen() for nexus events, calls renderMarkdown()
    └── style.css                 ← Dark theme matching TUI colours
```

### UDS client (nexus.rs)

- Reads `NEXUS_SOCKET` env var (set by the TUI at launch: `/tmp/forgiven-nexus-{pid}.sock`)
- `tokio::net::UnixStream` connects to the socket
- `BufReader` reads line-by-line; each line is a `NexusEvent` JSON object
- On `buffer_update`: `app.emit("nexus-update", payload)` → webview
- On `cursor_move`: `app.emit("nexus-cursor", context)`
- On `mode_change`: `app.emit("nexus-mode", context.mode)`
- On `shutdown`: `std::process::exit(0)`
- Reconnect loop with 500 ms back-off when the TUI exits and a new session starts

### Window configuration (tauri.conf.json)

```json
{
  "app": {
    "windows": [{
      "decorations": false,
      "transparent": true,
      "alwaysOnTop": false,
      "width": 800,
      "height": 600,
      "resizable": true,
      "title": "Forgiven Previewer"
    }]
  }
}
```

### Frontend (main.js)

```js
import { listen } from '@tauri-apps/api/event';
import { marked } from './vendor/marked.esm.js';

listen('nexus-update', ({ payload }) => {
  document.getElementById('content').innerHTML = marked.parse(payload);
});
listen('nexus-mode', ({ payload }) => {
  document.body.dataset.mode = payload;
});
```

### Ghost overlay (Phase 2 stretch)

The TUI can query its own screen position via `xterm` OSC sequences and send the coordinates as a `window_hint` event. The Tauri window uses `window.setPosition()` to snap alongside the TUI preview pane. This is implemented after basic markdown rendering works.

### Launch integration

**Not yet implemented.** Currently the companion must be started manually. Planned:

```toml
[sidecar]
enabled = true
auto_launch = false  # opt-in; true spawns companion on editor startup
```

- Default is `false` — the companion is a separate window users may not always want.
- When `auto_launch = true`, `setup_services()` spawns the `companion` binary as a child process, passing `NEXUS_SOCKET=/tmp/forgiven-nexus-{pid}.sock` as an env var.
- A keybinding (TBD — candidate: `SPC p c`) toggles the companion open/closed at runtime regardless of `auto_launch`.
- The companion already handles the `shutdown` Nexus event by calling `std::process::exit(0)`, so forgiven quitting always cleans it up.

### Build

```bash
cd companion
npm install
npm run tauri build   # release
npm run tauri dev     # dev mode (hot-reload webview)
```

### Key files

| File | Role |
|------|------|
| `companion/src-tauri/src/main.rs` | Tauri entry point |
| `companion/src-tauri/src/nexus.rs` | UDS → webview event bridge |
| `companion/src-tauri/tauri.conf.json` | Window + security config |
| `companion/ui/index.html` | Webview shell |
| `companion/ui/main.js` | Event listener + markdown render |
| `companion/ui/style.css` | Dark theme |

---

## Phase 3 — "Nexus" (IPC via UDS)

**Goal:** Lightning-fast communication between the Rust TUI and the Tauri sidecar.

### Status: Complete

### Wire format

Newline-delimited JSON over `/tmp/forgiven-nexus-{pid}.sock`.

```json
{"event":"buffer_update","content_type":"markdown","payload":"# Hello World","context":{"file_path":"notes.md","cursor_line":1,"mode":null}}
{"event":"cursor_move","content_type":null,"payload":null,"context":{"file_path":"notes.md","cursor_line":42,"mode":null}}
{"event":"mode_change","content_type":null,"payload":null,"context":{"file_path":null,"cursor_line":null,"mode":"Insert"}}
{"event":"shutdown","content_type":null,"payload":null,"context":{"file_path":null,"cursor_line":null,"mode":null}}
```

### Event triggers

| Event | Trigger | Debounce |
|-------|---------|----------|
| `buffer_update` | Any buffer edit (via `notify_lsp_change()`) | 300 ms |
| `cursor_move` | Cursor row changes by ≥ 3 lines | None |
| `mode_change` | Mode differs from previous tick | None |
| `shutdown` | Editor `cleanup()` called | None |

### Key files

| File | Role |
|------|------|
| `src/sidecar/protocol.rs` | `NexusEvent`, `NexusContext` types |
| `src/sidecar/server.rs` | `SidecarServer`, UDS accept-loop task |
| `src/editor/mod.rs` | `flush_sidecar_events()`, `sidecar` field |
| `src/editor/event_loop.rs` | Flush call + mode-change detection per tick |
| `src/editor/lsp.rs` | `last_sidecar_send` armed on every edit |

### Smoke test

```bash
# In one terminal: run the editor
cargo run -- src/main.rs

# In another terminal: connect and watch the stream
nc -U /tmp/forgiven-nexus-$(pgrep forgiven).sock
```

---

## Phase 4 — "Ingester" (MCP Integration)

**Goal:** Pull HackerNews, blogs, and docs directly into Forgiven.

### Architecture

```
src/ingester/
├── mod.rs       # Public API: ingest(url) -> IngestResult
├── reader.rs    # MCP fetch tool → clean Markdown
└── router.rs    # Classify result: render in TUI vs. promote to sidecar
```

### Ingest flow

1. User triggers link open (keybinding TBD — `SPC i u`).
2. `reader.rs` calls the MCP `fetch` tool with the URL.
3. Response HTML is cleaned to Markdown (strip nav/ads, preserve content).
4. `router.rs` classifies the result:
   - Plain text / Markdown → render in TUI markdown preview pane.
   - Visual / rich content (images, interactive) → emit `buffer_update` over Nexus → Tauri sidecar renders it.
5. If the agent panel is open, optionally RAG-summarise the content against the current project.

### MCP tools used

| Tool | Server | Purpose |
|------|--------|---------|
| `fetch` | `@modelcontextprotocol/server-fetch` | Retrieve and clean URL content |

### Key files (planned)

| File | Role |
|------|------|
| `src/ingester/mod.rs` | `ingest(url, mcp_manager) -> IngestResult` |
| `src/ingester/reader.rs` | MCP fetch → Markdown cleanup |
| `src/ingester/router.rs` | TUI vs sidecar routing decision |

---

## Implementation Order

```
Step 1  ✅  UDS Nexus link (nc smoke test)
Step 2  ✅  ImageProtocol detection at startup (Glimpse foundation)
Step 3  ✅  Tauri companion: UDS client, markdown render, buffer/mode/cursor sync
Step 4  ✅  Mermaid rendering in companion webview (mermaid.js + DOM post-processing)
            Decision: Kitty/Sixel inline pipeline deferred — companion covers the need.
Step 4.5    Companion auto-launch from forgiven
            Config: [sidecar] auto_launch = false (opt-in)
            Impl:   setup_services() spawns companion binary with NEXUS_SOCKET env var
            Keybind: SPC p c to toggle companion at runtime
Step 5      MCP Reader Mode (URL → Markdown ingestion via src/ingester/)
Step 6      Ghost Overlay positioning (TUI OSC coords → Tauri window.setPosition())
```
