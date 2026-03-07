# ADR 0051: Startup Loading Indicator and Service Parallelisation

**Date:** 2026-03-07
**Status:** Accepted

## Context

Forgiven's startup sequence initialised services synchronously and gave no visual feedback while doing so:

1. `Editor::new()` blocked on `Highlighter::new()` (~50 ms, loads syntect grammars/themes) before the terminal was even drawn.
2. `setup_lsp()` initialised each LSP server sequentially — `rust-analyzer` then the Copilot `npx` server — waiting for each `initialize` handshake to complete before starting the next.
3. `setup_mcp()` then ran, again sequentially, connecting to each MCP server one at a time.
4. LSP setup completed before MCP setup began; the two phases were never concurrent.

With a typical config (2 LSP servers, 5 MCP servers via `npx`/`docker`), the measured startup cost was:

| Phase | Time |
|---|---|
| `Editor::new()` | ~80 ms |
| LSP (2 servers, sequential) | ~1.5 s |
| MCP (5 servers, sequential) | ~4 s |
| **Total** | **~5.5 s** |

During this entire period the screen was blank — the alternate terminal screen was entered but nothing was drawn until `run()` started its event loop.

## Decision

### 1. Loading screen (`Editor::render_loading`)

A new method `render_loading(&mut self, msg: &str) -> Result<()>` on `Editor` draws a frame immediately to the already-open alternate screen.  The frame shows the cross and wordmark (identical to the welcome screen) with a dim status line beneath:

```
starting services…
```

This is called once from `main()` before `setup_services()` so the user sees the branding immediately while services connect in the background.

### 2. Startup elapsed time on the welcome screen

A field `pub startup_elapsed: Option<std::time::Duration>` is added to `Editor`.  `main()` records `Instant::now()` before `Editor::new()` and writes the elapsed duration into `editor.startup_elapsed` after `setup_services()` completes.

`UI::render_welcome()` reads this value and appends a dim `"ready in X ms"` line below the keyboard hints when `startup_elapsed` is `Some`.  The welcome screen is the first thing the user sees after opening the editor without arguments, so this is a natural place for a one-time timing report.

### 3. Parallel LSP server connections (`lsp::init_servers_parallel`)

A new standalone async function in `src/lsp/mod.rs`:

```rust
pub async fn init_servers_parallel(
    servers: &[LspServerConfig],
    workspace_root: PathBuf,
    notification_tx: mpsc::UnboundedSender<LspNotificationMsg>,
) -> Vec<(String, Result<LspClient>)>
```

It uses `tokio::task::JoinSet` to spawn one task per server.  Each task calls `LspClient::spawn` (synchronous process fork) followed by `client.initialize()` (async handshake).  Results are collected into index-ordered slots to preserve config ordering, then flattened into `Vec<(language, Result<LspClient>)>` for the caller to apply.

Two helper methods are added to `LspManager`:
- `notification_tx() -> UnboundedSender<…>` — clones the shared sender so tasks can each receive notifications independently.
- `insert_client(language, client)` — inserts an already-initialised client, used after parallel startup.

The old sequential `add_server` method is removed.

### 4. Parallel MCP server connections (`McpManager::from_config`)

`McpManager::from_config` is rewritten to use `JoinSet`:

```rust
let mut join_set: JoinSet<(usize, Result<(McpServer, Child)>)> = JoinSet::new();
for (idx, cfg) in configs.iter().enumerate() {
    let cfg = cfg.clone();
    join_set.spawn(async move { (idx, spawn_and_init(&cfg).await) });
}
```

Results are collected into a `Vec<Option<…>>` indexed by original position so the `tool_map` indices (which reference into the `servers` Vec) remain stable regardless of completion order.

### 5. Concurrent LSP + MCP via `Editor::setup_services`

`setup_lsp` and `setup_mcp` previously both took `&mut self`, making `tokio::join!` impossible without two simultaneous mutable borrows.  The solution is a new `setup_services` method that:

1. Clones the config data it needs (`lsp_servers`, `mcp_servers`, `notif_tx`) — no borrows held during the async work.
2. Calls `tokio::join!` on `init_servers_parallel` and `McpManager::from_config` concurrently.
3. Applies both results to `self` sequentially after both futures complete.

```rust
pub async fn setup_services(&mut self) {
    let lsp_servers = self.config.lsp.servers.clone();
    let mcp_servers = self.config.mcp.servers.clone();
    let notif_tx    = self.lsp_manager.notification_tx();

    let (lsp_results, mcp_manager) = tokio::join!(
        crate::lsp::init_servers_parallel(&lsp_servers, workspace_root, notif_tx),
        McpManager::from_config(&mcp_servers),
    );

    // apply LSP …
    // apply MCP …
}
```

`main()` is simplified to a single call:

```rust
editor.render_loading("starting services…")?;
editor.setup_services().await;
editor.startup_elapsed = Some(t0.elapsed());
```

## Consequences

### Startup time

| Phase | Before | After |
|---|---|---|
| Visual feedback | none until `run()` | immediate (loading screen) |
| LSP (2 servers) | ~1.5 s sequential | ~1 s parallel |
| MCP (5 servers) | ~4 s sequential | ~1.5 s parallel (slowest server) |
| LSP + MCP | sequential | concurrent |
| **Total** | **~5.5 s** | **~1.5 s** |

The dominant remaining cost is the slowest single server (typically the `atlassian` MCP server, which performs OAuth on first connection, or the `docker`-based GitHub MCP server on a cold Docker start).

### Code structure

- `setup_lsp` and `setup_mcp` are removed; `setup_services` replaces both.
- `LspManager::add_server` is removed; callers use `init_servers_parallel` + `insert_client`.
- The parallel MCP and LSP paths are self-contained and non-breaking — error handling per server is unchanged (failures are non-fatal, logged, and shown in the diagnostics overlay via `SPC d`).

### Trade-offs

- **Log interleaving** — concurrent server init means log lines from different servers appear interleaved in `/tmp/forgiven.log`. Each line already includes the server name, so this is readable.
- **Ordering** — config ordering is preserved for both LSP clients (inserted into `HashMap` by language key) and MCP servers (re-sorted by original index before building `tool_map`).
- **No timeout change** — the existing 10-second LSP `initialize` timeout is unchanged; the MCP `spawn_and_init` has no explicit timeout (relies on OS process termination). This is unchanged from before.
