# ADR 0003 — LSP Integration Architecture

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

Language intelligence (diagnostics, hover, go-to-definition, completions) requires
talking to language servers over the Language Server Protocol (LSP) — JSON-RPC 2.0
over stdin/stdout of a child process.

Key constraints:
- Multiple language servers may be active simultaneously (e.g. rust-analyzer + Copilot)
- LSP stdio reads block indefinitely while waiting for messages
- The editor event loop must never block waiting for an LSP response

## Decision

### Process spawning

Each language server is spawned with `std::process::Command` with:
- `stdin(Stdio::piped())` — for sending JSON-RPC requests
- `stdout(Stdio::piped())` — for reading JSON-RPC responses/notifications
- `stderr(Stdio::piped())` — for capturing diagnostic output to `/tmp/forgiven.log`
- Augmented `PATH` that includes Homebrew (`/opt/homebrew/bin`) and dynamically
  discovered nvm version paths, so `npx` and other tools can be found regardless
  of shell initialisation state

### Threading model

Each `LspClient` owns **two background `std::thread`s**:

```
┌─────────────────────────────────────────────────────────┐
│                     LspClient                           │
│                                                         │
│  writer_thread ← writer_rx ← send_request()/notify()   │
│       │                                                 │
│       └─► child.stdin (JSON-RPC wire)                   │
│                                                         │
│  reader_thread ─► child.stdout (JSON-RPC wire)          │
│       │                                                 │
│       └─► response_tx / notification_tx                 │
│             ↓                       ↓                   │
│   pending_requests HashMap    LspNotificationMsg channel│
└─────────────────────────────────────────────────────────┘
                                       ↓
                              LspManager::drain_messages()
                                       ↓
                              Editor event loop
```

- **writer thread**: receives `lsp_server::Message` values from a `std::sync::mpsc`
  channel and writes them to the child's stdin
- **reader thread**: reads JSON-RPC frames from the child's stdout, matches responses
  to pending `oneshot::Sender`s in a `HashMap<RequestId, oneshot::Sender<Value>>`,
  and forwards notifications to a `notification_tx` channel

### Request/response matching

```rust
fn send_request<R: lsp_types::request::Request>(&mut self, params: R::Params)
    -> Result<oneshot::Receiver<serde_json::Value>>
```

Each outgoing request gets an auto-incremented integer ID. A `oneshot::Sender` is
stored in `pending_requests`. When the reader thread sees a response with that ID, it
fires the sender. The caller polls `rx.try_recv()` each event loop frame.

### Raw JSON requests for LSP 3.18+

`lsp-types 0.97` predates `textDocument/inlineCompletion` (LSP 3.18). Rather than
upgrading (which risks breaking other typed APIs), an additional
`inline_completion(&mut self, uri, line, character)` method was added that constructs
the JSON payload manually and uses the same `pending_requests` infrastructure.

### Initialization options

The `initialize()` call accepts `Option<serde_json::Value>` for
`InitializeParams::initialization_options`. The Copilot language server requires:

```json
{
  "editorInfo":       { "name": "forgiven", "version": "0.1.0" },
  "editorPluginInfo": { "name": "forgiven-copilot", "version": "0.1.0" }
}
```

Without these, Copilot returns `-32002 "editorInfo and editorPluginInfo not set"`.

### LspManager

`LspManager` holds a `HashMap<String, LspClient>` keyed by language identifier (e.g.
`"rust"`, `"copilot"`). `drain_messages()` is called each event-loop frame to process
all pending notifications (diagnostics, `window/showMessage`, etc.) without blocking.

## Consequences

- Language servers are auto-started based on `~/.config/forgiven/config.toml`
- Multiple servers run concurrently; each has its own pair of background threads
- The editor is non-blocking: it never awaits an LSP response synchronously
- LSP capabilities (hover, definition, rename, etc.) are currently stubbed —
  the infrastructure sends the request but the UI has no popup widget yet
- `copilot-language-server` (npm: `@github/copilot-language-server`) is treated as a
  standard LSP server with the sentinel language key `"copilot"`, plus Copilot-specific
  custom methods (`checkStatus`, `signInInitiate`) for the authentication flow
