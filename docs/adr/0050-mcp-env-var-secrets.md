# ADR 0050: MCP Server Environment Variable Secret Resolution

**Date:** 2026-03-07
**Status:** Accepted

## Context

MCP server configs can require secrets (e.g. `GITHUB_PERSONAL_ACCESS_TOKEN`) passed as
environment variables to the spawned server process. The initial implementation stored
these values literally in `~/.config/forgiven/config.toml`:

```toml
[[mcp.servers]]
name = "github"
command = "docker"
args = ["run", "--rm", "-i", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "ghp_actual_secret_here" }
```

This has two problems:

1. **Plaintext secrets on disk** — the config file is world-readable by default and may
   be accidentally committed to version control.
2. **No indirection** — rotating a token requires editing the config file rather than
   updating a single shell export.

Additionally, the Atlassian Rovo remote MCP server (`https://mcp.atlassian.com/v1/mcp`)
was not yet configured. It uses an SSE-over-HTTPS transport managed by `mcp-remote`, which
handles OAuth 2.1 and does not require any secret in the config file at all.

## Decision

### `$VAR_NAME` expansion in `spawn_and_init` (`src/mcp/mod.rs`)

When iterating over `cfg.env` to set environment variables on the child process, values
that begin with `$` are treated as references to the current process environment rather
than literal strings:

```rust
let resolved = if let Some(var_name) = v.strip_prefix('$') {
    std::env::var(var_name).unwrap_or_else(|_| {
        warn!(
            "MCP server '{}': env var ${} is not set in the shell environment",
            cfg.name, var_name
        );
        String::new()
    })
} else {
    v.clone()
};
cmd.env(k, resolved);
```

- Only leading `$` is treated as an indirection marker; values without `$` are passed
  through unchanged (backwards-compatible).
- If the referenced variable is not set, an empty string is passed and a `WARN` tracing
  event is emitted (visible in the diagnostics overlay via `SPC d`).
- No shell-style brace expansion (`${VAR}`), default values, or nested substitution —
  simple prefix stripping is sufficient and avoids re-implementing a shell parser.

### Config convention

Secrets are stored as `$VAR_NAME` in the config and exported from the user's shell
profile (`~/.zshrc`, `~/.bashrc`, etc.):

```toml
[[mcp.servers]]
name = "github"
command = "docker"
args = ["run", "--rm", "-i", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_PERSONAL_ACCESS_TOKEN" }
```

```sh
# ~/.zshrc
export GITHUB_PERSONAL_ACCESS_TOKEN="ghp_..."
```

### Atlassian Rovo MCP server

Added via `mcp-remote`, which proxies the remote SSE endpoint to stdio so the existing
stdio-only `McpManager` requires no transport changes:

```toml
[[mcp.servers]]
name = "atlassian"
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.atlassian.com/v1/mcp"]
```

Authentication is OAuth 2.1: on first connection `mcp-remote` opens a browser window for
the user to authorise access. The resulting session token is cached locally by
`mcp-remote`; no credential ever appears in the Forgiven config file.

## Consequences

- **Secrets are never stored in `config.toml`** — the file can be safely committed or
  shared without leaking credentials.
- **Token rotation** requires only updating the shell export and restarting the editor;
  the config file is untouched.
- **Backwards-compatible** — existing configs with literal values (no leading `$`) work
  unchanged.
- **Visible failure** — a missing env var produces a `WARN` log visible via `SPC d`
  rather than silently passing an empty or wrong value.
- **Atlassian tools** (Jira, Confluence, Compass) are now available to the agentic loop
  via the Rovo MCP server with no config-level secrets.
- The `mcp-remote` OAuth cache lives in the user's home directory (managed by `mcp-remote`
  internally); clearing it forces re-authorisation on the next editor start.
