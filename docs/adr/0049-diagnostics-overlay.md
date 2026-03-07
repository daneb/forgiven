# ADR 0049: Diagnostics Overlay

**Date:** 2026-03-07
**Status:** Accepted

## Context

Forgiven had no in-app way to inspect its own runtime state. Failures were silently swallowed or written only to `/tmp/forgiven.log`, which requires leaving the editor. Specifically:

- MCP server connection failures were truncated to a single line in the agent panel bottom bar, making long error chains unreadable.
- There was no way to confirm which LSP servers were active.
- Tracing events (WARN/ERROR) were only visible by tailing the log file externally.

## Decision

### Keybinding

| Key | Action |
|-----|--------|
| `SPC d` | Open diagnostics overlay (`Action::DiagnosticsOpen` → `Mode::Diagnostics`) |

Any key closes the overlay and returns to `Mode::Normal`.

### `Mode::Diagnostics` and `Action::DiagnosticsOpen`

Added to `keymap/mod.rs`. The mode is treated like other popup modes — it is excluded from the panel-cycle guard (Ctrl+Tab) and has no editable state.

### `DiagnosticsData<'a>` struct (`src/ui/mod.rs`)

Passed into `UI::render` as `diag_overlay: Option<&DiagnosticsData<'_>>`. Fields:

| Field | Type | Description |
|-------|------|-------------|
| `mcp_connected` | `Vec<(&str, usize)>` | (server_name, tool_count) for connected servers |
| `mcp_failed` | `&[(String, String)]` | (server_name, full_error) for failed servers |
| `lsp_servers` | `Vec<&str>` | Language names from config |
| `log_path` | `&str` | Path to the log file (`/tmp/forgiven.log`) |
| `recent_logs` | `&[(String, String)]` | (level, message) newest-last, up to 50 entries |

### `render_diagnostics_overlay()` (`src/ui/mod.rs`)

Centred floating popup (60 cols wide) rendered over the current view using `Clear`. Three sections:

- **MCP Servers** — green `✓ name  N tools` for connected, red `✗ name  failed:` with the full anyhow error chain on subsequent lines for failed servers.
- **LSP Servers** — green `●` bullet per configured language.
- **Recent Logs** — colour-coded by level (red=ERROR, yellow=WARN, gray=INFO) with the log file path in the section header.

### Full error capture (`src/mcp/mod.rs`)

Previously, `failed_servers` stored only `e.to_string().lines().next()` (first line). Now stores `format!("{e:#}")` — the full anyhow error chain with `:#` pretty-printing — so multi-level causes are visible in the overlay.

### In-memory log ring buffer

**`src/main.rs`** — `RingBufLayer` implements `tracing_subscriber::Layer`. It intercepts events at WARN level or above, extracts the `message` field via `MessageVisitor` (a `tracing::field::Visit` impl), and appends `(level, message)` to a `Arc<Mutex<VecDeque>>` capped at 50 entries. The same `Arc` is installed into the tracing subscriber registry and shared with `Editor.log_buffer`.

**`src/editor/mod.rs`** — `pub log_buffer: Arc<Mutex<VecDeque<(String, String)>>>` field on `Editor`. Initialised to an empty deque; replaced in `main()` with the shared arc after the subscriber is installed. The deque is snapshotted (cloned) into `recent_logs_owned` each render when `Mode::Diagnostics` is active.

## Consequences

- Users can press `SPC d` at any time to see exactly which MCP and LSP servers are running, with full error detail for anything that failed — no log file required for common cases.
- The last 50 WARN/ERROR events are always available in-app, covering MCP failures, LSP errors, agent retries, and any other subsystem that uses `tracing::warn!` or `tracing::error!`.
- The log file path is surfaced in the overlay so deeper investigation (`tail -f /tmp/forgiven.log`) remains easy.
- Ring buffer snapshot on every `Mode::Diagnostics` render is a clone of at most 50 small strings — negligible cost.
