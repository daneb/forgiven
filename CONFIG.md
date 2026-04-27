# forgiven — Configuration Reference

Config file location: **`~/.config/forgiven/config.toml`**
(respects `$XDG_CONFIG_HOME` if set)

The file is TOML. All keys are optional — missing keys fall back to the defaults shown below. The editor writes this file itself when you change the model via `Ctrl+T`.
---

## Top-level keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `tab_width` | integer | `4` | Width of a tab stop in columns |
| `use_spaces` | bool | `true` | Insert spaces when Tab is pressed; `false` = insert a literal tab |
| `default_copilot_model` | string | `"gpt-4o"` | Copilot model used for the agent panel and inline completions. Must be a valid model ID returned by the Copilot models API. Changed at runtime with `Ctrl+T`. |
| `max_agent_rounds` | integer | `20` | Maximum agentic tool-calling rounds before the user is prompted to continue or stop |
| `agent_warning_threshold` | integer | `3` | Warn when this many rounds remain before hitting `max_agent_rounds` |

### Example

```toml
tab_width               = 4
use_spaces              = true
default_copilot_model   = "gpt-5.2"
max_agent_rounds        = 20
agent_warning_threshold = 3
```

---

## `[agent]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `spec_framework` | string | `""` | Slash-command prompt framework loaded into the agent panel |

### `spec_framework` values

| Value | Effect |
|-------|--------|
| `""` or `"none"` | Disabled — no slash-command interception |
| `"open-spec"` | Built-in OpenSpec spec-driven workflow (3 commands below) |
| `/absolute/path/to/dir` | Custom framework: any directory of `.md` files; the file stem becomes the command name |

#### Built-in OpenSpec slash commands

| Command | Purpose |
|---------|---------|
| `/openspec.propose` | Elicit requirements; writes `proposal.md`, `design.md`, `tasks.md` |
| `/openspec.review` | Audit artefacts; produce gap report before implementation |
| `/openspec.apply` | Execute tasks in order; archive completed change |

### Example

```toml
[agent]
spec_framework = "open-spec"
```

---

## `[lsp]` — Language Server Protocol

Defines language servers the editor will connect to on startup. The editor connects to a server when a file whose extension matches `language` is opened.

### `[[lsp.servers]]` fields

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `language` | string | yes | Language ID — must match one of the values in the table below |
| `command` | string | yes | Executable name or absolute path (resolved via `$PATH`) |
| `args` | array of strings | no | Arguments passed to the executable |
| `env` | inline table | no | Environment variables injected into the server process. Values beginning with `$` are resolved from the shell environment at startup |
| `initialization_options` | table | no | Arbitrary LSP `initializationOptions` forwarded to the server's `initialize` request. Merged with built-in defaults; user values win |

### Supported language IDs

| `language` value | File extensions |
|-----------------|-----------------|
| `"rust"` | `.rs` |
| `"python"` | `.py` |
| `"javascript"` | `.js` |
| `"typescript"` | `.ts`, `.tsx` |
| `"go"` | `.go` |
| `"c"` | `.c` |
| `"cpp"` | `.cpp`, `.cc`, `.cxx` |
| `"java"` | `.java` |
| `"ruby"` | `.rb` |
| `"sh"` | `.sh` |
| `"markdown"` | `.md` |
| `"json"` | `.json` |
| `"yaml"` | `.yaml`, `.yml` |
| `"toml"` | `.toml` |

### Examples

```toml
[lsp]

[[lsp.servers]]
language = "rust"
command  = "rust-analyzer"
args     = []

[[lsp.servers]]
language = "typescript"
command  = "typescript-language-server"
args     = ["--stdio"]

[[lsp.servers]]
language = "python"
command  = "pyright-langserver"
args     = ["--stdio"]

# Custom env — useful when multiple toolchains coexist
[[lsp.servers]]
language = "rust"
command  = "rust-analyzer"
env      = { RUSTUP_TOOLCHAIN = "stable" }

# Custom initialization_options (e.g. OmniSharp for C#)
[[lsp.servers]]
language = "csharp"
command  = "OmniSharp"
args     = ["-lsp"]
[lsp.servers.initialization_options.RoslynExtensionsOptions]
documentAnalysisTimeoutMs = 60000
enableImportCompletion    = true
```

---

## `[mcp]` — Model Context Protocol servers

MCP servers give the agent panel access to external tools (filesystem, git, web search, Jira, etc.). Each server is a child process that speaks the JSON-RPC 2.0 MCP protocol over stdio. Servers are started in parallel at editor startup (see ADR 0053).

### `[[mcp.servers]]` fields

| Key | Type | Required | Description |
|-----|------|----------|-------------|
| `name` | string | yes | Human-readable name shown in the agent panel status bar |
| `command` | string | yes | Executable to spawn (`npx`, `uvx`, `docker`, or an absolute path) |
| `args` | array of strings | no | Arguments passed to the executable |
| `env` | inline table | no | Environment variables for the server process. Values beginning with `$` are resolved from the shell environment (see [Secret resolution](#secret-resolution)) |

### Secret resolution

Any env value that starts with `$` is treated as a reference to the host shell environment:

```toml
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_PERSONAL_ACCESS_TOKEN" }
```

The actual token is read from your shell profile at startup — it is **never stored in the config file**. If the variable is unset, an empty string is passed and a warning appears in the diagnostics overlay (`SPC d`).

Export secrets in `~/.zshrc` or `~/.bashrc`:

```sh
export GITHUB_PERSONAL_ACCESS_TOKEN="ghp_..."
export SEARXNG_URL="http://localhost:8080"
```

### Connection status

Connected servers are shown **green** in the agent panel title bar. Failed servers are shown **red with ⚠**. Full error details are available in the diagnostics overlay (`SPC d`).

### Examples

```toml
[mcp]

# ── GitHub (Docker) ───────────────────────────────────────────────────────────
[[mcp.servers]]
name    = "github"
command = "docker"
args    = ["run", "--rm", "-i", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN",
           "ghcr.io/github/github-mcp-server"]
env     = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_PERSONAL_ACCESS_TOKEN" }

# ── Git (local, via uvx) ──────────────────────────────────────────────────────
[[mcp.servers]]
name    = "git"
command = "uvx"
args    = ["mcp-server-git"]

# ── Web search via SearXNG ────────────────────────────────────────────────────
# SearXNG must be running before the editor starts.
# OrbStack users: the domain is http://<container>.<stack>.orb.local
# Standard Docker: use http://localhost:<port>
[[mcp.servers]]
name    = "search"
command = "uvx"
args    = ["mcp-server-searxng", "--url", "http://searxng.searxng-mcp.orb.local"]

# ── Atlassian Rovo — Jira, Confluence, Compass (OAuth 2.1) ───────────────────
# No credentials in config. mcp-remote opens a browser on first connect.
[[mcp.servers]]
name    = "atlassian"
command = "npx"
args    = ["-y", "mcp-remote", "https://mcp.atlassian.com/v1/mcp"]

# ── Filesystem (npx) ─────────────────────────────────────────────────────────
# Restrict to specific directories for safety.
[[mcp.servers]]
name    = "filesystem"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/projects"]
```

---

## `[sidecar]` — Companion window

Controls the Tauri companion preview window that renders Markdown and Mermaid diagrams alongside the TUI.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `auto_launch` | bool | `false` | Spawn the companion automatically on editor startup. Opt-in: most users won't want a second window every session. |
| `binary_path` | string | *(none)* | Absolute path to the `forgiven-companion` binary (or the inner executable inside a `.app` bundle on macOS). Omit to search `$PATH`. |

### Example

```toml
[sidecar]
auto_launch = true
binary_path = "/Applications/Forgiven Previewer.app/Contents/MacOS/forgiven-companion"
```

Toggle the companion at runtime without restarting the editor: **`SPC p c`**.

---

## Diagnostics overlay

Press **`SPC d`** at any time to open the diagnostics overlay. It shows:

- MCP server connection status and full error chain for any failures
- Active LSP servers and their state
- Recent `WARN` / `ERROR` log lines (last 50, captured at startup)

This is the primary debugging tool for config problems.

---

## Full annotated example

```toml
# ~/.config/forgiven/config.toml

tab_width               = 4
use_spaces              = true
default_copilot_model   = "gpt-5.2"
max_agent_rounds        = 20
agent_warning_threshold = 3

[agent]
spec_framework = "open-spec"

[lsp]

[[lsp.servers]]
language = "rust"
command  = "rust-analyzer"
args     = []

[mcp]

[[mcp.servers]]
name    = "github"
command = "docker"
args    = ["run", "--rm", "-i", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN",
           "ghcr.io/github/github-mcp-server"]
env     = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_PERSONAL_ACCESS_TOKEN" }

[[mcp.servers]]
name    = "git"
command = "uvx"
args    = ["mcp-server-git"]

[[mcp.servers]]
name    = "search"
command = "uvx"
args    = ["mcp-server-searxng", "--url", "http://searxng.searxng-mcp.orb.local"]

[[mcp.servers]]
name    = "atlassian"
command = "npx"
args    = ["-y", "mcp-remote", "https://mcp.atlassian.com/v1/mcp"]
```
