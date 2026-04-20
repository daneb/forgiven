# ADR 0132 — Codified Context: Three-Tier Context Loader

**Status:** Implemented
**Date:** 2026-04-20
**Spec:** `docs/codified-context.md`

---

## Context

Forgiven agents repeatedly rediscover project conventions, make style-inconsistent changes, and call `get_file_outline` / `read_file` in round 1 to orient themselves. The root cause: there is no persistent, human-curated channel for project knowledge. ADR 0083 added an MCP memory server for agent-derived facts; this ADR adds the complementary channel for human-curated facts.

Research basis:
- Codified Context Infrastructure paper (March 2026, arXiv:2602.20478): three-tier hot/warm/cold memory validated on a 108 000-line C# distributed system with 19 specialised agents.
- Chatlatanagulchai et al. (November 2025, arXiv:2511.12884): 72% of Claude Code projects already write context files; the shallow Markdown hierarchy is the de facto standard.

---

## Decision

Implement a three-tier loader sourced from a `.forgiven/` directory at the project root.

### Tiers

| Tier | Path | Load strategy | Target size |
|---|---|---|---|
| 1 — Constitution | `.forgiven/constitution.md` | Always injected into system prompt | ≤ 500 tokens |
| 2 — Specialists | `.forgiven/agents/*.md` | Injected when path glob or ≥2 keywords match | ≤ 800 t each |
| 3 — Knowledge | `.forgiven/knowledge/*.md` | Retrieved by the model via `fetch_knowledge(name)` | ≤ 8 192 bytes per call |

### Config

```toml
[agent.codified_context]
enabled                 = false          # opt-in
directory               = ".forgiven"
constitution_max_tokens = 500
max_specialists_per_turn = 2
knowledge_fetch_max_bytes = 8192
```

### Specialist frontmatter (optional)

```markdown
---
trigger:
  paths: ["src/agent/**", "src/mcp_servers/**"]
  keywords: ["agent", "tool call", "MCP", "streaming"]
---

# Agent Specialist
...
```

Trigger logic: load when the active file matches a `paths` glob OR ≥ 2 `keywords` appear in the user message. At most `max_specialists_per_turn` specialists are injected, taken in lexicographic order from `.forgiven/agents/`.

### Knowledge catalogue

When knowledge documents exist, the system prompt includes:

```
Knowledge base (call fetch_knowledge(name) to retrieve):
- architecture-overview (Service topology, data flow)
- api-contracts (Public API schemas and versioning)
```

The one-line description is extracted from the first non-heading paragraph of each document (or a `## Summary` section if present).

---

## Implementation

| File | Change |
|---|---|
| `src/agent/codified_context.rs` | New module: `CodifiedContext`, `Constitution`, `Specialist`, `KnowledgeDoc` structs; `load()`, `system_prompt_block()`, `triggered_specialists()`; simple glob matcher; frontmatter parser |
| `src/config/mod.rs` | `CodifiedContextConfig` struct; `codified_context` field on `AgentConfig` |
| `src/agent/mod.rs` | 5 new fields on `AgentPanel`; `codified_context` module registered |
| `src/agent/panel.rs` | Reload on first submit; constitution + specialist injection in system prompt; tip-line when `.forgiven/` absent |
| `src/agent/tools.rs` | `fetch_knowledge` tool schema |
| `src/agent/agentic_loop.rs` | `knowledge_docs` + `knowledge_fetch_max_bytes` params passed to dispatch |
| `src/agent/tool_dispatch.rs` | Inline `fetch_knowledge` handler (async file read with byte cap) |
| `src/keymap/mod.rs` | 3 new `Action` variants; `SPC a c / SPC a C / SPC a k` keybindings |
| `src/editor/actions.rs` | Handlers: constitution creates stub if absent; specialists/knowledge open the dir and set status |
| `src/editor/mod.rs` | Wire `CodifiedContextConfig` fields to `AgentPanel` at startup |
| `src/ui/mod.rs` | `codified_context_info` field on `DiagnosticsData` |
| `src/editor/render.rs` | Populate `codified_context_info` from panel |
| `src/ui/popups.rs` | Render Codified Context section in `SPC d`: constitution tokens vs cap, specialist and knowledge counts |

Total: ~600 LoC across 12 files.

---

## Keybindings

| Key | Action |
|---|---|
| `SPC a c` | Open `.forgiven/constitution.md` (creates stub if absent) |
| `SPC a C` | Status tip directing user to `.forgiven/agents/` in the explorer |
| `SPC a k` | Status tip directing user to `.forgiven/knowledge/` in the explorer |

---

## Tip-line

On the first agent turn of a session when `.forgiven/` does not exist, a System message is posted:

```
[tip] Create .forgiven/constitution.md to improve agent consistency across sessions.
```

Fires once per conversation. Suppressed by `codified_context_tip_shown`. Cleared by `new_conversation()`.

---

## Relationship to existing ADRs

| ADR | Relationship |
|---|---|
| ADR 0083 | MCP memory server handles agent-derived facts; this handles human-curated facts |
| ADR 0097 | Constitution survives auto-clear (it is part of the system prompt, not history) |
| ADR 0100 | `fetch_knowledge` is conceptually related to the SpecSlicer; both are on-demand retrievers |
| ADR 0130 | Expand-on-demand for tool results; `fetch_knowledge` follows the same pattern for cold memory |

---

## Trade-offs accepted

**Stale constitution risk.** If the constitution says "use thiserror" but the project has moved to anyhow, the agent is misled. Mitigation: the constitution is version-controlled; drift is a PR review concern.

**Specialist thrashing.** Loose triggers load many specialists and bloat context. Mitigation: `max_specialists_per_turn = 2` hard cap.

**Authoring burden.** Feature is opt-in (`enabled = false`). Tip-line encourages adoption without forcing it. `SPC a c` lowers the friction to write the first constitution.

---

## Out of scope

- Auto-generating the constitution from the codebase.
- Fine-grained per-symbol triggers (v1 is path-glob + keyword only).
- Sharing specialists across projects (a future `forgiven skills` registry).
- `forgiven-bench/` corpus measurements (prerequisite identified in `docs/ai-ide-specs-index.md`).
