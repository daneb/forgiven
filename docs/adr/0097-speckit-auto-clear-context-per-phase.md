# ADR 0097 — Spec-Kit Auto-Clear Context Per Phase

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

Spec-kit (ADR 0056) is a seven-phase prompt framework:

| Phase | Command | Primary file reads |
|-------|---------|-------------------|
| 1 | `/speckit.constitution` | — |
| 2 | `/speckit.specify` | `CONSTITUTION.md` |
| 3 | `/speckit.plan` | `CONSTITUTION.md`, `SPEC.md` |
| 4 | `/speckit.tasks` | `CONSTITUTION.md`, `SPEC.md`, `PLAN.md` |
| 5 | `/speckit.implement` | `CONSTITUTION.md`, `SPEC.md`, `PLAN.md`, `TASKS.md` + all source files touched |
| 6 | `/speckit.clarify` | All spec `.md` files in all feature folders |
| 7 | `/speckit.analyze` | All spec `.md` files + relevant source files |

Each command is designed to be run as a **discrete, self-contained task** that
reads the spec documents it needs via tool calls. The document outputs are saved
to disk, so the next phase always has fresh, authoritative inputs available.

The problem: users typically chain phases in a single editor session without
pressing `SPC a n` (new conversation, ADR 0077) between them. This means:

1. The `/speckit.constitution` turn — including the model's lengthy
   `CONSTITUTION.md` output — stays in the conversation history.
2. When `/speckit.specify` runs, that history is re-sent to the API as context.
   The model then calls `read_file("docs/spec/CONSTITUTION.md")` *again* —
   which is already in history verbatim — doubling the token cost.
3. By `/speckit.implement`, the accumulated history of phases 1–4 can exceed
   **20 000–40 000 tokens**, all re-sent on every tool-calling round of the
   implementation loop.

Each phase already has full access to its inputs via `read_file`. The
conversation history from prior phases provides no additional value — the model
re-reads the spec files anyway. The history is pure overhead.

---

## Decision

All seven built-in spec-kit commands automatically call `new_conversation()`
before injecting their template into the user turn.

This is implemented as a **`clears_context` flag** on the framework's
slash-command resolution path:

- `SpecFramework` gains a `clears_context: HashSet<String>` field listing
  commands that should auto-clear.
- `resolve()` returns `Option<(template, rest, clears_context: bool)>`.
- In `AgentPanel::submit()`, when `clears_context` is `true`, the panel calls
  `self.new_conversation(&model_display)` before building the user turn.
  The divider message (`── New conversation · <model> ──`) is inserted so the
  user sees the boundary in the chat panel.
- An `info!` log line is emitted with the `[spec]` prefix so the auto-clear is
  visible in `~/.local/share/forgiven/forgiven.log` and `SPC d → Recent Logs`.

### Why here and not in the user's hands?

`SPC a n` already exists precisely for manual context clearing. The question is
whether a spec-kit phase boundary is a good *automatic* trigger point.

Arguments for automatic clearing:
- Every spec-kit command is explicitly designed as a phase boundary — the
  Spec-Driven Development workflow is sequential and file-mediated.
- The user's intent when typing `/speckit.plan` is to run the planning phase,
  not to continue a conversation. The prior session context is irrelevant.
- The risk of accidentally losing useful context is low: spec-kit commands are
  not ad-hoc messages; they are structured workflow invocations. Any important
  decisions are already captured in the spec documents on disk.
- Without auto-clear, most users will not know to `SPC a n` between phases and
  will silently hit context bloat.

Arguments against:
- A user might have attached files or given clarifications in the current session
  that they want the next phase to see. **Counter**: those should be part of the
  user context appended after the command (`/speckit.plan my-feature [context]`);
  the template appends `rest` verbatim.
- Clearing context resets `session_rounds` and the token counters. **Counter**:
  each phase IS a new session from a token-tracking perspective.

### Custom framework opt-in

Custom frameworks (directory of `.md` files, ADR 0056) can opt specific
commands into auto-clearing via YAML-lite front-matter:

```markdown
---
clears_context: true
---

Your template content here…
```

The front-matter block is stripped before the template is sent to the model.
Commands without front-matter default to `clears_context = false` — the prior
behaviour — so existing custom frameworks are unaffected.

---

## Implementation

### `src/spec_framework/mod.rs`

**`parse_front_matter(content: &str) -> (bool, &str)`** — free function.
Looks for a leading `---\n…\n---\n` block. Parses `clears_context: true`
(case-insensitive ASCII). Returns `(clears, body)` where `body` has the
front-matter stripped and the conventional blank-line separator removed.

**`SpecFramework` struct** — new field:

```rust
clears_context: HashSet<String>,
```

**`spec_kit()`** — populates `clears_context` with all seven command names.
No changes to the template `.md` files (the built-in flag is set in code).

**`from_directory()`** — calls `parse_front_matter()` on each template file.
If `clears = true`, inserts the command name into `clears_context` and stores
`body` (front-matter-stripped) as the template text.

**`resolve()` signature change**:

```rust
// Before
pub fn resolve<'a>(&self, input: &'a str) -> Option<(&str, &'a str)>

// After
pub fn resolve<'a>(&self, input: &'a str) -> Option<(&str, &'a str, bool)>
```

Third element is `self.clears_context.contains(cmd)`.

### `src/agent/panel.rs`

The slash-command interception block is restructured to resolve into owned
`String` values first (breaking the immutable borrow on `self.spec_framework`)
before calling the mutable `self.new_conversation()`:

```rust
let resolved: Option<(String, String, bool)> =
    self.spec_framework.as_ref().and_then(|fw| {
        fw.resolve(&user_text)
            .map(|(tmpl, rest, clears)| (tmpl.to_string(), rest.to_string(), clears))
    });
let user_text = if let Some((template, rest, clears_context)) = resolved {
    if clears_context {
        let model_display = self.selected_model_display().to_string();
        info!("[spec] auto-clearing conversation before /{cmd} (clears_context = true)");
        self.new_conversation(&model_display);
    }
    if rest.is_empty() { template } else { format!("{template}{rest}") }
} else {
    user_text
};
```

### Tests added

| Test | Asserts |
|------|---------|
| `all_spec_kit_commands_clear_context` | All 7 built-in commands return `clears = true` |
| `front_matter_clears_context_parsed` | Front-matter `clears_context: true` sets flag and strips block |
| `front_matter_absent_defaults_false` | No front-matter → `clears = false`, content unchanged |
| Existing tests | Updated to destructure 3-tuple from `resolve()` |

---

## Token impact

A typical spec-kit run through all five core phases (constitution → implement)
without auto-clear accumulates approximately:

| After phase | History in next submit |
|-------------|----------------------|
| `/constitution` | ~1 500 t (model's CONSTITUTION.md output) |
| `/specify` | ~3 500 t (+ SPEC.md output) |
| `/plan` | ~6 000 t (+ PLAN.md output + Mermaid diagram) |
| `/tasks` | ~10 000 t (+ TASKS.md output) |
| `/implement` | 10 000 t + tool results per round |

With auto-clear, each phase starts at **0 t** of prior history. The model reads
the spec documents it needs fresh from disk (as the templates already instruct
it to). The only overhead eliminated is redundant re-sends of previous phases'
outputs — which the model ignores anyway because it re-reads the canonical files.

---

## Consequences

**Positive**
- Spec-kit sessions no longer accumulate cross-phase history silently.
- Each phase template runs against a clean context window, giving the model the
  maximum budget for tool calls and reasoning.
- Custom frameworks get opt-in auto-clearing without any code changes — just
  add front-matter to the `.md` file.
- The auto-clear is logged at `[spec]` prefix, visible in `SPC d → Recent Logs`
  and `~/.local/share/forgiven/forgiven.log`, so it is never a surprise.

**Negative / trade-offs**
- Any in-session context the user accumulated before the command (e.g.,
  clarifications typed as plain messages) is lost. Mitigated by: the user
  context appended after the command name (`/speckit.plan feature [context]`)
  is included in the new turn, so the user has a clear mechanism to carry
  forward what matters.
- `session_rounds` and token totals reset. This is correct — each phase is a
  distinct session for diagnostic purposes.
- The user sees a `── New conversation · <model> ──` divider appear when they
  invoke a spec-kit command. This makes the auto-clear explicit and auditable
  rather than silent.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Document `SPC a n` in spec-kit help text | Relies on users reading docs; they won't; token bloat happens silently |
| Warn but don't auto-clear | Warning fatigue; the right action is always to clear — never not to |
| Compress prior history via LLMLingua before the new phase | Compression does not help with re-sent prior-phase tool results (which often contain source code, excluded from LLMLingua); clearing is cleaner |
| Summarise prior context before clearing | Adds a background API call with latency; spec documents on disk are already the canonical summary |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0056](0056-spec-framework-integration.md) | Spec-kit framework — the feature this ADR extends |
| [0077](0077-agent-context-window-management.md) | `new_conversation()` — the mechanism used for auto-clear |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Context bloat — the underlying problem |
| [0093](0093-cap-open-file-context-injection.md) | Open-file cap — complementary fix; both reduce per-turn system prompt cost |
