# ADR 0040 — Context Gauge in Agent Panel Title

**Date:** 2026-03-04
**Status:** Accepted

## Context

The agent chat panel sends all conversation history to the Copilot API on every
request.  As a conversation grows the total prompt token count approaches the
model's context-window limit.  When that limit is hit the API silently truncates
history, degrading response quality without any warning to the user.

The OpenAI-compatible streaming API can emit a final `usage` chunk when the
request includes `"stream_options": {"include_usage": true}`.  This chunk
carries `prompt_tokens` and `completion_tokens` counts that accurately reflect
what was billed for the request.

## Decision

1. **Add `stream_options`** to every chat-completion request so the API emits a
   usage chunk after `[DONE]`.

2. **Parse the usage chunk** in the SSE streaming loop and emit a new
   `StreamEvent::Usage { prompt_tokens, completion_tokens }` event.

3. **Store token counts** on `AgentPanel` (`last_prompt_tokens`,
   `last_completion_tokens`), updated via the existing `poll_stream()` handler.
   Values stay at `0` until the first response arrives — graceful degradation if
   the API does not support usage chunks.

4. **Add `context_window_size()`** on `AgentPanel` returning a hardcoded limit
   keyed on the selected model ID prefix:
   - `gpt-4o`, `gpt-4`, `o1`, `o3` → 128 000 tokens
   - `claude` → 200 000 tokens
   - fallback → 128 000 tokens

5. **Render a color-coded gauge** in the agent panel history-block title as a
   ratatui `Line` with per-span styling:
   - < 50 % used → `DarkGray` (subtle, low priority)
   - 50–79 % used → `Yellow` (warning)
   - ≥ 80 % used → `Red` (alert)

   Example title appearances:
   ```
    Copilot Chat [gpt-4o]  8.4k/128k
    Copilot Chat [gpt-4o]  68.5k/128k  ● streaming [2/20]
    Copilot Chat [gpt-4o]  104.1k/128k
   ```

## Consequences

- Users see real-time feedback about context pressure without leaving the editor.
- If GitHub Copilot does not emit usage chunks the gauge stays blank — no
  regression for existing users.
- Hardcoded limits are an approximation; they are good enough for a visual
  warning and can be refined as new models are added.
- `completion_tokens` is stored but not displayed; it is available for future
  use (e.g. cost estimation).
