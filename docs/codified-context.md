# SPEC: Codified Context Infrastructure (Option B)

**Status:** Draft — not yet reviewed
**Priority:** Rank 2 of 4 in the AI-IDE architecture exploration
**Estimated size:** ~600 LoC
**Estimated time:** 1–2 weeks of focused work
**Dependencies:** ADR 0083 (MCP memory server) — partially leveraged

---

## Goal

Promote the informal `CLAUDE.md` / `AGENTS.md` convention into a first-class, three-tier context system for Forgiven. The three tiers correspond to different load frequencies and purposes:

1. **Hot memory** — a project constitution always loaded into every agent round.
2. **Warm memory** — per-domain specialist prompts, loaded when the session touches that domain.
3. **Cold memory** — on-demand specification documents retrieved via a tool call.

The design follows the three-tier pattern validated in the "Codified Context Infrastructure" paper (March 2026) on a 108,000-line C# distributed system.

---

## Problem

Forgiven currently has two context sources that should belong to a unified system:

- **ADR 0083 MCP memory server** — a knowledge graph the agent can read/write across sessions. Capable but underused: `search_nodes` is not always called on first turn, and the graph is unstructured.
- **Informal project files** — `CLAUDE.md`, `AGENTS.md`, `.cursorrules` patterns that developers write by hand. Forgiven does not recognise these as a first-class load.

The problem is three-fold:

1. **Project conventions drift.** The agent repeatedly asks for confirmation on patterns that are already established in the project (e.g. "should I use `thiserror` or `anyhow`?" when the project uses `anyhow` everywhere).
2. **First-session friction.** On every new conversation, the agent rediscovers the project structure from scratch via `get_file_outline` calls.
3. **Cold knowledge is either all-in or missing.** Large design documents either go into context permanently (expensive) or are absent (agent lacks context).

Recent empirical work (Chatlatanagulchai et al., November 2025, "Agent READMEs: An Empirical Study of Context Files for Agentic Coding") shows that 72% of Claude Code projects specify application architecture in context files, and these files evolve like configuration code. There is a documented pattern Forgiven can implement.

---

## Proposed design

### Directory layout

Inside a Forgiven-managed project:

```
.forgiven/
├── constitution.md               # Tier 1 — hot memory, always loaded
├── agents/                       # Tier 2 — warm memory, domain specialists
│   ├── backend.md
│   ├── frontend.md
│   ├── data.md
│   └── rust-style.md
└── knowledge/                    # Tier 3 — cold memory, retrieved on demand
    ├── architecture-overview.md
    ├── api-contracts.md
    ├── deployment-notes.md
    └── migration-log.md
```

Files are plain Markdown. No YAML front-matter is required in v1 (can be added later if routing becomes complex).

### Tier 1 — Constitution (hot memory)

**Target size:** ≤ 500 tokens (~2 KB).
**Load strategy:** always included in the system prompt.

The constitution captures invariants that apply to every agent turn:

- Language and framework choices ("Rust 2021 edition, tokio async runtime, anyhow for errors")
- Naming conventions ("module files are snake_case.rs, types are PascalCase, no abbreviations in public APIs")
- Hard constraints ("never introduce `unsafe` blocks", "all tool functions return `Result<T, Error>`")
- Orchestration hints ("prefer symbol tools over file tools; see `docs/context-efficiency.md`")

Template at `.forgiven/constitution.md`:

```markdown
# Project Constitution

## Language
Rust 2021, edition 2024. MSRV 1.82.

## Style
- anyhow::Result everywhere except at public API boundaries
- Never introduce unsafe blocks — forbidden project-wide in Cargo.toml
- snake_case files, PascalCase types, SCREAMING_SNAKE for consts

## Architecture
- src/agent/     — agent loop, tool dispatch, streaming
- src/editor/    — event loop, modal state, rendering entry points
- src/buffer/    — text buffer, cursor, undo/redo

## Hard rules
- No new background threads — use tokio::spawn inside the existing runtime
- Config keys go in src/config/mod.rs with a default_<name>() function
- All ADRs numbered sequentially in docs/adr/NNNN-kebab-case.md
```

### Tier 2 — Specialists (warm memory)

**Target size per file:** ≤ 800 tokens.
**Load strategy:** injected when the session matches a trigger.

Each specialist is a Markdown file with a frontmatter-optional trigger spec:

```markdown
---
trigger:
  paths: ["src/agent/**", "src/mcp_servers/**"]
  keywords: ["agent", "tool call", "MCP", "streaming"]
---

# Agent Specialist

## Where agents run
[... specialist content ...]
```

Trigger logic: load the specialist when either (a) the current open file matches one of the `paths` globs, or (b) the user's raw message matches two or more `keywords`.

Multiple specialists can be active in a single turn. They are appended to the system prompt in order of load priority (constitution > triggered specialists).

### Tier 3 — Knowledge (cold memory)

**Load strategy:** retrieved via a new `fetch_knowledge(name: string)` tool.

Knowledge documents are not loaded automatically. They are listed in the system prompt as a one-line catalogue:

```
Knowledge base (call fetch_knowledge(name) to retrieve):
- architecture-overview (Service topology, data flow)
- api-contracts (Public API schemas and versioning)
- deployment-notes (Production deployment sequence)
- migration-log (Historical migrations and their rationales)
```

The one-line description is read from the first Markdown paragraph (or `## Summary` section if present).

### Configuration

```toml
[agent.codified_context]
# Enable the three-tier loader. Default: false until validated.
enabled = false

# Path to the .forgiven directory (relative to project root, or absolute)
directory = ".forgiven"

# Hard cap on constitution size in tokens. Larger triggers a warning.
constitution_max_tokens = 500

# Max number of specialists loaded per turn (if more match, priority wins)
max_specialists_per_turn = 2

# Cold memory size cap per fetch_knowledge() call (bytes)
knowledge_fetch_max_bytes = 8192
```

### Authoring UX

- `SPC a c` — open the constitution in the editor.
- `SPC a C` — list all specialists; select one to edit.
- `SPC a k` — list all knowledge documents; select one to edit.

On first agent turn in a project without `.forgiven/`, emit a non-intrusive status line:

```
[tip] Create .forgiven/constitution.md to improve agent consistency across sessions.
```

Do not block, do not nag, do not auto-create.

### Relationship to the MCP memory server (ADR 0083)

The MCP memory server remains authoritative for **session-level, dynamically-accrued** knowledge: "the user fixed bug X on April 14", "this function was refactored yesterday". The file-based tiers are authoritative for **project-level, human-curated** knowledge.

Clear boundary:

| Fact type | Where it lives |
|---|---|
| Language, style, conventions | Constitution |
| Per-domain patterns and pitfalls | Specialist |
| Architecture docs, design rationale | Knowledge |
| "User prefers X" | MCP memory |
| "This bug was fixed in PR #123" | MCP memory |
| "On 2026-04-19 we decided to use Y" | MCP memory |

When in doubt: if a human wrote it, it lives in `.forgiven/`. If the agent derived it, it lives in MCP memory.

---

## Acceptance criteria

- [ ] `CodifiedContext` struct loads from `.forgiven/` on session start.
- [ ] Constitution is appended to system prompt when present.
- [ ] Specialist trigger logic evaluates open-file glob + keyword match.
- [ ] `fetch_knowledge(name)` tool registered and returns trimmed Markdown.
- [ ] `SPC a c`, `SPC a C`, `SPC a k` keybinds open the relevant files.
- [ ] If `.forgiven/` is absent, the tip line appears once per session and can be suppressed via config.
- [ ] Constitution exceeding `constitution_max_tokens` produces a warning in `SPC d`.

---

## Measurement plan

Two metrics on the `forgiven-bench/` corpus:

| Metric | Target |
|---|---|
| Tokens spent on exploratory `read_file` / `get_file_outline` calls in first 3 rounds | ≥ 30% reduction with constitution + specialists active |
| Answer quality (F1 vs. golden) | No drop; ideally +2 to +5 pp from better conventions adherence |

Additional qualitative measurement: review 10 real agent sessions before and after. Count "conventions violations" (agent introduces style inconsistent with the project). The constitution should eliminate most.

---

## Risks and trade-offs

**Stale constitution.** If `.forgiven/constitution.md` says "we use `thiserror`" but the project has migrated to `anyhow`, the agent is misled. Mitigation: the constitution is version-controlled alongside the code; drift is a PR review issue.

**Over-specification.** An ambitious constitution with 20 rules can overwhelm simple tasks. Mitigation: the `constitution_max_tokens = 500` cap with warning forces discipline.

**Specialist thrashing.** If triggers are loose, many specialists load per turn and context bloats. Mitigation: `max_specialists_per_turn = 2` hard cap; priority ordering.

**Authoring burden.** Developers must write and maintain the files. Mitigation: the feature is opt-in (`enabled = false` by default). The tip line encourages adoption without forcing it.

---

## Out of scope

- Auto-generating the constitution from the codebase (separate, larger effort).
- Sharing specialists across projects (a future `forgiven skills` registry).
- Version-pinning: the loader reads whatever is on disk; git determines history.
- Fine-grained per-symbol triggers (v1 is path-globs + keywords only).

---

## Implementation order

1. Define `CodifiedContext`, `Constitution`, `Specialist`, `KnowledgeDoc` structs.
2. Implement the loader for `.forgiven/` with graceful absence handling.
3. Wire constitution injection into `build_system_prompt()`.
4. Implement specialist trigger matching (path globs + keyword counts).
5. Register `fetch_knowledge` tool and add to the tool schema.
6. Add `SPC a c`, `SPC a C`, `SPC a k` keybinds.
7. Add tip-line rendering.
8. Add `SPC d` constitution size indicator.
9. Write integration tests.
10. Document in a new ADR.

Total: 1–2 weeks focused.

---

## Related work

- **ADR 0083** — MCP memory server. Complementary: the codified context handles human-curated knowledge; MCP memory handles agent-derived facts.
- **ADR 0097** — spec-kit auto-clear. The constitution survives auto-clear because it is part of the system prompt, not conversation history.
- **ADR 0126** — token-efficiency analysis. Notes that hot-memory-style guidance is partially present; this spec formalises it.
- **ADR 0100** — SpecSlicer. The Tier 3 `fetch_knowledge` tool is conceptually similar to a generalisation of the spec slicer.

## References

- Codified Context Infrastructure paper (March 2026, arXiv:2602.20478) — three-tier pattern validated on a 108,000-line C# distributed system, 283 development sessions, 19 specialised agents, 34 cold-memory documents.
- Chatlatanagulchai et al. (November 2025). *Agent READMEs: An Empirical Study of Context Files for Agentic Coding.* arXiv:2511.12884. — Empirical analysis of 2,303 agent context files across 1,925 repositories. Establishes that the shallow-hierarchy Markdown pattern is standard.
