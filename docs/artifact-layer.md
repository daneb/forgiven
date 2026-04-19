# SPEC: Generalised Artifact Layer (Option C)

**Status:** Draft — not yet reviewed
**Priority:** Rank 3 of 4 in the AI-IDE architecture exploration
**Estimated size:** ~800 LoC (delta over ADR 0130)
**Estimated time:** ~2 weeks focused
**Dependencies:** ADR 0130 (expand-on-demand for tool results) — this spec generalises it

---

## Goal

Generalise the expand-on-demand pattern from ADR 0130 to cover **all** large context elements, not just tool results. Every large artifact — open-file injections, MCP tool outputs, search results, image descriptions — becomes a reference by default, with full content expanded only when the agent explicitly requests it for the current round.

The target architecture mirrors Google's Agent Development Kit (ADK) "artifact + working context" separation, validated in production multi-agent systems: *"By default, agents see only a lightweight reference (a name and summary). When — and only when — an agent requires the raw data, it uses the LoadArtifactsTool. This action temporarily loads the content into the Working Context. Once the model call or task is complete, the artifact is offloaded from the working context by default."*

---

## Problem

ADR 0130 solved expand-on-demand for one class of context: tool results above `expand_threshold_chars`. Other context sources are still inserted into the prompt in full:

1. **Open-file injection** — `context_snippet` is capped at 150 lines (ADR 0093) but that cap is always loaded even when the agent never references the open file.
2. **MCP tool outputs** — the MCP memory server's `search_nodes` returns the full graph subset; large subgraphs go straight into history.
3. **Image artifacts** — clipboard-pasted images (ADR 0066) are attached as base64 blobs in full.
4. **Investigation results** — ADR 0128's subagent output is capped at 200 words, but the tool-call chain that produced it is visible in the subagent's isolated context and re-read if repeated.
5. **Search results** — `search_files` output can be hundreds of lines; ADR 0130's truncation helps but the full text is only retrievable via `expand_result`, not via targeted range queries.

Each of these has had a bespoke cap applied over time. The architecture is fragmented: there is no single concept of "artifact" that all these sources share, and there is no unified cache or reference model.

---

## Proposed design

### Core abstraction

A new module `src/agent/artifacts.rs` introduces the `Artifact` and `ArtifactStore` types:

```rust
pub struct ArtifactId(String);            // content-hash-based, stable

pub struct Artifact {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub summary: String,                   // 1–2 line description, always in prompt
    pub full_bytes: usize,                 // original size
    pub content: ArtifactContent,
}

pub enum ArtifactKind {
    OpenFile { path: PathBuf },
    ToolResult { tool: String },
    McpOutput { server: String },
    SearchResult { query: String },
    Image { mime: String },
    Investigation { query: String },
}

pub enum ArtifactContent {
    Text(String),
    Binary(Vec<u8>),                       // for images
}

pub struct ArtifactStore {
    artifacts: HashMap<ArtifactId, Artifact>,
    // Bounded by session memory budget; LRU eviction
    budget_bytes: usize,
    current_bytes: usize,
}
```

### Reference representation in history

When an artifact is created, only its reference goes into history:

```
[artifact src/agent/panel.rs (1,247 lines, openfile)
 Summary: Agent panel event loop, submit() assembly, streaming.
 Expand: expand(id="openfile_8a3c")]
```

The `expand(id, range?)` tool (generalising `expand_result` from ADR 0130) returns the full content for the current round only. The reference stays in history.

### Summary generation

Summaries are produced at artifact-creation time using a local heuristic first, with optional upgrade:

- **Text artifacts (Tier 1, default):** First H1/H2 heading if Markdown, else first non-blank line, truncated to 120 chars.
- **Code artifacts:** Extract top-level item signatures (functions, types) via tree-sitter (ADR 0104 dependency). List first 3, append `... and N more`.
- **Search results:** `"{query} matched {hits} files, {lines} lines"`.
- **MCP outputs:** First 200 chars, trimmed to sentence boundary.
- **Images:** `"{mime} {w}×{h}, N bytes"`.

A future enhancement could replace the heuristic with a small-model summariser (e.g. a local 3B model), but v1 sticks to heuristics for zero latency.

### Integration points

Each existing context source is refactored to produce an Artifact:

| Source | Current behaviour | Refactored behaviour |
|---|---|---|
| Open-file injection (`context_snippet`) | Inserted into system prompt | `OpenFile` artifact; reference in prompt; expand on demand |
| `read_file` / `search_files` (ADR 0130) | Already truncated via `expand_result` | Migrated to unified `Artifact` + `expand` tool |
| MCP tool output | Inserted in full | `McpOutput` artifact; reference in history |
| Clipboard image | Full base64 in message | `Image` artifact; reference; `expand(id)` returns base64 to model |
| Investigation subagent result | 200-word summary injected | Full subagent trace stored as Artifact; summary in history; agent can `expand` if needed |

### Tool consolidation

`expand_result` from ADR 0130 is renamed to `expand` with a broadened signature:

```json
{
  "name": "expand",
  "description": "Retrieve the full content of any artifact referenced by ID in history.",
  "parameters": {
    "id": { "type": "string", "description": "The artifact ID" },
    "range": {
      "type": "object",
      "properties": {
        "start": { "type": "integer" },
        "end": { "type": "integer" }
      },
      "description": "Optional byte (text) or byte (binary) range."
    }
  }
}
```

The old `expand_result` remains as a deprecated alias for one minor version.

### Cache budget

`ArtifactStore` has a per-session budget (default: 2 MB). When exceeded, LRU eviction removes the least-recently-accessed artifact. If an artifact is evicted and the agent later tries to expand it, the expand tool returns:

```
[artifact evicted from cache; re-generate by calling the tool that produced it:
  <suggestion based on ArtifactKind>]
```

The suggestion for `OpenFile` is `read_file("path")`; for `SearchResult`, a fresh `search_files` call; for `Investigation`, nothing (read-only subagent — cannot be recreated automatically).

### Telemetry in `SPC d`

```
  Artifacts in store:
    8 total   1.2 MB / 2.0 MB budget
    3 open files, 4 tool results, 1 mcp output
  Expand calls this session: 5
  Evictions this session: 0
```

### Configuration

```toml
[agent.artifacts]
# Enable the unified artifact layer. Default: false during rollout; true after validation.
enabled = false

# Per-session cache budget (bytes)
budget_bytes = 2097152  # 2 MB

# Summary strategy: "heuristic" | "local-llm"
summary_strategy = "heuristic"

# Evict policy: "lru" (default) | "fifo"
eviction = "lru"

# Tool name to expose (for backward compatibility with ADR 0130)
expand_tool_name = "expand"   # or "expand_result" during transition
```

---

## Acceptance criteria

- [ ] `Artifact`, `ArtifactStore`, `ArtifactId`, `ArtifactKind`, `ArtifactContent` defined in `src/agent/artifacts.rs`.
- [ ] Open-file injection converted to produce an `OpenFile` artifact; reference inserted into system prompt; full content served via `expand`.
- [ ] `read_file`, `search_files`, `get_file_outline`, `get_symbol_context` migrated to produce `ToolResult` artifacts.
- [ ] MCP tool outputs wrapped as `McpOutput` artifacts.
- [ ] Image artifacts (from ADR 0066 paste) wrapped as `Image` artifacts.
- [ ] `expand_result` from ADR 0130 aliased to the new `expand` tool.
- [ ] LRU eviction implemented and tested.
- [ ] `SPC d` shows artifact telemetry.
- [ ] Integration tests: (1) create artifact, reference in prompt, expand returns full, (2) evict and re-request returns eviction message, (3) budget accounting is correct across create/expand/evict cycles.

---

## Measurement plan

Against `forgiven-bench/` corpus:

| Metric | Target |
|---|---|
| Mean tokens per task | ≥ 15% additional reduction over ADR 0130 baseline |
| Answer quality (F1 vs. golden) | No drop > 2 pp |
| Expand tool call rate | Measure — expect 1–3 calls per task on average |
| Artifact store hit rate | ≥ 90% (artifacts are usually expanded at least once) |

Diminishing returns warning: ADR 0130 already captured the largest benefit from this pattern for tool results. The marginal gain from extending it to open-file, MCP, and images is smaller. The justification for the full refactor is architectural coherence (one pattern, one cache, one tool) rather than token savings alone.

---

## Risks and trade-offs

**Latency per round.** Creating artifacts adds a small overhead (hashing, heuristic summary). For heuristic summaries, target < 5 ms per artifact.

**Expand tool call overhead.** Every expansion is a tool round-trip. If the agent expands everything reflexively, net token cost increases. Mitigated by sharpening the reference summary so the agent can usually answer without expanding.

**Open-file feedback loop.** If the agent always needs the open file and always expands it, this is a regression vs. today's always-inject. Mitigation: make the OpenFile summary include the cursor position, visible line range, and symbol at cursor. This gives the agent enough to decide whether to expand.

**Eviction surprises.** If an evicted artifact is referenced in history but cannot be expanded, the agent may get confused. Mitigation: the eviction message explicitly tells the agent how to regenerate.

**Migration effort.** Five context sources must be refactored. Each is tested independently; regressions in any single source block the cut-over.

**Investigation subagent preservation.** ADR 0128's subagent is already isolated; the `InvestigationArtifact` wrapping is cosmetic. Don't over-complicate it.

---

## Out of scope

- Persisting artifacts across editor restarts.
- Artifact content diffs (showing what changed when a file is re-read).
- Cross-session artifact sharing.
- Artifact-level access control (all artifacts are always accessible to the main agent and subagents in the same session).

---

## Implementation order

1. Define the `Artifact` types and the `ArtifactStore` with LRU eviction.
2. Implement heuristic summary generators for each `ArtifactKind`.
3. Alias ADR 0130's `expand_result` as `expand` and migrate its tool-result cache into `ArtifactStore` without breaking behaviour.
4. Migrate `OpenFile` — riskiest because it's always present; feature-flag with `enabled = false`.
5. Migrate MCP outputs.
6. Migrate images.
7. Wrap investigation subagent results as `Investigation` artifacts.
8. Add `SPC d` telemetry.
9. Write integration tests.
10. Benchmark against `forgiven-bench/`.
11. Document in a new ADR.

Total: ~2 weeks focused.

---

## Related work

- **ADR 0130** — Expand-on-demand for tool results. This spec generalises the same pattern to all artifact types.
- **ADR 0093** — Open-file context cap at 150 lines. Retained as the cap for the expanded artifact; the reference in history is much smaller.
- **ADR 0083** — MCP memory server. Its outputs become `McpOutput` artifacts.
- **ADR 0066** — Image clipboard paste. Images become `Image` artifacts.
- **ADR 0128** — Investigation subagent. Its outputs become `Investigation` artifacts.
- **ADR 0104** — Tree-sitter integration. Used for code-artifact summary generation.

## References

- Google Developers Blog (December 2025). *Architecting efficient context-aware multi-agent framework for production.* — Describes ADK's artifact/working-context separation as a production pattern. Core principle reused here: "Separate storage from presentation."
