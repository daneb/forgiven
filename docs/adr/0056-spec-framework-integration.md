# ADR 0056 — Pluggable Prompt-Framework Integration (spec-kit)

**Date:** 2026-03-10
**Status:** Accepted

---

## Context

Structured, spec-driven workflows (requirements → architecture → tasks → implementation) produce more consistent AI output than open-ended chat, but require well-crafted prompt templates to guide the model through each phase.

GitHub's **spec-kit** project (`github/spec-kit`, ~75k stars) codifies this as Spec-Driven Development (SDD): five sequential phases each driven by a dedicated slash command that injects a structured prompt into the AI agent. spec-kit ships template files for every major AI coding assistant (Claude Code, Copilot, Cursor, etc.), but has no built-in support for custom TUI editors.

The goal is to bring spec-kit's SDD workflow into forgiven's agent panel while keeping the framework layer swappable — so users who prefer a different set of prompt templates (or want to write their own) can do so without modifying the editor.

---

## Decision

### Framework abstraction

A new module `src/spec_framework/mod.rs` introduces a `SpecFramework` struct that maps slash-command names to template strings. Two backends are supported:

| Backend | Source |
|---|---|
| `spec-kit` | 7 templates embedded at compile time via `include_str!` |
| Custom | Any directory of `.md` files; file stem = command name |

The framework is **optional and disabled by default** (`spec_framework = "none"`).

### Config

```toml
[agent]
spec_framework = "spec-kit"           # built-in SDD workflow
# spec_framework = "none"             # disabled (default when key is absent)
# spec_framework = "/path/to/my/fw"  # custom directory of .md templates
```

### Slash-command dispatch

When the user submits a message in the agent panel, `AgentPanel::submit()` checks whether the input starts with `/<command>`. If a framework is active and the command is recognised, the template is substituted for the raw command name and any trailing text is appended as user context. If the command is unknown or no framework is active, the message is forwarded unchanged — zero regression for existing usage.

```
/speckit.specify build a REST API for a todo app
```
→ the full `specify.md` template is sent as the user turn, with "build a REST API for a todo app" appended after the template's `## User context` heading.

### Built-in spec-kit commands

| Command | SDD Phase | Purpose |
|---|---|---|
| `/speckit.constitution` | 1 | Establish principles, constraints, non-goals |
| `/speckit.specify` | 2 | Elicit and document user stories + acceptance criteria |
| `/speckit.plan` | 3 | Produce technical architecture and module layout |
| `/speckit.tasks` | 4 | Decompose plan into ordered, atomic work items |
| `/speckit.implement` | 5 | Execute all tasks to produce working code |
| `/speckit.clarify` | — | Surface and resolve ambiguities across spec docs |
| `/speckit.analyze` | — | Audit codebase for drift against the spec |

---

## Implementation

| File | Change |
|---|---|
| `src/spec_framework/mod.rs` | New module: `SpecFramework`, `load_from_config()`, `resolve()`, `from_directory()` |
| `src/spec_framework/templates/speckit/*.md` | 7 template files embedded at compile time |
| `src/config/mod.rs` | Added `AgentConfig { spec_framework: String }`; wired into `Config` |
| `src/agent/mod.rs` | Added `spec_framework: Option<SpecFramework>` to `AgentPanel`; slash-command interception in `submit()` |
| `src/editor/mod.rs` | Framework loaded from config in `Editor::new()` and set on `agent_panel` |
| `src/main.rs` | Added `mod spec_framework` |

No new dependencies. Templates are compiled into the binary.

---

## Consequences

- **Positive**: Full spec-kit SDD workflow available inside the editor with no external tooling required.
- **Positive**: Swappable by config — any directory of Markdown files becomes a custom framework; dropping in a new framework requires no code changes.
- **Positive**: Zero breaking change — existing agent usage is unaffected when no framework is configured.
- **Positive**: Templates are embedded at compile time, so the binary is fully self-contained.
- **Negative**: spec-kit templates evolve upstream; the embedded copies require manual updates to stay current.
- **Negative**: No autocomplete or in-panel help for available commands yet (planned: show command list when user types `/` with empty input).
- **Negative**: Framework name is not yet surfaced in the agent panel UI (no visual indicator that a framework is active).
