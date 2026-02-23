# ADR 0014 — Agent Model Selection: Dynamic Discovery and Ctrl+T Cycling

**Date:** 2026-02-23
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

**`ensure_models()`** — idempotent: fetches the list if empty, no-op otherwise.
Used by the `Ctrl+T` handler.

### 3. Lazy population strategy

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

### 4. `Ctrl+T` keybinding

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
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let _ = self.agent_panel.ensure_models().await;
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

### 5. Model shown in panel title and hint

The agent panel title now includes the active model:

```
╭ Copilot Chat [claude-sonnet-4-5] ─────────────────────╮
```

The input box hint is updated to advertise the keybinding:

```
 Ask Copilot… (Enter=send, Ctrl+T=model, Tab=back)
```

### 6. Model propagated through the call chain

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
- **Model list per session**: the list is cached in `AgentPanel.available_models`
  for the lifetime of the process. If Copilot adds new models while the editor
  is running, a restart is required to see them. A future improvement could
  refresh the list when the token is renewed.
- **Terminal key constraints**: `Ctrl+C`, `Ctrl+D`, `Ctrl+Z`, `Ctrl+M` (Enter),
  `Ctrl+I` (Tab), `Ctrl+H` (Backspace) are all reserved at the terminal level.
  `Ctrl+T` was chosen as the closest safe letter to the original intent.
- **Model compatibility**: not all models support the `tool_choice: "auto"`
  field required for the agentic loop. If the user selects a non-tool-capable
  model the API will return an error, which is surfaced via `StreamEvent::Error`
  in the chat panel.
