# ADR 0075 — Slash-Menu Description Hints

**Date:** 2026-03-19
**Status:** Accepted

---

## Context

The spec-kit framework exposes seven slash commands
(`/speckit.constitution` through `/speckit.analyze`) that are intended to be
used in a specific workflow order. Users who set up spec-kit but rarely use it
found themselves forgetting the order and purpose of each step, causing them to
run commands out of sequence or skip phases entirely.

The existing slash-command autocomplete popup (ADR introduced alongside
spec-kit) showed command names only — no hint about what each command does or
where it sits in the workflow.

---

## Decision

Extend the slash-menu popup with a **description hint line** rendered below the
command list. The hint updates live as the user arrows through the list, showing
the step number and a one-line description for the currently highlighted command.

For custom frameworks (directory of `.md` files), descriptions are simply absent
— the hint line is omitted entirely so the popup stays the same size as before.

---

## Implementation

### `src/spec_framework/mod.rs`

**`SpecFramework` gains a `descriptions` field**

```rust
pub struct SpecFramework {
    templates: HashMap<String, String>,
    descriptions: HashMap<String, String>,   // ← new
    pub name: String,
}
```

**Built-in spec-kit descriptions** (function `speckit_descriptions()`):

| Command | Hint |
|---------|------|
| `speckit.constitution` | Step 1 · Define project principles & constraints |
| `speckit.specify`      | Step 2 · Write a feature specification |
| `speckit.plan`         | Step 3 · Create an implementation plan |
| `speckit.tasks`        | Step 4 · Break plan into actionable tasks |
| `speckit.implement`    | Step 5 · Implement a specific task |
| `speckit.clarify`      | Step 6 · Resolve ambiguities in the spec |
| `speckit.analyze`      | Analyze existing code or architecture |

Custom frameworks constructed via `from_directory()` receive an empty
`descriptions` map — `describe()` returns `None` for all commands.

**`pub fn describe(&self, cmd: &str) -> Option<&str>`** — new lookup method.

### `src/agent/mod.rs`

**`SlashMenuState` gains a `description` field**

```rust
pub struct SlashMenuState {
    pub items: Vec<String>,
    pub selected: usize,
    pub description: Option<String>,   // ← new
}
```

`description` is set to `fw.describe(selected_cmd)` whenever:
- `update_slash_menu()` creates or refreshes the menu
- `move_slash_selection()` changes the highlighted row

### `src/ui/mod.rs`

`render_slash_menu()` splits the inner area when `menu.description.is_some()`:

```
┌ commands ─────────────────────────┐
│  /speckit.analyze                 │
│ ▶/speckit.clarify                 │  ← selected (cyan highlight)
│  /speckit.constitution            │
│  /speckit.implement               │
│  /speckit.plan                    │
│  /speckit.specify                 │
│  /speckit.tasks                   │
│───────────────────────────────────│  ← DarkGray separator
│  Step 6 · Resolve ambiguities…    │  ← Yellow italic hint
└───────────────────────────────────┘
```

The popup height grows by 2 rows (separator + hint) only when a description is
available. The list scroll behaviour is unchanged.

---

## Consequences

**Positive**
- Users see step numbers and purpose inline as they cycle through commands —
  no need to memorise the workflow or consult documentation.
- Zero impact on custom frameworks: no descriptions → no extra rows.
- No new dependencies; pure rendering change.
- `cargo fmt` and `cargo clippy -D warnings` clean.

**Negative / trade-offs**
- Custom framework authors have no way to supply descriptions yet. A future ADR
  could support a sidecar `descriptions.toml` or front-matter in the `.md`
  templates.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [spec-kit integration](../adr) | Original spec-kit slash-command implementation |
| [0068](0068-which-key-dynamic-height-ask-user-dialog.md) | Which-key dynamic height — same pattern of computed popup sizing |
