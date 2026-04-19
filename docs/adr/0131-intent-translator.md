# ADR 0131 — Intent Translator

**Date:** 2026-04-19  
**Status:** Implemented

---

## Context

Forgiven passes the user's raw message directly into the agentic loop after prepending
the system prompt and open-file context. This works for crisp prompts ("add a test for
`HoverHandler::request` that covers the error path") but produces expensive sessions for
underspecified ones ("fix the thing in the agent panel that's weird").

The observed cost pattern from `sessions.jsonl`:

- Ambiguous prompts trigger exploratory `read_file` loops (3–8 calls) as the agent infers
  intent from the codebase.
- Those tool calls accumulate in history and are re-sent every round.
- Sessions routinely exceed 150 k tokens before producing a usable result.

`docs/intent-translator.md` (ranked Option D — cheapest of four AI-IDE architecture
options) proposed a lightweight preprocessing step: a small, fast model rewrites the user
message into a structured task spec *before* the main agent sees it.

---

## Decision

Implement the Intent Translator as described in `docs/intent-translator.md`.

A new async function `translate_intent()` in `src/agent/intent.rs` runs inside `submit()`
after history selection and before the agentic loop is spawned. It makes a single
non-streaming HTTP call to a configurable backend, parses a JSON response, and either:

- **Replaces** the user message in the API payload with a crisp `structured_prompt`, while
  leaving the original message in the display history; or
- **Bails out** with clarifying questions shown as a dim System message when
  `ambiguities` is non-empty, letting the user refine and resubmit; or
- **Falls through** silently (raw message used unchanged) on timeout, HTTP error, or
  malformed JSON.

The feature is **disabled by default** (`enabled = false`) until validated against the
`forgiven-bench/` corpus.

---

## Implementation

### New module — `src/agent/intent.rs`

Public types:

```rust
pub struct TranslationContext<'a> { open_file, recent_files, project_root, language_hint }
pub struct Intent { goal, scope, expected_output, ambiguities, structured_prompt }
pub enum IntentScope { SingleFile, MultiFile, Symbol, ProjectWide, Unknown }
pub enum OutputType { Code, Diff, Explanation, Question, Mixed }
pub struct IntentCallSettings<'a> { endpoint, api_token, model, provider_kind, … }

pub async fn translate_intent(message, ctx, settings) -> Option<Intent>
pub fn format_preamble(intent: &Intent) -> String
```

`translate_intent` is a pure async function: no global state, no panel dependencies.
All provider credentials are threaded in via `IntentCallSettings`. Falls through on any
error.

### Config — `[agent.intent_translator]`

Added `IntentTranslatorConfig` to `AgentConfig`:

| Key                     | Default                   | Description                              |
|-------------------------|---------------------------|------------------------------------------|
| `enabled`               | `false`                   | Master switch                            |
| `provider`              | `"ollama"`                | `"ollama"` or `"active"` (main provider) |
| `ollama_model`          | `"qwen2.5-coder:7b"`      | Used when `provider = "ollama"`          |
| `model`                 | `"claude-haiku-4-5-20251001"` | Used when `provider = "active"`      |
| `min_chars_to_translate`| `40`                      | Skip short (already-crisp) messages      |
| `timeout_ms`            | `10000`                   | Abort and fall through on timeout        |
| `skip_patterns`         | `[]`                      | Literal prefixes that bypass translation |

Slash commands (`/…`) are always skipped regardless of `skip_patterns`.

### Provider routing

When `provider = "ollama"`: endpoint = `{ollama_base_url}/v1/chat/completions`, no auth,
`ProviderKind::Ollama`.

When `provider = "active"`: reuses the main agent's resolved endpoint and token.

### Panel wiring — `src/agent/panel.rs`

In `submit()`, after history selection and before the current user message is pushed to
`send_messages`:

1. Resolve translator endpoint/token/model based on `intent_translator_provider`.
2. Call `translate_intent()`.
3. On ambiguity: push System + User messages and return `Ok(())` (no loop spawned).
4. On clean translation: set `api_user_text = structured_prompt`; push a dim System
   preamble (`"Goal: … · Scope: … · Output: …"`) before the User message.
5. On fallthrough (`None`): `api_user_text = user_text` (unchanged).

The display history always shows the **original** user message; the API payload uses the
translated version.

### Keybinding — `SPC a t`

`AgentIntentTranslatorToggle` flips `intent_translator_enabled` for the current session
and reports the new state in the status bar. No config file edit required.

---

## Recommended model

`qwen2.5-coder:7b` via Ollama is the recommended default:

- Fits in 14 GB unified memory with headroom for the editor process (~4.5 GB weights).
- 70–80 tokens/sec on Apple M4 with Ollama's MLX backend.
- First token in ~1–2 s — well under the 10 s timeout.
- Reliably produces raw JSON without markdown fences due to code/structured-data training.

`gemma3:4b` is a viable fallback if memory is tight; `qwen3:4b` is not yet proven for
structured JSON output on Apple Silicon.

---

## Consequences

**Positive:**
- Ambiguous prompts are caught before they trigger expensive exploratory tool loops.
- The model receives a crisp, scoped instruction; unnecessary `read_file` discovery rounds
  are avoided.
- Latency overhead is bounded and transparent: 1–2 s on local Ollama, silent fallthrough
  on any failure.
- The feature is fully opt-in and session-togglable with no architectural commitment.

**Negative / open:**
- Adds 1–3 s of latency to every dispatched message when enabled. Mitigated by
  `min_chars_to_translate` and `skip_patterns`.
- Translation quality is unvalidated without the `forgiven-bench/` corpus. The `enabled =
  false` default enforces this: measure before rolling out.
- Ollama must be running and the model pulled. Cold starts (model load) can approach the
  10 s timeout on the first call of a session.

---

## Relationship to other ADRs

| ADR  | Relationship                                                              |
|------|---------------------------------------------------------------------------|
| 0057 | `ask_user` popup. The ambiguity bail-out path uses the same System-message display convention, but does not yet wire into the interactive ask_user channel. |
| 0098 | Ollama provider. Translator reuses `ollama_base_url` and the Ollama HTTP path. |
| 0116 | Multi-provider backend. `IntentCallSettings` mirrors the `ProviderSettings` pattern. |
| 0128 | Investigation subagent. Same pre-loop single-round pattern, different purpose. |
| 0130 | Expand-on-demand. Both ADRs reduce token cost; this one acts before the loop starts. |

---

## Validation plan

Measure against `forgiven-bench/` corpus once built (see `docs/ai-ide-specs-index.md`):

| Metric                     | Target                                      |
|----------------------------|---------------------------------------------|
| Mean tokens per task       | ≥ 20% reduction vs translator off           |
| Answer quality (F1 vs golden) | No drop > 2 percentage points            |
| P95 first-token latency    | ≤ +3 s with local Ollama                   |
| Translation success rate   | ≥ 95% (rest fall through silently)          |

Interim signal before the corpus exists: compare `SPC d` session-total token counts over
10 real sessions with translator on vs. off.
