# ADR 0116 — Multi-Provider LLM Backend (Anthropic, OpenAI, Gemini, OpenRouter)

**Date:** 2026-04-06
**Status:** Accepted — Implemented

---

## Context

forgiven supports two LLM backends today: GitHub Copilot (ADR 0001) and a local
Ollama server (ADR 0098).  The roadmap identifies four additional providers as
missing:

- **Anthropic direct API** — first-class access to Claude models without Copilot
  as intermediary; useful when the user has an Anthropic API key but not a Copilot
  Enterprise seat.
- **OpenAI API** — access to GPT-4o, o3, and future OpenAI models directly;
  also covers Azure OpenAI deployments via `base_url` override.
- **Google Gemini** — access to Gemini 2.5 Pro / Flash; competitive on long-context
  tasks (1 M token window).
- **OpenRouter** — aggregates 300+ models from multiple providers behind a single
  key; useful for model comparison and fallback routing.

All four providers expose an **OpenAI-compatible `/v1/chat/completions` endpoint**
with identical SSE wire format.  The entire SSE parser, streaming header, tool-call
loop, and token-usage extraction in `agentic_loop.rs` work unchanged — zero new
parsing code is required.

The existing `ProviderKind` + `ProviderSettings` abstraction (ADR 0098) was
explicitly designed for this expansion: "making it straightforward to add a third
provider by adding a new `ProviderKind` variant."

---

## Decision

### Four new `ProviderKind` variants

```rust
pub enum ProviderKind {
    Copilot,      // existing
    Ollama,       // existing
    Anthropic,    // new — api.anthropic.com OpenAI-compat layer
    OpenAi,       // new — api.openai.com  (or custom base_url for Azure)
    Gemini,       // new — generativelanguage.googleapis.com OpenAI-compat
    OpenRouter,   // new — openrouter.ai aggregator
}
```

All four new variants share the same cloud-provider profile:

| Parameter | Value |
|-----------|-------|
| `requires_auth()` | `true` |
| `supports_stream_usage()` | `true` |
| `connect_timeout_secs()` | 15 |
| `chunk_timeout_secs()` | 60 |
| `max_retries()` | 5 |

Tool calling and planning tools default to `true` for all four — each provider's
OpenAI-compat layer supports structured `tool_calls` deltas.

### API key handling — environment variable expansion

API keys are **never stored in plaintext** in `config.toml`.  The same `$VAR`
expansion pattern used for MCP env vars (ADR 0050) is applied to each provider's
`api_key` field at startup:

```toml
[provider.anthropic]
api_key = "$ANTHROPIC_API_KEY"   # resolved from shell env at startup
```

If the env var is unset or empty, the editor logs a warning and falls back to an
empty token — the first API call will fail with a 401, surfacing the
misconfiguration clearly.

A `resolve_api_key(raw: &str) -> String` helper in `src/agent/provider.rs`
performs the expansion (matches `^$[A-Z_][A-Z0-9_]*$`, looks up via
`std::env::var`).

### Config schema

```toml
[provider]
active = "anthropic"   # "copilot" | "ollama" | "anthropic" | "openai" | "gemini" | "openrouter"

[provider.anthropic]
api_key       = "$ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-5"

[provider.openai]
api_key       = "$OPENAI_API_KEY"
default_model = "gpt-4o"
# base_url omitted → uses "https://api.openai.com/v1"
# Override for Azure: base_url = "https://MY-DEPLOYMENT.openai.azure.com/openai/deployments/MY-MODEL"

[provider.gemini]
api_key       = "$GEMINI_API_KEY"
default_model = "gemini-2.5-pro"

[provider.openrouter]
api_key            = "$OPENROUTER_API_KEY"
default_model      = "anthropic/claude-sonnet-4-5"
# site_url / app_name forwarded as X-Title / HTTP-Referer per OpenRouter etiquette
site_url           = "https://github.com/user/forgiven"
app_name           = "forgiven"
```

Existing `[provider.copilot]` and `[provider.ollama]` blocks are unaffected.

### Endpoints

| Provider | Chat endpoint |
|----------|---------------|
| Anthropic | `https://api.anthropic.com/v1/chat/completions` |
| OpenAI | `https://api.openai.com/v1/chat/completions` (or `{base_url}/chat/completions`) |
| Gemini | `https://generativelanguage.googleapis.com/v1beta/openai/chat/completions` |
| OpenRouter | `https://openrouter.ai/api/v1/chat/completions` |

All four use `Authorization: Bearer {api_key}` — no custom headers beyond what
`start_chat_stream_with_tools` already sends for Copilot are needed for the core
request.  OpenRouter additionally sends `HTTP-Referer` and `X-Title` headers (if
`site_url` / `app_name` are configured) to identify the client per OpenRouter's
documentation.

### Model discovery

| Provider | Strategy |
|----------|----------|
| Anthropic | Static list — the compat endpoint's `/models` is unreliable; hardcode the canonical model IDs and context windows |
| OpenAI | `GET {base_url}/models` — filter to chat-capable models; context window from a static lookup table (the API does not return it) |
| Gemini | `GET generativelanguage.googleapis.com/v1beta/openai/models` — compat layer returns a model list |
| OpenRouter | `GET openrouter.ai/api/v1/models` — returns full catalogue with `context_length` per model |

Static Anthropic model list (seeded at implementation time; updated as new models
release):

| Model ID | Context |
|----------|---------|
| `claude-opus-4-6` | 200 000 |
| `claude-sonnet-4-6` | 200 000 |
| `claude-haiku-4-5-20251001` | 200 000 |

`fetch_models_for_provider(kind, settings) -> Vec<ModelInfo>` in `models.rs`
dispatches to the appropriate strategy.

### `ProviderSettings` — no new fields required

The existing struct carries everything the HTTP layer needs:

```rust
pub struct ProviderSettings {
    pub kind:                ProviderKind,
    pub api_token:           String,   // resolved at submit() time
    pub chat_endpoint:       String,
    pub num_ctx:             Option<u32>,   // None for all new providers
    pub supports_tool_calls: bool,
    pub planning_tools:      bool,
}
```

`num_ctx` (the Ollama-specific KV-cache override) is `None` for all four new
providers — the field is omitted from the request body when `None`.

### Panel labels and emoji

| Provider | Human turn | AI turn | Panel title |
|----------|-----------|---------|-------------|
| Anthropic | `🧑 You` | `🟠 Claude` | `Anthropic [model]` |
| OpenAI | `🧑 You` | `🟢 GPT` | `OpenAI [model]` |
| Gemini | `🧑 You` | `🔵 Gemini` | `Gemini [model]` |
| OpenRouter | `🧑 You` | `🌐 <model-base>` | `OpenRouter [model]` |

The streaming header (`╔ 🟠 Claude ▋`) uses `ai_emoji()` + `ai_label_name()` as
with the existing providers — no UI code changes beyond updating these methods.

---

## Implementation

### Modified files

| File | Change |
|------|--------|
| `src/agent/provider.rs` | Added `Anthropic`, `OpenAi`, `Gemini`, `OpenRouter` to `ProviderKind`; `from_str`, `display_name`, `ai_emoji`, `requires_auth`, `supports_stream_usage`, timeout/retry methods updated; `resolve_api_key()` helper added; `openrouter_site_url` / `openrouter_app_name` fields added to `ProviderSettings`; `user_emoji()` collapsed to a single value (all providers show `🧑`) |
| `src/config/mod.rs` | Added `AnthropicProviderConfig`, `OpenAiProviderConfig`, `GeminiProviderConfig`, `OpenRouterProviderConfig`; added fields to `ProviderConfig`; `active_default_model()` extended with four new arms |
| `src/agent/models.rs` | Added `fetch_models_anthropic()` (static list, 3 models), `fetch_models_openai()` (dynamic with static context-window lookup), `fetch_models_gemini()` (dynamic via compat endpoint), `fetch_models_openrouter()` (full catalogue with `context_length`), and `fetch_models_for_provider()` dispatcher; `use super::provider::ProviderKind` import added |
| `src/agent/mod.rs` | Added `api_key`, `openai_base_url`, `openrouter_site_url`, `openrouter_app_name` fields to `AgentPanel` |
| `src/agent/panel.rs` | Import swapped to `fetch_models_for_provider`; `AgentPanel::new()` initialises new fields; `ai_label_name()` extended; `ensure_token()` rewritten with exhaustive match across all six variants; `ensure_models()`, `refresh_models()`, and the inline model-fetch in `submit()` all use the dispatcher; `ProviderSettings` construction in `submit()` and `start_inline_assist()` handles all six endpoints and sets `openrouter_*` fields; `use_planning` match exhaustive |
| `src/editor/mod.rs` | Startup block resolves API keys via `resolve_api_key()` for `Anthropic` / `OpenAi` / `Gemini` / `OpenRouter`; sets `openai_base_url`, `openrouter_site_url`, `openrouter_app_name` on the panel |
| `src/editor/ai.rs` | `one_shot_with_provider()` signature extended with `api_key`, `openai_base_url`, `openrouter_site_url`, `openrouter_app_name`; four new match arms; OpenRouter `HTTP-Referer` / `X-Title` headers injected; both call sites updated to pass new parameters |
| `src/agent/agentic_loop.rs` | OpenRouter `HTTP-Referer` / `X-Title` headers added in the streaming request builder |
| `src/ui/agent_panel.rs` | Two `match panel.provider` blocks (message header colour and streaming header colour) extended with four new variants: `Anthropic` → `LightRed`, `OpenAi` → `LightGreen`, `Gemini` → `LightBlue`, `OpenRouter` → `LightCyan` |

### New files

None.  All changes extend existing files.

### Deviations from the design

One minor deviation from the ADR spec: `user_emoji()` was simplified to return `"🧑"` for all providers (including the original `Copilot` case, which previously also returned `"🧑"`).  Differentiating the human turn by provider adds no user value and would require updating every panel colour match on each new provider addition.

### `agentic_loop.rs` — unchanged

The SSE parser, tool-call accumulator, retry loop, and token-usage extraction are
identical across all six providers.  The only per-provider branches that already
exist — auth header injection, `stream_options.include_usage`, connect/chunk
timeouts, `num_ctx` injection — remain in `start_chat_stream_with_tools` and
continue to work via `ProviderSettings`.

For OpenRouter, the two additional headers (`HTTP-Referer`, `X-Title`) are injected
alongside the existing headers in `start_chat_stream_with_tools` when
`provider.kind == ProviderKind::OpenRouter` and the config values are non-empty.

---

## Consequences

**Positive**
- Users with Anthropic, OpenAI, or Gemini API keys can use forgiven without a
  Copilot Enterprise seat.
- OpenRouter gives access to 300+ models (including Mistral, Cohere, Perplexity,
  and others) via a single key — useful for model benchmarking.
- Zero changes to the SSE parser or agentic loop — all new providers ride the
  existing code path entirely.
- API keys are never written to disk; `$VAR` expansion matches the MCP pattern
  users already know.
- Azure OpenAI deployments work via `[provider.openai] base_url = ...` — no
  Azure-specific code needed.

**Negative / trade-offs**
- Anthropic's native API supports features not available via the compat layer
  (extended thinking budgets, citations, richer streaming events).  Using the
  compat endpoint forgoes these.  If native Anthropic API features become
  compelling a follow-up ADR can add a `Native` variant with a separate parser.
- Static Anthropic model list requires a code update when new models release.
  OpenRouter's dynamic list is always current.
- OpenAI's `/models` endpoint does not return context-window sizes.  The static
  lookup table will lag new model releases — users can work around by specifying
  `[agent] context_budget_tokens` manually.
- Six `ProviderKind` variants increase the surface of every `match` in
  `provider.rs`.  Each arm is a handful of constants — no logic divergence.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Use Anthropic native API (`/v1/messages`) | Different wire format requires a second SSE parser; deferred until native-only features (extended thinking) justify the cost |
| `async_trait` + `Provider` trait object | Adds a dependency and boxing overhead; an enum dispatching through `ProviderSettings` achieves the same extensibility with zero overhead, consistent with ADR 0098's stated rationale |
| Store API keys in keychain / secret store | Platform-specific APIs; `$VAR` env expansion is simpler, cross-platform, and matches existing MCP pattern (ADR 0050) |
| Per-session provider switching (no restart) | Adds significant UI complexity; session-level selection (restart required) is sufficient for the use case and consistent with ADR 0098 |
| Merge OpenRouter + all providers into a single "gateway" approach | Would require removing direct-provider paths; users may prefer direct endpoints for lower latency and to avoid a third-party intermediary |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0001](0001-terminal-ui-framework.md) | Original Copilot-only decision |
| [0098](0098-ollama-local-provider.md) | Provider abstraction this ADR extends |
| [0050](0050-mcp-env-var-secrets.md) | `$VAR` env expansion pattern reused for API keys |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Token-limit pressure motivating direct-API alternatives to Copilot |
