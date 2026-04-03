# ADR 0100 — Spec Slicer: Virtual Context for Implement Phase

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

ADR 0099 (Phase 1) added `tiktoken-rs` token counting and the per-segment Context
Breakdown visible in `SPC d`. Phase 1 proved *where* budget was going; Phase 2
acts on that data.

The speckit roadmap (`docs/context-optimization-speckit.md`) Phase 2 goal is the
**Spec Slicer**: instead of sending the model to read full spec files on every
invocation of `/speckit.implement`, pre-extract only the active task and relevant
spec sections and inject them as a compact "virtual context" block at the start of
the user turn.

### Why implement phase only?

`/speckit.implement` is the high-frequency command — it runs repeatedly as the
implementation loop progresses. `/speckit.tasks` also benefits because the model
reads both `SPEC.md` and `PLAN.md`; knowing the active task upfront focuses it.

Earlier phases (specify, plan, constitution) are one-shot and already start with
empty context (ADR 0097). Slicing is most valuable when tasks are long-running
and the model makes multiple `read_file` calls per round.

### Token impact without slicing

A typical `TASKS.md` with 20 tasks (~2 000 t) is read in full on every
`/speckit.implement` round even though only one task is active. `SPEC.md` for a
medium feature is ~3 000–6 000 t. Combined, the model re-reads ~5 000–8 000 t of
largely irrelevant content on every round.

With slicing: the active task body is ~100–300 t; one or two relevant spec
sections are ~500–1 000 t. **Saving: ~80–90% of spec-file read cost per round.**

---

## Decision

### 1. New module: `src/spec_framework/spec_slicer.rs`

```
pub struct ActiveTask {
    pub phase_heading: Option<String>,  // ## Phase N — ... heading above this task
    pub title: String,                  // text after "- [ ] "
    pub body: String,                   // indented detail lines (inputs/outputs/acceptance)
}

pub struct VirtualContext {
    pub active_task: ActiveTask,
    pub spec_sections: Vec<SpecSection>,
}

pub struct SpecSection {
    pub heading: String,
    pub content: String,
}

pub struct SpecSlicer;
```

**`SpecSlicer::parse_active_task(tasks_md: &str) -> Option<ActiveTask>`**

Scans `TASKS.md` line-by-line:
- Tracks `## ` headings as the current phase heading.
- Returns the **first** line matching `- [ ] ` (case-sensitive), plus all
  subsequent lines that are indented by at least two spaces, as `body`.
- If no unchecked task exists, returns `None` (all tasks done).

**`SpecSlicer::slice_spec(spec_md: &str, task: &ActiveTask) -> Vec<SpecSection>`**

1. Splits `spec_md` into sections by lines that start with `## `.
2. Extracts "keywords" from the task title: words ≥ 4 characters, excluding a
   small stopword list (`with`, `from`, `into`, `that`, `this`, `have`, `will`,
   `when`, `each`).
3. For each section, checks whether the heading or first 400 characters of
   content contains any keyword (case-insensitive).
4. Returns the **first three** matching sections to bound injection size.

**`SpecSlicer::build(feature_dir: &Path) -> Option<VirtualContext>`**

Reads `<feature_dir>/TASKS.md` and `<feature_dir>/SPEC.md` from disk. Returns
`None` if `TASKS.md` is missing or has no unchecked tasks (graceful no-op).
`SPEC.md` absence is tolerated — the task portion is still returned with an empty
`spec_sections` list.

**`VirtualContext::to_prompt_block(&self) -> String`**

Formats the virtual context as a markdown block:

```
<!-- SpecSlicer: pre-extracted virtual context — saves full-file read overhead -->
## Active Task

> Phase 2 — Database Layer

- [ ] Create user table migration
  Inputs: `PLAN.md`, existing schema
  Outputs: `migrations/001_users.sql`
  Acceptance: schema applied cleanly on a fresh database.

## Relevant Spec Sections

### Authentication Requirements
...section content (up to 400 chars shown)...

---
<!-- End SpecSlicer block -->
```

### 2. Integration in `src/agent/panel.rs` `submit()`

Before the slash-command template resolution block, capture the raw command name
and the `rest` (feature name + any extra context) from `user_text`:

```rust
let spec_cmd_ctx: Option<(String, String)> = user_text
    .trim_start()
    .strip_prefix('/')
    .and_then(|s| {
        let cmd = s.split_whitespace().next()?;
        if cmd.starts_with("speckit.") {
            let rest = s[cmd.len()..].trim_start().to_string();
            Some((cmd.to_string(), rest))
        } else {
            None
        }
    });
```

After template resolution, for `speckit.implement` and `speckit.tasks`:

```rust
let user_text = if matches!(
    spec_cmd_ctx.as_ref().map(|(c, _)| c.as_str()),
    Some("speckit.implement") | Some("speckit.tasks")
) {
    // feature name is the first word of rest
    let feature = spec_cmd_ctx.as_ref()
        .and_then(|(_, r)| r.split_whitespace().next())
        .unwrap_or("");
    if !feature.is_empty() {
        let feature_dir = project_root.join("docs/spec/features").join(feature);
        match crate::spec_framework::spec_slicer::SpecSlicer::build(&feature_dir) {
            Some(vctx) => {
                let block = vctx.to_prompt_block();
                info!(
                    "[spec] SpecSlicer: injected virtual context ({} t, task: {:?})",
                    super::token_count::count(&block),
                    vctx.active_task.title
                );
                format!("{user_text}\n\n{block}")
            }
            None => user_text,
        }
    } else {
        user_text
    }
} else {
    user_text
};
```

The virtual context is appended **after** the template text. The template already
instructs the model to read `TASKS.md`; the SpecSlicer block narrows the focus
before the model begins its tool-call loop.

---

## Implementation

### Files changed

| File | Change |
|------|--------|
| `src/spec_framework/spec_slicer.rs` | New — `SpecSlicer`, `ActiveTask`, `VirtualContext`, `SpecSection` |
| `src/spec_framework/mod.rs` | Add `pub mod spec_slicer;` |
| `src/agent/panel.rs` | Capture `spec_cmd_ctx` pre-resolution; inject virtual context post-resolution |

### No Cargo.toml changes

`tiktoken-rs` (added in ADR 0099) is already a dependency. `SpecSlicer::build`
uses only `std::fs::read_to_string`.

### Graceful degradation

If `TASKS.md` is missing → `build()` returns `None` → `user_text` unchanged.
If all tasks are checked → `parse_active_task()` returns `None` → same no-op.
If `SPEC.md` is missing → `VirtualContext` has `spec_sections: vec![]` → block
still includes active task, omits spec section.

---

## Consequences

**Positive**
- `/speckit.implement` rounds immediately know the active task without a
  `read_file("TASKS.md")` round-trip. Implementation turns save 1 API round and
  ~2 000 t per invocation.
- Spec section slicing saves ~3 000–5 000 t of SPEC.md reads per round when the
  relevant content is a minority of the full file.
- `info!("[spec] SpecSlicer …")` log line is visible in `SPC d → Recent Logs`
  and `~/.local/share/forgiven/forgiven.log` so the injection is auditable.
- Falls back silently when feature directory or TASKS.md don't exist — custom
  workflows are unaffected.

**Negative / trade-offs**
- The keyword matching heuristic may miss relevant spec sections or include
  marginally relevant ones. The model still has `read_file` available; the
  virtual context narrows focus but doesn't replace access to the full files.
- The virtual context block adds ~100–1 300 t to the user turn, but this is
  always less than a full `read_file` result for the same files.
- Feature name must be the first token of the command argument. This is already
  the convention in all spec-kit templates (e.g. `/speckit.implement my-feature`).

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Intercept `read_file` tool results for spec files | Requires hooking into the agentic loop tool execution path; complex; breaks separation of concerns |
| Full semantic embedding search for spec sections | Requires an embedding model call; adds latency; overkill for markdown section matching |
| Inject only task, skip spec slicing | Spec slicing provides the larger saving on medium/large features; worth the marginal complexity |
| Slice `PLAN.md` too | PLAN.md is read once (not every round) by the implement template; the cost is lower; leave for Phase 3 |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0099](0099-context-breakdown-token-awareness.md) | Phase 1 — token counting; `token_count::count` used for SpecSlicer log line |
| [0097](0097-speckit-auto-clear-context-per-phase.md) | Auto-clear — Phase 2 SpecSlicer runs on the clean slate that ADR 0097 provides |
| [0093](0093-cap-open-file-context-injection.md) | Open-file cap — complementary; both reduce per-turn system prompt cost |
| [0056](0056-spec-framework-integration.md) | Spec-kit framework — `/speckit.implement` command resolved here |
