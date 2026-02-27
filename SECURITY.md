# Security & Privacy

This document explains exactly what forgiven does (and does not do) with your
data and your network connection. It is intended to give you confidence when
running an AI-assisted tool on your codebase.

---

## Reporting a Vulnerability

Please open a [GitHub Issue](https://github.com/danebalia/forgiven/issues) and
label it **security**. For anything sensitive, use GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing/privately-reporting-a-security-vulnerability)
feature instead.

---

## Network Calls

forgiven makes **no background network calls**. Every outbound request is
triggered by a deliberate user action and goes only to GitHub's official
Copilot endpoints.

| # | Endpoint | Method | Triggered by | What is sent |
|---|----------|--------|--------------|--------------|
| 1 | `api.github.com/copilot_internal/v2/token` | GET | First Copilot action per session | Your local GitHub OAuth token (read from the Copilot config file on disk — the same token the official Copilot extension uses) |
| 2 | `api.githubcopilot.com/models` | GET | First `Ctrl+T` press in agent panel | Bearer token only — no code |
| 3 | `api.githubcopilot.com/chat/completions` | POST | Sending a message in the agent panel (`Enter`) | Your chat message + any file context included in the conversation |

### What is NOT sent

- No analytics or telemetry.
- No crash reports or usage metrics.
- No file contents are sent unless you actively use the agent panel and the
  agent reads a file as part of answering your question.
- `telemetry/event` and `$/copilot/openURL` notifications received from
  `copilot-language-server` are explicitly silenced and discarded
  (`src/lsp/mod.rs`).

### Inline completions (ghost text)

Ghost-text completions are handled by the **`copilot-language-server`**
process, which forgiven launches as a child process and communicates with over
stdio (LSP protocol). That server makes its own network calls to GitHub's
Copilot API under the same authentication you already granted when you
installed GitHub Copilot. forgiven does not inspect or proxy those requests.

### Offline / no-Copilot use

All core editor features (file editing, syntax highlighting, undo/redo,
project search, lazygit, LSP diagnostics via `rust-analyzer`) work entirely
offline. Copilot features are silently unavailable when no token is found or
when there is no network connection.

---

## Agent File Access

When the agent calls its built-in tools (`read_file`, `write_file`,
`edit_file`, `list_directory`), every path is validated by `safe_path()`
(`src/agent/tools.rs`) before any disk operation:

- Paths containing `..` are **rejected outright** (no directory traversal).
- All paths are resolved relative to the **project root** you opened — the
  agent cannot read or write files outside that directory tree.

---

## Supply-Chain Security

The CI pipeline runs on every push and pull request:

| Check | Tool | What it catches |
|-------|------|-----------------|
| Dependency CVEs | `cargo-audit` (rustsec/audit-check) | Known vulnerabilities in any dependency |
| License compliance | `cargo-deny` | Copyleft or banned licences sneaking in |
| Code scanning | GitHub Advanced Security | Static analysis of the Rust source |
| Unsafe code | `[lints.rust] unsafe_code = "forbid"` in `Cargo.toml` | Any `unsafe` block anywhere in the codebase |

You can reproduce all of these checks locally:

```bash
make install-tools   # cargo-audit + cargo-deny
make check           # fmt → lint → audit → deny → test
```

---

## Building from Source

The safest way to run forgiven is to build it yourself from the source you
have reviewed:

```bash
git clone https://github.com/danebalia/forgiven
cd forgiven
cargo build --release
./target/release/forgiven
```

The binary has no runtime dependencies beyond the optional tools listed in the
README (`rg`, `lazygit`, `rust-analyzer`, `copilot-language-server`), all of
which are well-known open-source projects.
