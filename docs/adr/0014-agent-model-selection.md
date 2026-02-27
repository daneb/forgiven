# ADR 0014 — Agent Model Selection: Dynamic Discovery and Ctrl+T Cycling

**Date:** 2026-02-23  
**Updated:** 2026-02-26 (added configurable default and refresh)  
**Status:** Accepted

---

## Context

The `start_chat_stream_with_tools` function had `"model": "gpt-4o"` hardcoded.
This created two problems:

1. **Stale list**: any hardcoded list of model IDs would go out of date as
   GitHub Copilot adds new models. Between mid-2024 and early 2026 the catalogue
   grew from a handful of OpenAI models to include Claude 3.x/4.x, Gemini 2.x/3.x,
   Grok, and specialised reasoning/coding variants.

2. **No user control**: users had no way to choose a model without recompiling.
   Different models have different strengths — a fast model for quick edits, a
   reasoning model for architectural questions, a coding-specialist for refactors.

3. **No way to pick up new releases**: Once the model list was fetched, there was
   no way to refresh it when GitHub Copilot released new models without restarting
   the editor.

4. **No user preference**: The default model was always `gpt-4o` with no way for
   users to configure their preferred model.

A hardcoded fallback list was considered but rejected because the exact API slug
strings (e.g. `claude-sonnet-4-5`, `gemini-2.5-pro`) are not reliably
documented and change with each release. The only authoritative source is the
live `/models` endpoint.

---

## Decision

### 1. Live model discovery: `GET /models`

A new `fetch_models(api_token)` async function calls the Copilot API:

```
GET https://api.githubcopilot.com/models
Authorization: Bearer <token>
```

The response is OpenAI-compatible:

```json
{
  "data": [
    { "id": "gpt-4o", "model_picker_enabled": true, ... },
    { "id": "claude-sonnet-4-5", "model_picker_enabled": true, ... },
    ...
  ]
}
```

**Filtering rules:**
- Skip models whose `id` contains `embed`, `whisper`, `tts`, or `dall`
  (non-chat capabilities).
- Skip models where `model_picker_enabled == false` (hidden from the UI by
  GitHub's own flag).

**Sort order:** `gpt-4o` first (existing default), then alphabetically, so the
list is stable across calls and `gpt-4o` remains the default on first load.

### 2. `AgentPanel` state additions

```rust
pub struct AgentPanel {
    // ... existing fields ...
    /// Model IDs fetched from GET /models (lazily populated on first use).
    pub available_models: Vec<String>,
    /// Index into available_models for the currently selected model.
    pub selected_model: usize,
}
```

**`selected_model_id() -> &str`** — returns the active model ID, falling back
to `"gpt-4o"` before the list has been fetched:

```rust
pub fn selected_model_id(&self) -> &str {
    if self.available_models.is_empty() { return "gpt-4o"; }
    &self.available_models[self.selected_model.min(self.available_models.len() - 1)]
}
```

**`cycle_model()`** — advances the index, wrapping at the end of the list.

**`ensure_models(preferred_model: &str)`** — idempotent: fetches the list if empty, 
no-op otherwise. Selects `preferred_model` if available, falls back to `gpt-4o`, 
then index 0.

**`refresh_models(preferred_model: &str)`** — forces a refresh from the API, preserving
the current selection if still available, otherwise selecting `preferred_model`.

**`set_models(models, preferred_model)`** — internal helper that sets the available
models and intelligently selects the best default (user preference → gpt-4o → first).

### 3. Configuration: `~/.config/forgiven/config.toml`

Users can now set their preferred default model:

```toml
# ~/.config/forgiven/config.toml
default_copilot_model = "claude-sonnet-4-5"

tab_width = 4
use_spaces = true

[[lsp.servers]]
language = "rust"
command = "rust-analyzer"
```

The `Config` struct now includes:
```rust
pub struct Config {
    pub default_copilot_model: String,  // Defaults to "gpt-4o" if not set
    // ... other fields
}
```

The editor stores config in `Editor.config` and passes `config.default_copilot_model`
to `ensure_models()` on first use.

### 4. Lazy population strategy

The model list is populated in two places, whichever happens first:

| Trigger | Where |
|---------|-------|
| User presses `Ctrl+T` with empty list | `handle_agent_mode()` via `block_in_place` |
| User submits first message | `AgentPanel::submit()` before spawning the loop |

This means:
- **Zero startup cost** — no network call until the user interacts with the
  agent.
- **`Ctrl+T` works before first submit** — the handler calls `ensure_models()`
  synchronously via `tokio::task::block_in_place` (same pattern used by the
  `Enter`/submit handler).

### 5. `Ctrl+T` keybinding (cycle models)

`Ctrl+M` was the original choice but is **byte `0x0D` (carriage return)** —
identical to `Enter` in every terminal emulator, even in raw mode. Crossterm
reports it as `KeyCode::Enter`, so the `KeyCode::Char('m') with CONTROL` arm
never fires.

`Ctrl+T` (byte `0x14`) has no reserved meaning in VT terminals and is safe to
use in raw mode:

```rust
KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    if self.agent_panel.available_models.is_empty() {
        self.set_status("Loading model list…".to_string());
        let preferred = self.config.default_copilot_model.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let _ = self.agent_panel.ensure_models(&preferred).await;
            });
        });
    }
    self.agent_panel.cycle_model();
    let model = self.agent_panel.selected_model_id().to_string();
    let n = self.agent_panel.available_models.len();
    let idx = self.agent_panel.selected_model + 1;
    self.set_status(format!("Agent model → {model}  [{idx}/{n}]  (Ctrl+T to cycle)"));
}
```

### 6. `Ctrl+Shift+T` keybinding (refresh model list)

When GitHub Copilot releases new models, users can refresh the list without
restarting the editor:

```rust
KeyCode::Char('T') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    self.set_status("Refreshing model list from API…".to_string());
    let preferred = self.config.default_copilot_model.clone();
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            if let Err(e) = self.agent_panel.refresh_models(&preferred).await {
                self.set_status(format!("Failed to refresh models: {e}"));
            } else {
                let model = self.agent_panel.selected_model_id().to_string();
                let n = self.agent_panel.available_models.len();
                self.set_status(format!("Refreshed {n} models, selected: {model}"));
            }
        });
    });
}
```

The refresh preserves the currently selected model if it's still available,
otherwise falls back to the configured default.

### 7. Model shown in panel title and hint

The agent panel title now includes the active model:

```
╭ Copilot Chat [claude-sonnet-4-5] ─────────────────────╮
```

The input box hint is updated to advertise the keybinding:

```
 Ask Copilot… (Enter=send, Ctrl+T=model, Ctrl+Shift+T=refresh, Tab=back)
```

### 8. Model propagated through the call chain

The model ID is passed from `submit()` all the way to the HTTP body:

```
submit()
  → model_id = self.selected_model_id().to_string()
  → tokio::spawn(agentic_loop(..., model_id))
      → start_chat_stream_with_tools(..., model_id)
          → body["model"] = model_id
```

The same `model_id` is reused for every round of the tool-calling loop within a
single conversation, so a multi-round task is never split across two models.

---

## Consequences

- **First `Ctrl+T` latency**: the initial press triggers a token exchange
  (if needed) plus the `/models` HTTP call — typically 300–700 ms total.
  Subsequent presses are instant. The status bar shows `"Loading model list…"`
  to indicate activity.
- **Configurable default**: Users can set their preferred model in `config.toml`
  so it's selected by default on first use.
- **Model refresh**: `Ctrl+Shift+T` refreshes the model list from the API without
  restarting the editor, picking up newly released models.
- **Smart fallback**: If the configured default is no longer available (deprecated),
  the system falls back to `gpt-4o`, then the first available model.
- **Preserved selection on refresh**: When refreshing, the currently selected model
  is preserved if it's still available in the new list.
- **Terminal key constraints**: `Ctrl+C`, `Ctrl+D`, `Ctrl+Z`, `Ctrl+M` (Enter),
  `Ctrl+I` (Tab), `Ctrl+H` (Backspace) are all reserved at the terminal level.
  `Ctrl+T` was chosen as the closest safe letter to the original intent.
- **Model compatibility**: not all models support the `tool_choice: "auto"`
  field required for the agentic loop. If the user selects a non-tool-capable
  model the API will return an error, which is surfaced via `StreamEvent::Error`
  in the chat panel.
