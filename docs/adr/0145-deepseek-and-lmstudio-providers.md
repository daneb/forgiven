# ADR 0145 — DeepSeek and LM Studio Providers

**Date:** 2026-05-01
**Status:** Accepted

---

## Context

Forgiven shipped with six AI backends: GitHub Copilot, Ollama, Anthropic, OpenAI,
Google Gemini, and OpenRouter (ADR 0087 provider abstraction). Two frequently
requested backends were missing:

**DeepSeek** — a cost-competitive cloud API (`api.deepseek.com`) with strong
coding model performance. Identical wire format to OpenAI: Bearer token auth,
`/v1/chat/completions` SSE streaming, OpenAI-compatible `/v1/models` listing.
Configurable base URL allows self-hosted DeepSeek deployments.

**LM Studio** — a popular local inference server that exposes the same OpenAI
wire format on `localhost:1234`. Users wanting fully offline, privacy-preserving
inference prefer it over Ollama for its GUI model management. Like Ollama it
requires no auth, and tool-calling support is model-dependent so it must be
opt-in.

Both providers fit cleanly into the existing `ChatProvider` trait without any
protocol changes to the streaming parser or agentic loop.

---

## Decision

### New provider files

`src/agent/provider/deepseek.rs` — `DeepSeekProvider { api_key, base_url }`:
- Follows the OpenAI provider pattern exactly.
- `endpoint()` → `"{base_url}/chat/completions"` (configurable for self-hosted).
- `requires_auth()` → `true`; `supports_stream_usage()` → `true`.
- Cloud timeouts: 15 s connect, 60 s chunk, 5 retries.

`src/agent/provider/lmstudio.rs` — `LmStudioProvider { base_url, tool_calls, planning_tools }`:
- Follows the Ollama provider pattern.
- `requires_auth()` → `false`; `supports_stream_usage()` → `false`.
- Local timeouts: 60 s connect (model may need loading), 20 s chunk, 2 retries.
- `tool_calls` and `planning_tools` default `false` — model-dependent, opt-in.

### ProviderKind enum

Two new variants added to `src/agent/provider/mod.rs`:

```rust
DeepSeek,
LmStudio,
```

`from_str()` accepts `"deepseek"` and `"lmstudio"` / `"lm-studio"` / `"lm_studio"`.
All exhaustive `match` blocks updated: `display_name`, `ai_emoji`, timeout and retry
methods, `chat_endpoint`, `build_settings`, `make_provider`.

Local-provider groups extended where applicable:

| Method | Ollama | LmStudio | DeepSeek | Cloud |
|---|---|---|---|---|
| `requires_auth` | false | false | true | true |
| `supports_stream_usage` | false | false | true | true |
| `connect_timeout_secs` | 60 | 60 | 15 | 15 |
| `chunk_timeout_secs` | 20 | 20 | 60 | 60 |
| `max_retries` | 2 | 2 | 5 | 5 |

### Runtime ProviderConfig

Four new fields on `provider::ProviderConfig`:

```rust
pub deepseek_base_url: String,       // default: "https://api.deepseek.com/v1"
pub lmstudio_base_url: String,       // default: "http://localhost:1234/v1"
pub lmstudio_tool_calls: bool,
pub lmstudio_planning_tools: bool,
```

### TOML config

Two new per-provider sections in `src/config/mod.rs`:

```toml
[provider.deepseek]
api_key       = "$DEEPSEEK_API_KEY"
default_model = "deepseek-chat"
base_url      = "https://api.deepseek.com/v1"  # optional override

[provider.lmstudio]
default_model  = ""            # set to the model currently loaded in LM Studio
base_url       = "http://localhost:1234/v1"
tool_calls     = false
planning_tools = false
```

### Model listing

`fetch_models_for_provider` in `src/agent/models.rs` reuses the existing
`fetch_models_openai(api_token, base_url)` helper for both:
- DeepSeek: `fetch_models_openai(api_token, &config.deepseek_base_url)`
- LM Studio: `fetch_models_openai("", &config.lmstudio_base_url)` (no auth needed)

Both servers expose an OpenAI-compatible `GET /models` endpoint.

### One-shot generation

`one_shot_with_provider` in `src/editor/ai.rs` gains match arms for DeepSeek
(Bearer auth, configurable endpoint) and LM Studio (no auth, local endpoint).

### UI

| Provider | `ai_emoji` | `ai_label_name` | Panel colour |
|---|---|---|---|
| DeepSeek | 🔷 | `"DeepSeek"` | `Color::Cyan` |
| LM Studio | 🖥️ | model name before `:` | `Color::Magenta` |

---

## Consequences

- **Zero protocol changes.** Both providers use the existing OpenAI SSE path.
  The streaming parser, context management, and agentic tool loop are unchanged.
- **Eight exhaustive match sites updated.** The Rust compiler enforces completeness,
  so no provider is silently unhandled anywhere.
- **LM Studio tool calls off by default.** Matches Ollama's conservative default;
  users enable explicitly once they confirm their loaded model supports the
  structured `tool_calls` delta format.
- **DeepSeek base URL is configurable.** Self-hosted DeepSeek deployments work
  without code changes, identical to the existing OpenAI Azure override pattern.
- **No auth regression for LM Studio.** `requires_auth()` returns `false`, so the
  `Authorization` header is never sent to the local server.

## References

- `src/agent/provider/deepseek.rs` — `DeepSeekProvider`
- `src/agent/provider/lmstudio.rs` — `LmStudioProvider`
- `src/agent/provider/mod.rs` — `ProviderKind`, `ProviderConfig`, `make_provider`
- `src/config/mod.rs` — `DeepSeekProviderConfig`, `LmStudioProviderConfig`
- `src/agent/models.rs` — `fetch_models_for_provider`
- `src/editor/ai.rs` — `one_shot_with_provider`
- ADR 0087 — Provider abstraction (`ChatProvider` trait)
