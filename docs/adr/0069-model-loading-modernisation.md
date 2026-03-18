# ADR 0069 — Model Loading Modernisation: Codex Models, Dynamic Context Windows, Updated Defaults

**Date:** 2026-03-18
**Status:** Accepted

---

## Context

The GitHub Copilot model landscape evolved significantly since ADRs 0014/0028/0038 were
written. The original code was built around a `gpt-4o`-centric world; several assumptions
were now stale:

1. **`id.contains("codex")` filter blocked new Codex models.** The filter at `fetch_models()`
   was added to exclude legacy OpenAI Codex (code-davinci-002 etc.) — completion-only models
   that fail on `/chat/completions`. GitHub's current model catalogue includes GPT-5.1-Codex,
   GPT-5.2-Codex, GPT-5.3-Codex, GPT-5.1-Codex-Mini, and GPT-5.1-Codex-Max — all legitimate
   chat/agent-capable models. The blanket `contains("codex")` check silently dropped them.

2. **`capabilities.type` filter was too narrow.** Only `"chat"` was accepted. The newer Codex
   models may report a `"agent"` capability type. Filtering on `!= "chat"` alone risked
   excluding them even after fixing the name-based filter.

3. **Hardcoded context-window sizes.** `context_window_size()` used `starts_with` prefix
   checks for `gpt-4o`, `gpt-4`, `o1`, `o3`, and `claude` only. GPT-5.x, Gemini, and Grok
   families were all unmapped, falling through to a generic 128k default. Meanwhile, the
   Copilot `/models` API already returns the exact value via
   `capabilities.limits.max_context_window_tokens`.

4. **"gpt-4o" default is deprecated.** GitHub's supported-models list no longer includes
   `gpt-4o`. The `copilot-cli` system default is `claude-sonnet-4.5`. Three locations in our
   code used `"gpt-4o"` as a fallback: `default_copilot_model()`, `selected_model_id()`, and
   `set_models()`.

5. **Sorting hardcoded `gpt-4o` first.** The sort comparator gave `gpt-4o` priority rank 0;
   with `gpt-4o` gone from the list this collapsed to a plain alphabetical sort.

6. **Misleading docstrings.** The `ModelVersion` struct docstring said `version` was
   "the pinned build sent in requests", and `selected_model_id()` claimed to return
   `version` for exact routing. Both were wrong: the code correctly sends `id`, which is
   confirmed by the [ericc-ch/copilot-api](https://github.com/ericc-ch/copilot-api) proxy
   (`model.id === payload.model`) and the
   [copilot-cli model routing architecture](https://deepwiki.com/github/copilot-cli/6.6-model-routing-and-api-communication).

---

## Decision

### 1. Remove `"codex"` from the name-based filter (`src/agent/mod.rs`)

```rust
// Before
if id.contains("embed") || id.contains("whisper")
    || id.contains("tts") || id.contains("dall")
    || id.contains("codex")
{ return None; }

// After
if id.contains("embed") || id.contains("whisper")
    || id.contains("tts") || id.contains("dall")
{ return None; }
```

The `capabilities.type` filter (see below) already handles non-chat models. The remaining
name-based entries (`embed`, `whisper`, `tts`, `dall`) are kept as safety nets for models
that may lack capabilities metadata.

### 2. Widen `capabilities.type` to accept `"agent"` (`src/agent/mod.rs`)

```rust
// Before
if cap_type != "chat" { return None; }

// After
if cap_type != "chat" && cap_type != "agent" { return None; }
```

### 3. Parse and store `context_window` from the API response (`src/agent/mod.rs`)

Added a `context_window: u32` field to `ModelVersion`, populated from
`capabilities.limits.max_context_window_tokens` (fallback: 128,000):

```rust
pub struct ModelVersion {
    pub id: String,
    pub version: String,
    pub name: String,
    pub context_window: u32,  // ← new
}
```

`context_window_size()` now reads this field directly instead of using hardcoded
`starts_with` prefix checks:

```rust
pub fn context_window_size(&self) -> u32 {
    if self.available_models.is_empty() { return 128_000; }
    self.available_models[self.selected_model.min(self.available_models.len() - 1)]
        .context_window
}
```

### 4. Update default model to `"claude-sonnet-4"` (`src/agent/mod.rs`, `src/config/mod.rs`)

Changed in three locations:
- `default_copilot_model()` config default
- `selected_model_id()` pre-fetch fallback
- `set_models()` fallback when preferred model not found

`"claude-sonnet-4"` was chosen as a widely available, non-preview model. Users with an
existing `default_copilot_model` in their `config.toml` are unaffected — their saved
preference takes priority.

### 5. Simplify sort order (`src/agent/mod.rs`)

```rust
// Before
models.sort_by(|a, b| {
    let a_pref = if a.id == "gpt-4o" { 0 } else { 1 };
    let b_pref = if b.id == "gpt-4o" { 0 } else { 1 };
    a_pref.cmp(&b_pref).then(a.id.cmp(&b.id))
});

// After
models.sort_by(|a, b| a.id.cmp(&b.id));
```

The user's preferred model is already positioned by `set_models()` via config lookup;
the sort only needs to be stable and predictable for cycling.

### 6. Fix docstrings (`src/agent/mod.rs`)

- `ModelVersion`: corrected to state `id` is sent in requests, `version` is metadata.
- `selected_model_id()`: corrected to state it returns `id`.
- `fetch_models()`: updated header comment to match new sort behaviour.

---

## Consequences

**Positive**

- GPT-5.x-Codex models now appear in the Ctrl+T picker and can be selected for chat.
- Context gauge (ADR 0040) shows accurate token counts for all models, including Gemini
  (1M+ windows) and newer GPT-5.x variants, without code changes when new models are added.
- Default model is a current, available model — new users are no longer routed to a
  deprecated `gpt-4o` fallback.
- Docstrings accurately describe the code's behaviour, preventing future misunderstandings
  about which field is sent to the API.

**Negative / trade-offs**

- Users who had `default_copilot_model = "gpt-4o"` in their config will now see a
  `warn!` log and fall back to `claude-sonnet-4` instead of silently using index 0.
  This is strictly better — the log makes the fallback diagnosable.
- If the Copilot API introduces a new `capabilities.type` value beyond `"chat"` and
  `"agent"`, those models will be filtered out until we add the new type.
- Models with missing `max_context_window_tokens` fall back to 128k, which may be
  inaccurate for some edge cases.

---

## Related ADRs

- **ADR 0014** — Agent Model Selection (original dynamic discovery + Ctrl+T)
- **ADR 0028** — Model Selection Persistence (config save + eager loading)
- **ADR 0038** — Unified Model Selection (removed `model_picker_enabled` filter)
- **ADR 0040** — Context Gauge (uses `context_window_size()`)
