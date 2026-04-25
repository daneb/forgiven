# ADR 0140 — Copilot Quota Display and Business Endpoint Correction

**Status:** Accepted

## Context

Forgiven supports GitHub Copilot as a provider via the `copilot_internal/v2/token` OAuth exchange flow. Two problems were identified during investigation of the Copilot Metrics API:

**Problem 1 — Wrong API base URL for Business accounts.**
The token exchange response includes an `endpoints.api` field that points to the correct routing tier for the authenticated account. For Copilot Business / Enterprise seats this is `https://api.business.githubcopilot.com`, not the consumer endpoint `https://api.githubcopilot.com`. All chat completions and model-list calls were being sent to the wrong base URL.

**Problem 2 — No quota visibility.**
The VS Code Copilot extension displays a "Copilot Business Usage" panel showing premium request consumption (e.g. "99.1% used, resets May 1"). This data is available from the undocumented `GET https://api.github.com/copilot_internal/user` endpoint using the same OAuth token that is already loaded for the token exchange. Forgiven had no equivalent visibility.

Investigation also confirmed that the GitHub Copilot GA Metrics API (`/orgs/{org}/copilot/metrics`) requires `read:org` scope, which is not present on the Copilot OAuth token (a zero-scope fine-grained token), making it unsuitable for in-editor use.

## Decision

### 1. Business endpoint correction

`CopilotApiToken` gains a `business_api_url: Option<String>` field populated from `val.pointer("/endpoints/api")` in the token exchange response. `AgentPanel` stores this as `copilot_api_base: String` (default: `https://api.githubcopilot.com`). On every token refresh the field is updated from the token response.

All three `chat/completions` call sites in `panel.rs` and the `fetch_models` call in `models.rs` are updated to use `self.copilot_api_base` / the passed base URL instead of a hardcoded string. `fetch_models` and `fetch_models_for_provider` gain a `copilot_api_base: &str` parameter.

### 2. Quota fetch

A new `pub async fn fetch_copilot_quota(oauth_token: &str) -> Option<CopilotQuota>` function in `auth.rs` calls `copilot_internal/user` and extracts the `quota_snapshots.premium_interactions` object:

| Field extracted | Meaning |
|---|---|
| `percent_remaining` | Percentage of premium interactions *remaining* |
| `remaining` | Absolute remaining count |
| `entitlement` | Included interactions for the billing period |
| `overage_permitted` | Whether usage above entitlement is allowed |
| `overage_count` | Overage interactions consumed |
| `quota_reset_date` (top-level) | Reset date string (e.g. `"2026-05-01"`) |

Quota is fetched immediately after each token refresh inside `ensure_token` using the OAuth token (before it goes out of scope). Result is stored as `AgentPanel.copilot_quota: Option<CopilotQuota>`. Since the Copilot token refreshes every ~25 minutes, quota is implicitly refreshed at the same cadence — once per billing period reset is more than sufficient.

Failures are silent (`Option::None`): the endpoint is undocumented and may be withdrawn without notice.

### 3. Insights panel display

`InsightsDashboardState` gains a `copilot_quota: Option<CopilotQuota>` field. When `SPC a I` opens the dashboard, the current quota snapshot is cloned from `AgentPanel` and passed in. The Summary tab renders a new "Copilot Premium Requests" section:

- A 30-character fill bar, green → yellow → red at 70 % / 90 % consumed
- Used / Included count, remaining absolute, overage line (when permitted), reset date

The section is suppressed entirely when `copilot_quota` is `None` (non-Copilot provider or fetch failure).

### Search model filter (companion fix)

The Copilot `/models` API began returning search-capability models (names like "Search A", "Search B"). These were slipping through the chat/agent capability filter because they had no `capabilities.type` field. Added `id.contains("search")` to the explicit exclusion list alongside `embed`, `whisper`, `tts`, and `dall`.

## Consequences

- **Correctness for Business accounts.** All API traffic now routes through `api.business.githubcopilot.com` when the token response specifies it, matching VS Code behaviour.
- **Quota visibility.** Users can see premium request consumption inside forgiven without leaving the editor.
- **Undocumented endpoint risk.** `copilot_internal/user` is not part of GitHub's public REST API. GitHub may change or remove it. The implementation is fully defensive — a 4xx/5xx response silently leaves `copilot_quota` as `None` with no user-visible error.
- **No additional auth required.** The same OAuth token already in use for the token exchange is reused. No new scopes, no new config fields.
- **Non-Copilot providers unaffected.** `copilot_api_base` defaults to the consumer URL and is only updated for Copilot provider sessions. All other providers continue to use their own endpoints unchanged.

## References

- `src/agent/auth.rs` — `CopilotQuota`, `fetch_copilot_quota`, `CopilotApiToken.business_api_url`
- `src/agent/models.rs` — `fetch_models(base_url)`, `fetch_models_for_provider` signature
- `src/agent/panel.rs` — `copilot_api_base`, `copilot_quota` fields, `ensure_token` quota fetch
- `src/insights/panel.rs` — Summary tab quota section
- ADR 0129 — Insights Dashboard (original)
