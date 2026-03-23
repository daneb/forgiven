# ADR 0083: MCP Memory Server for Cross-Session Context

**Date:** 2026-03-23
**Status:** Accepted

## Context

At the start of each new session the agent has no memory of prior work. Users typically paste in project context ("we're using ratatui 0.30, the main loop is in editor/mod.rs…") to re-establish context, which costs tokens and attention on every session.

The official `@modelcontextprotocol/server-memory` server implements a persistent knowledge graph (entities, relations, observations stored in a local JSONL file). The agent can store key facts at the end of a session and retrieve the relevant subgraph at the start of the next one, replacing expensive context replay.

## Decision

Document `@modelcontextprotocol/server-memory` as an optional but recommended MCP server in `README.md`:

```toml
[[mcp.servers]]
name    = "memory"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-memory"]
```

No code changes are required — the existing MCP infrastructure (ADR 0045, ADR 0050) handles stdio transport, process lifecycle, and tool exposure automatically.

Tools exposed to the agent:
- `create_entities` — register named entities (e.g. "ratatui", "editor/mod.rs", "AgentPanel")
- `add_observations` — attach facts to entities
- `search_nodes` — semantic search over the knowledge graph
- `read_graph` — retrieve the full graph for a fresh context dump
- `create_relations` — link entities ("AgentPanel" — "lives in" → "src/agent/mod.rs")

## Usage guidance

The agent should:
1. At session start: call `search_nodes("project architecture")` to recover prior context.
2. During work: call `add_observations` when it discovers non-obvious facts.
3. At session end: store key decisions via `create_entities` + `add_observations`.

## Consequences

- Zero code changes; purely additive configuration.
- Requires `npm` / `npx` to be available (already required for existing MCP servers).
- Knowledge graph persists across sessions in `~/.local/share/memory/` by default.
- The agent learns the memory tools are available from the tool definitions — no prompt engineering needed.
