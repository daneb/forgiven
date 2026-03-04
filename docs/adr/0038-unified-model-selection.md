# ADR 0038 — Unified Model Selection: Removing the `model_picker_enabled` Filter

**Status:** Accepted

---

## Context

The agent panel fetches available chat models from `GET https://api.githubcopilot.com/models`
and exposes them via the Ctrl+T cycling keybinding. The `fetch_models` function previously
applied two filters before adding a model to the selectable list:

1. **Non-chat exclusion** — models whose IDs contain `embed`, `whisper`, `tts`, or `dall`
   are dropped (these are embedding, transcription, and image-generation models, not chat).
2. **`model_picker_enabled` exclusion** — models where the API response includes
   `"model_picker_enabled": false` were silently dropped from the list.

The second filter caused a user-visible inconsistency. GitHub Copilot marks several chat-capable
models — including `claude-3.5-sonnet` — with `"model_picker_enabled": false`. These models
are not excluded from the `/chat/completions` endpoint; they work correctly when used. However,
because they were filtered out of `available_models`, they never appeared in the Ctrl+T picker.

The result was that a user might be receiving responses from a model (e.g., one set in the
config or inferred from prior session state) that they could not see, select, or deliberately
switch away from using the in-editor controls. The selected model and the apparent model
selection UI were out of sync.

Additionally, when `set_models` could not find the user's saved `default_copilot_model` in the
filtered list, it silently fell back to `gpt-4o` or position 0 with no diagnostic output.

---

## Decision

### 1. Remove the `model_picker_enabled` filter (`src/agent/mod.rs`)

The `"model_picker_enabled": false` check is removed from `fetch_models`. The only models
still excluded are those whose IDs contain `embed`, `whisper`, `tts`, or `dall` — models
that are definitively not chat models regardless of their API flags.

```rust
// Before
let id = v.get("id")?.as_str()?.to_string();
if id.contains("embed") || id.contains("whisper") || id.contains("tts") || id.contains("dall") {
    return None;
}
if let Some(picker) = v.get("model_picker_enabled") {
    if picker == &serde_json::Value::Bool(false) {
        return None;
    }
}
Some(id)

// After
let id = v.get("id")?.as_str()?.to_string();
if id.contains("embed") || id.contains("whisper") || id.contains("tts") || id.contains("dall") {
    return None;
}
Some(id)
```

The `model_picker_enabled` field reflects GitHub's own VS Code picker UI preferences, not a
restriction on API usability. Deferring to it in forgiven's picker is incorrect: it hides
models the user may legitimately want to use.

### 2. Warn when the preferred model is not found (`src/agent/mod.rs`)

`set_models` now emits a `warn!` log entry when the saved `default_copilot_model` is not
present in the returned model list, before falling back:

```rust
fn set_models(&mut self, models: Vec<String>, preferred_model: &str) {
    let found = models.iter().position(|m| m == preferred_model);
    if found.is_none() && !preferred_model.is_empty() {
        warn!(
            "Preferred model '{}' not found in model list; falling back. Available: {:?}",
            preferred_model, models
        );
    }
    let default_idx = found
        .or_else(|| models.iter().position(|m| m == "gpt-4o"))
        .unwrap_or(0);
    self.available_models = models;
    self.selected_model = default_idx;
}
```

This makes silent fallbacks diagnosable via the log file without adding UI noise for the
common case where the preferred model is found normally.

---

## Alternatives considered

**Keep the filter, expose a separate "all models" list**

A second model list (all models) alongside the picker list would let the user access hidden
models via a separate binding. Rejected: the complexity is not warranted. The Copilot API's
`model_picker_enabled` flag is an artefact of VS Code's UI and carries no meaningful semantic
value for forgiven.

**Surface the fallback in the status bar**

Showing a status-bar message like `"Model 'claude-3.5-sonnet' not found, using gpt-4o"` on
every session start could be noisy — particularly during network failures or token expiry
where `available_models` is temporarily empty. A `warn!` log is sufficient for diagnosis
without affecting normal UX.

**Query `/models` at startup unconditionally**

Currently the model list is fetched lazily (on first submit or Ctrl+T). Eager loading at
startup would eliminate the window where `selected_model_id()` returns the `"gpt-4o"`
placeholder. Deferred; lazy loading avoids an unnecessary API round-trip for sessions where
the agent panel is never opened.

---

## Consequences

**Positive**
- All chat-capable models returned by the Copilot API are now visible and selectable via
  Ctrl+T, including Claude models that GitHub marks as `model_picker_enabled: false`.
- The model shown in the agent panel title bar is always the model actually used for
  completions — there is no longer a hidden discrepancy.
- Silent fallbacks are now diagnosable from the log file.

**Negative / trade-offs**
- The picker may show more models than before, including preview or beta models that
  GitHub intentionally suppressed from its own UI. Users should be aware that not all
  listed models are equally stable.
- The `model_picker_enabled` flag is now ignored entirely. If GitHub uses it in future to
  signal genuine deprecation or removal, we would need to revisit this decision.
