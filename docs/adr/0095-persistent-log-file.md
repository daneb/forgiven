# ADR 0095 — Persistent Log File at XDG Data Directory

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

Since the editor's initial logging setup, the log file has been opened with:

```rust
let log_file = std::fs::File::create("/tmp/forgiven.log")?;
```

`File::create` truncates the file to zero bytes on every open. This means:

1. **Every editor restart wipes the log.** Any `[ctx]`, `[usage]`, `[llmlingua]`
   or WARN/ERROR lines from the previous session are permanently lost.

2. **The file lives in `/tmp`.** On macOS, `/tmp` is a symlink to a
   per-boot directory under `/private/var/folders/…` that the OS cleans
   periodically. Even if the file were opened in append mode, it would not
   survive an OS restart.

ADR 0087 added structured `[ctx]` and `[usage]` log lines specifically to
enable post-hoc diagnosis of token usage. ADR 0092 added a separate JSONL
metrics file for structured session data. But the full prose log — which
includes WARN messages, LLMLingua compression ratios, model-switch events, and
LSP/MCP startup details — remained ephemeral.

---

## Decision

Move the log file to the **XDG data directory** and open it in **append mode**.

### Path

`$XDG_DATA_HOME/forgiven/forgiven.log`, falling back to
`$HOME/.local/share/forgiven/forgiven.log` when `XDG_DATA_HOME` is not set.

This is the same directory used by the JSONL metrics log (ADR 0092), so both
files are co-located:

```
~/.local/share/forgiven/
├── forgiven.log        ← full prose log (all levels, append mode)
└── sessions.jsonl      ← structured per-invocation metrics (ADR 0092)
```

### Append mode

`OpenOptions::new().create(true).append(true)` instead of `File::create`.
Each editor startup appends to the existing file. Session boundaries are
distinguishable by timestamp — the `tracing` subscriber writes ISO-formatted
timestamps on every line. Alternatively, the startup log line
`"Starting forgiven"` (already emitted by `main.rs`) serves as a session marker.

### Fallback

If `$HOME` is not set (unusual; container environments without a home directory),
the path function returns `None` and `main.rs` falls back to
`/tmp/forgiven.log`. This matches the previous behaviour exactly.

### Directory creation

`std::fs::create_dir_all(parent)` is called before opening the file, mirroring
the pattern used by `Config::save()` and `append_session_metric()`.

---

## Implementation

### `src/config/mod.rs`

New associated function on `Config`:

```rust
pub fn log_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".local/share")
    };
    Some(base.join("forgiven").join("forgiven.log"))
}
```

This mirrors the existing `Config::config_path()` pattern (XDG_CONFIG_HOME +
fallback to `~/.config`) for consistency.

### `src/main.rs`

Replace:

```rust
let log_file = std::fs::File::create("/tmp/forgiven.log")?;
```

With:

```rust
let log_path = Config::log_path()
    .unwrap_or_else(|| std::path::PathBuf::from("/tmp/forgiven.log"));
if let Some(parent) = log_path.parent() {
    let _ = std::fs::create_dir_all(parent);
}
let log_file = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)?;
```

### `src/editor/mod.rs`

The hardcoded `"/tmp/forgiven.log"` string in the `DiagnosticsData` construction
(shown in `SPC d`) is updated to `"~/.local/share/forgiven/forgiven.log"` so the
overlay reflects the actual file location. A future improvement could pass the
resolved `PathBuf` directly to avoid the tilde shorthand.

---

## Consequences

**Positive**
- Log entries persist across editor restarts. A token-related `WARN` from a
  previous session is still readable when investigating a recurring issue.
- `~/.local/share/forgiven/forgiven.log` and `sessions.jsonl` are co-located,
  making it easy to correlate the prose log with the structured metrics.
- Append mode means the log grows monotonically. `grep '[usage]'` on the full
  log gives a complete history of every token event since the file was created.
- No new dependencies. No format changes. All existing `[ctx]`, `[usage]`,
  `[llmlingua]`, `[stream]`, `[models]` prefixes are preserved.

**Negative / trade-offs**
- The log grows unboundedly. A heavy user emitting 200 log lines/session across
  500 sessions produces ~50 MB/year. Log rotation is not implemented.
  Users who want to prune can `> ~/.local/share/forgiven/forgiven.log` or
  delete the file; a fresh one is created on next startup.
- The `SPC d` overlay shows `"~/.local/share/forgiven/forgiven.log"` as a
  tilde path rather than the fully expanded absolute path. This is cosmetically
  inconsistent but correct for display purposes; the actual path used for
  writing is fully resolved via `$HOME`.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Keep `/tmp` but use append mode | `/tmp` is OS-cleaned; survives restarts within a boot but not across reboots |
| `~/.config/forgiven/forgiven.log` | Config dir is for configuration, not runtime data (XDG spec separates these) |
| Log rotation (keep last N bytes) | Adds complexity; file size is modest enough for manual pruning for now |
| Structured JSON log (replace tracing's text formatter) | Large change; the existing `[prefix]` conventions in the text log are already useful and familiar |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0049](0049-diagnostics-overlay.md) | `SPC d` — displays the log path and last 5 WARN/ERROR entries |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Adds `[ctx]`/`[usage]` log lines that are now durably preserved |
| [0092](0092-persistent-session-metrics-jsonl.md) | JSONL metrics — co-located in `~/.local/share/forgiven/` |
