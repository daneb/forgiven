# ADR 0098 — Ollama Local Provider

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

forgiven uses GitHub Copilot Enterprise as its sole AI backend (ADR 0001).
Several constraints motivate adding an alternative:

- **Token limits.** Copilot Enterprise sessions exhaust their quota during heavy
  coding sessions (ADR 0087).  A local model has no per-session quota.
- **Latency and cost.** Cloud roundtrips add 300–800 ms to first-token latency.
  A warmed local model on the same machine can stream at 30–120 tok/s with near-zero
  first-token latency.
- **Offline / air-gapped use.** Some environments have no outbound network access.
- **Model choice.** Ollama exposes a growing catalogue of coding-capable models
  (`qwen2.5-coder`, `deepseek-coder-v2`, `llama3.3`, etc.) with context windows
  up to 128 K tokens.

The requirement is straightforward: the user configures one provider in
`config.toml`; all agent interactions (chat, tool calls where supported, commit
messages, release notes) use that provider exclusively.  Switching providers
requires a restart.

---

## Decision

### Provider abstraction

A new `ProviderKind` enum and `ProviderSettings` struct replace the
previously hard-coded Copilot values scattered across the agent module.

```rust
// src/agent/provider.rs

pub enum ProviderKind {
    Copilot,   // default — cloud, OAuth auth, full tool-call support
    Ollama,    // local, no auth, OpenAI-compat /v1/chat/completions
}

pub struct ProviderSettings {
    pub kind:                 ProviderKind,
    pub api_token:            String,    // empty for Ollama
    pub chat_endpoint:        String,
    pub num_ctx:              Option<u32>,
    pub supports_tool_calls:  bool,
}
```

`ProviderSettings` is built once per `submit()` call and threaded into
`agentic_loop` and `start_chat_stream_with_tools` as a single argument,
replacing the previous `api_token: String` parameter.

### Config schema

```toml
[provider]
active = "ollama"           # "copilot" (default) | "ollama"

[provider.copilot]
default_model = "claude-sonnet-4"

[provider.ollama]
base_url        = "http://localhost:11434"
default_model   = "qwen2.5-coder:14b"
context_length  = 32768    # pins Ollama's KV-cache via options.num_ctx
tool_calls      = false    # opt-in; most models emit JSON as text, not structured calls
```

The legacy top-level `default_copilot_model` field is preserved for backwards
compatibility; `Config::active_default_model()` resolves the correct value for
the active provider.

### OpenAI-compatible endpoint

Ollama's `/v1/chat/completions` endpoint uses **identical SSE wire format** to
Copilot.  The entire SSE parser in `agentic_loop.rs` is unchanged.  Only three
things differ per provider:

| Concern | Copilot | Ollama |
|---------|---------|--------|
| Auth | `Authorization: Bearer {token}` | none |
| Endpoint | `https://api.githubcopilot.com/chat/completions` | `{base_url}/v1/chat/completions` |
| Extra headers | `Copilot-Integration-Id`, `editor-version`, `openai-intent` | none |

### Tool calling disabled by default for Ollama

Many local models — including current builds of `qwen2.5-coder` — do not
reliably emit tool calls in the OpenAI `tool_calls` delta format; they output
the call intent as raw JSON text in the content stream, which pollutes the panel.

When `tool_calls = false` (the default for Ollama), the agentic loop sends an
empty tools array and omits `tool_choice`.  The model operates as a plain chat
assistant.  Enable with `tool_calls = true` only after verifying the specific
model and Ollama version support structured tool calls.

### Timeout tuning

Local models have a different latency profile from cloud:

| Parameter | Copilot | Ollama |
|-----------|---------|--------|
| Connect timeout | 15 s | 60 s (model may be loading) |
| Per-chunk timeout | 60 s | 20 s (local is fast once warm) |
| Max retries | 5 | 2 (local failures rarely transient) |

### Startup warmup

Ollama loads the model into RAM on first request, adding 5–30 s of latency to
the first interaction.  At startup, when `provider.active = "ollama"`, a
background `tokio::spawn` fires immediately:

```
POST {base_url}/api/generate  { "model": "…", "keep_alive": "30m" }
```

Ollama loads the model without generating tokens and keeps it resident for
30 minutes.  The warmup runs concurrently with LSP/MCP startup; it never blocks
the editor from becoming interactive.  Failures are logged as warnings and
silently dropped.

### Per-provider UI labels and emoji

| Provider | Human turn | AI turn | Panel title |
|----------|-----------|---------|-------------|
| Copilot | `🧑 You` (green) | `🤖 Copilot` (cyan) | `Copilot [model]` |
| Ollama | `👤 You` (green) | `🦙 <model-base-name>` (magenta) | `Ollama [model]` |

The streaming header shows the model base name for Ollama
(`╔ 🦙 qwen2.5-coder ▋`) so users can confirm which model is active.

### Commit messages and release notes

`one_shot_complete` in `auth.rs` (Copilot-only) is removed and replaced by
`one_shot_with_provider` in `editor/ai.rs`.  It branches on `ProviderKind`:
Copilot acquires an OAuth token and sends routing headers; Ollama sends no auth
and uses the local endpoint.  Both use the same non-streaming
`/v1/chat/completions` call.

### Model discovery

| Provider | Endpoint | Context window |
|----------|----------|---------------|
| Copilot | `GET /models` — reports `capabilities.limits.max_context_window_tokens` | From API |
| Ollama | `GET /api/tags` — does not report context size | From `context_length` config, or a family heuristic table |

Heuristic fallbacks when `context_length` is not set:

| Model family | Assumed context |
|-------------|----------------|
| qwen2.5, qwen3 | 32 768 |
| deepseek, llama3, gemma3 | 131 072 |
| mistral, mixtral | 32 768 |
| phi4 | 16 384 |
| unknown | 8 192 |

Setting `context_length` explicitly is strongly recommended — it ensures the
history-truncation budget and Ollama's active KV-cache (`num_ctx`) are
consistent.

---

## Implementation

### New file

**`src/agent/provider.rs`**

- `ProviderKind` enum with per-variant constants for timeouts, retry counts,
  auth flag, stream-usage flag, emoji, and display name.
- `ProviderSettings` struct carrying all runtime HTTP parameters.
- `warmup_ollama(base_url, model)` async function.

### Modified files

| File | Change |
|------|--------|
| `src/config/mod.rs` | `CopilotProviderConfig`, `OllamaProviderConfig`, `ProviderConfig`; `active_default_model()` helper |
| `src/agent/mod.rs` | `pub mod provider`; re-export `ProviderKind`; `provider`, `ollama_base_url`, `ollama_context_length`, `ollama_tool_calls` fields on `AgentPanel` |
| `src/agent/models.rs` | `fetch_models_ollama()`, `infer_ollama_context_window()` |
| `src/agent/panel.rs` | `ensure_token()` short-circuits for Ollama; `ensure_models()`, `refresh_models()`, `submit()` branch on `ProviderKind`; `submit()` builds `ProviderSettings`; `ai_label_name()` method |
| `src/agent/agentic_loop.rs` | `agentic_loop` and `start_chat_stream_with_tools` accept `ProviderSettings` instead of `api_token: String`; tool-defs gated on `provider.supports_tool_calls`; `tool_choice` omitted when tools disabled; chunk timeout from `provider.chunk_timeout_secs()`; connect timeout from `provider.connect_timeout_secs()` |
| `src/agent/auth.rs` | `one_shot_complete` removed (superseded) |
| `src/ui/mod.rs` | `ProviderKind` added to import |
| `src/ui/agent_panel.rs` | Message labels and streaming header use `panel.provider.{user,ai}_emoji()` and `panel.ai_label_name()`; panel title uses `panel.provider.display_name()` |
| `src/editor/mod.rs` | `panel.provider`, `ollama_base_url`, `ollama_context_length`, `ollama_tool_calls` set from config at startup |
| `src/editor/ai.rs` | `one_shot_with_provider()` helper; `start_commit_msg` and `trigger_release_notes_generation` use active provider |
| `src/editor/actions.rs` | `preferred_model` uses `config.active_default_model()` |
| `src/editor/input.rs` | `preferred_model` uses `config.active_default_model()` |
| `src/main.rs` | Ollama warmup spawned after `setup_services()` when `provider.active = "ollama"` |

---

## Recommended configuration

### 16 GB desktop

```toml
[provider]
active = "ollama"

[provider.ollama]
base_url        = "http://localhost:11434"
default_model   = "qwen2.5-coder:14b"
context_length  = 32768
```

A 14 B model at Q4 quantisation uses ~10–12 GB of RAM, leaving 4–6 GB headroom
for the OS and editor.  32 K tokens of context fits comfortably within that
RAM budget.

### 24 GB desktop

```toml
[provider.ollama]
default_model   = "qwen2.5-coder:14b"
context_length  = 65536
```

Or use `deepseek-coder-v2:16b` (~12 GB) with `context_length = 65536`.

---

## Consequences

**Positive**
- No Copilot quota consumed during Ollama sessions.
- First-message latency after warmup is near-zero (local inference).
- Provider switch is a one-line config change; all tooling (chat, commit messages,
  release notes) follows automatically.
- The SSE parser, history truncation, and tool-calling loop are unchanged — there
  is no parallel code path to maintain.
- `ProviderSettings` encapsulates all provider-specific behaviour, making it
  straightforward to add a third provider (e.g. Anthropic direct API, LM Studio)
  by adding a new `ProviderKind` variant.

**Negative / trade-offs**
- Tool calling is disabled for Ollama by default.  The full agentic loop
  (file read/write, search, task planning) is unavailable until the user opts in
  with `tool_calls = true` and verifies their model supports structured calls.
- Ollama's `/api/tags` does not report context-window sizes.  Without an explicit
  `context_length` in config, history truncation uses a heuristic that may be
  wrong for fine-tuned or quantised variants.
- The warmup keeps the model in RAM for 30 minutes.  On memory-constrained
  machines this may be undesirable; set `keep_alive` in Ollama's server config
  to `0` to disable server-side model persistence and ignore the warmup effect.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Use Ollama's native `/api/chat` (NDJSON) | Requires a separate streaming parser; the OpenAI-compat `/v1/chat/completions` endpoint provides identical SSE format, allowing full parser reuse |
| `ollama-rs` crate | Thin reqwest wrapper with no advantage over raw reqwest for a codebase already using it; adds a dependency |
| `async_trait` + `Provider` trait with async methods | Adds a dependency; an enum + `ProviderSettings` struct achieves the same result with no additional crates and no boxing overhead |
| Enable tool calling for Ollama by default | Current model versions emit tool calls as raw text rather than structured deltas, breaking the panel; opt-in is safer |
| Per-round provider selection | Adds UI complexity; the use case is always session-level; a restart on config change is acceptable |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0001](0001-github-copilot-as-ai-backend.md) | Original decision to use Copilot — this ADR adds an alternative |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Token-limit pressure — primary motivation for a quota-free local alternative |
| [0093](0093-cap-open-file-context-injection.md) | Open-file cap — remains in effect for Ollama; critical given smaller default context windows |
