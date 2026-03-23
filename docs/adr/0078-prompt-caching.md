# ADR 0078: Prompt Caching — Cached Token Tracking

**Date:** 2026-03-23
**Status:** Accepted

## Context

Every agentic loop round re-sends the full prompt (system message + tool definitions + conversation history) to the API. For long sessions this wastes tokens and money. Provider-side prompt caching can serve repeated stable prefixes at a fraction of the input token cost.

Forgiven connects to GitHub Copilot's OpenAI-compatible endpoint (`api.githubcopilot.com/chat/completions`), not the Anthropic API directly. **Anthropic `cache_control` markers do not apply here.** OpenAI's automatic prompt caching is the relevant mechanism:

- Activates automatically for prompts ≥ 1,024 tokens (no API changes required).
- Applies to the longest stable prefix of the request.
- Returns the cached token count in `usage.prompt_tokens_details.cached_tokens`.
- Cached tokens are billed at 50% of the base input price.

The current request structure already places stable content first (tool definitions in the `tools` array, system message first in `messages`), which is the correct layout for automatic caching to apply.

## Decision

1. Parse `usage.prompt_tokens_details.cached_tokens` from the streaming SSE response.
2. Propagate the count through `StreamEvent::Usage { cached_tokens }` to `AgentPanel`.
3. Display it in the existing context gauge as `"5.2k/200k (3.1k cached)"` so users can see when caching is active and how much is being saved.

No request structure changes are needed — the message ordering is already correct.

## Consequences

- Users get real-time feedback on cache efficiency directly in the panel title.
- Zero risk: purely additive to the response parsing path; cached_tokens defaults to 0 when absent.
- Automatic caching only activates after a prompt crosses the 1,024-token threshold and the provider's cache warms up, so the indicator may be 0 for short sessions.
