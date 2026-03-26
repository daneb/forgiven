# ADR 0083: MCP Memory Server for Cross-Session Context

**Date:** 2026-03-23
**Updated:** 2026-03-24
**Status:** Accepted

## Context

At the start of each new session the agent has no memory of prior work. Users typically paste in project context ("we're using ratatui 0.30, the main loop is in editor/mod.rs…") to re-establish context, which costs tokens and attention on every session.

The official `@modelcontextprotocol/server-memory` server implements a persistent knowledge graph (entities, relations, observations stored in a local JSONL file). The agent can store key facts at the end of a session and retrieve the relevant subgraph at the start of the next one, replacing expensive context replay.

The original ADR described "at session end" storage as aspirational usage guidance, relying on the agent to proactively call the memory tools. In practice this never happened — the agent has no signal that a session is ending, and the LLM does not call `create_entities` unprompted.

## Decision

### Phase 1 — MCP server configuration (original)

Document `@modelcontextprotocol/server-memory` as an optional but recommended MCP server in `README.md`:

```toml
[[mcp.servers]]
name    = "memory"
command = "npx"
args    = ["-y", "@modelcontextprotocol/server-memory"]
```

The existing MCP infrastructure (ADR 0045, ADR 0050) handles stdio transport, process lifecycle, and tool exposure automatically.

Tools exposed to the agent:
- `create_entities` — register named entities (e.g. "ratatui", "editor/mod.rs", "AgentPanel")
- `add_observations` — attach facts to entities
- `search_nodes` — semantic search over the knowledge graph
- `read_graph` — retrieve the full graph for a fresh context dump
- `create_relations` — link entities ("AgentPanel" — "lives in" → "src/agent/mod.rs")

### Phase 2 — Explicit memory save trigger (`SPC a s`)

Add `Action::MemorySave` bound to `SPC a s` (under the agent leader node). When triggered:

1. A canned prompt is injected into the agent input instructing the agent to call `create_entities`, `add_observations`, and `create_relations` for non-obvious facts from the session.
2. The agent panel is opened and focused so the tool calls stream visibly.
3. Submit fires immediately via the same `block_in_place` pattern used by Enter in Agent mode.

The prompt text (defined as `MEMORY_PROMPT` in `editor/mod.rs`):

> Please save the key context from this session to the knowledge graph now.
> 1. Call `create_entities` for any new concepts, files, or components we discussed.
> 2. Call `add_observations` with non-obvious facts discovered during this session (decisions made, bugs found, patterns identified, architectural constraints).
> 3. Call `create_relations` to link related entities where useful.
> Focus on what would be expensive to re-discover in a future session. Skip anything already obvious from reading the code.

This puts the user in explicit control rather than relying on an implicit "session end" signal the LLM cannot detect.

## Usage guidance

- **At session start:** call `search_nodes("project architecture")` to recover prior context.
- **During work:** call `add_observations` when the agent discovers non-obvious facts.
- **At session end:** press `SPC a s` to flush session context to the knowledge graph.

## Consequences

- `SPC a s` added to keymap under the `a` (agent) leader node.
- `Action::MemorySave` added to `src/keymap/mod.rs`.
- Handler added to `src/editor/mod.rs` alongside the markdown actions.
- Requires `npm` / `npx` to be available (already required for existing MCP servers).
- Knowledge graph persists across sessions in `~/.local/share/memory/` by default.
- No memory is stored if the `memory` MCP server is not configured — the agent will surface a tool-not-found error in the panel.
