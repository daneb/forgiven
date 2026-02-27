# ADR 0004 — GitHub Copilot Enterprise Authentication

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

Copilot Enterprise requires an authenticated session to serve completions and chat
responses. There are two distinct auth surfaces:

1. **LSP auth** — the `copilot-language-server` process itself authenticates via a
   GitHub device-flow that it surfaces through `window/showMessage` LSP notifications
2. **HTTP auth** — the agent chat panel calls the Copilot REST API directly and needs
   a short-lived bearer token

## Decision

### LSP-side auth (device flow)

`copilot-language-server` manages its own credential store at
`~/.config/github-copilot/apps.json`. On first run it sends `window/showMessage`
notifications containing the GitHub device-flow URL and user code.

forgiven handles this by:
- Adding a `ShowMessage { message: String }` variant to `LspNotificationMsg`
- Handling `"window/showMessage"` and `"window/showMessageRequest"` in the reader thread
- Surfacing these as **sticky status messages** (stored in `Editor.status_sticky`) that
  persist until the user presses Esc, so the auth URL is visible long enough to copy
- Providing a `:copilot auth` command that manually triggers `checkStatus` →
  `signInInitiate` if the automatic flow stalls

The editor polls Copilot's `checkStatus` during the `run()` loop and automatically
escalates `NotSignedIn` responses to `signInInitiate`, displaying the resulting URL and
user code on the status line.

### HTTP-side auth (agent panel)

The agent panel (`src/agent/mod.rs`) manages its own token lifecycle independently of
the LSP:

```
~/.config/github-copilot/apps.json
    └─► oauth_token (ghu_…)
            │
            ▼
GET api.github.com/copilot_internal/v2/token
    Authorization: token <oauth_token>
            │
            ▼
    { "token": "tid=…", "expires_at": "2024-…" }
            │
            ▼
POST api.githubcopilot.com/chat/completions
    Authorization: Bearer <copilot_api_token>
```

The short-lived Copilot API token is cached in `AgentPanel.token: Option<CopilotApiToken>`
and refreshed 60 seconds before expiry via `ensure_token()`.

`expires_at` is an ISO-8601 timestamp. To avoid pulling in `chrono` as a dependency,
a bespoke `chrono_unix_from_iso()` parser converts it to a Unix timestamp using only
the standard library.

### Required HTTP headers for Copilot API

The Copilot API requires specific headers or it rejects requests:

```
Copilot-Integration-Id: vscode-chat
editor-version: forgiven/0.1.0
editor-plugin-version: forgiven-copilot/0.1.0
openai-intent: conversation-panel
```

## Consequences

- Users must authenticate with `copilot-language-server` at least once (device flow)
  before the agent panel can exchange tokens
- The OAuth token in `apps.json` is long-lived; the derived Copilot API token is
  short-lived (~30 min) and auto-refreshed
- No secrets are stored by forgiven itself; it reads credentials written by the
  official Copilot tooling
- The token exchange endpoint (`copilot_internal/v2/token`) is an internal GitHub API
  that other third-party Copilot integrations also use
- Parsing failures during token exchange log the full raw response body at DEBUG level
  to aid diagnosis
