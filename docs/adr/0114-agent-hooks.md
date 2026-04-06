# ADR 0114 — Agent Hooks / Background Automation

**Date:** 2026-04-05
**Status:** Accepted

---

## Context

The agentic loop is currently reactive — the user types a prompt and presses Enter.
Competitors (Kiro, Windsurf) support event-driven automation: a configured hook
fires the agent automatically when something happens in the project (a file is
saved, a test fails, a lint error appears).

This transforms the editor from a "talk to the AI" tool into a "the AI watches
your work" tool — a meaningful differentiator for AI-first development.

The `notify` file-watcher crate is already a dependency (ADR 0064). The agentic
loop is already callable from the editor via the `MemorySave` / `JanitorCompress`
pattern in `editor/actions.rs`. The infrastructure cost is low.

---

## Decision

### Configuration model

Hooks are defined as a TOML array under `[agent]` in `~/.config/forgiven/config.toml`:

```toml
[[agent.hooks]]
trigger  = "on_save"
glob     = "*.rs"
prompt   = "The file {file} was just saved. Check it for obvious bugs or style issues and fix them silently."

[[agent.hooks]]
trigger  = "on_save"
glob     = "**/*.test.ts"
prompt   = "A test file ({file}) was saved. Run through the test logic and flag any gaps."
enabled  = false          # temporarily disabled without removing
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `trigger` | `String` | When to fire: `"on_save"` or `"on_test_fail"`. |
| `glob` | `String` | Glob pattern matched against the project-relative path of the triggering file. |
| `prompt` | `String` | Message sent to the agent. `{file}` is replaced with the file path; `{output}` (on_test_fail only) is replaced with truncated test output. |
| `enabled` | `bool` | Default `true`. Set `false` to disable without deleting the hook. |

### Trigger: `on_save`

Fires when the **editor itself saves** a file (i.e. `Action::FileSave` succeeds).
External writes detected by the file watcher (ADR 0064) do **not** trigger hooks —
the user did not initiate them and they could cause feedback loops.

### Trigger: `on_test_fail`

Fires when a test run transitions from passing to failing (pass→fail only; repeated
failures are silent). Configuration is in a separate `[agent.test]` section:

```toml
[agent.test]
command      = "cargo test"   # optional; auto-detected from project root
run_on_save  = true           # must be true to enable test runs on save
```

Auto-detection precedence: `Cargo.toml` → `cargo test`, `package.json` → `npm test`,
`pyproject.toml` / `pytest.ini` / `setup.cfg` → `pytest`.

Test output (stdout + stderr combined) is truncated to 2 000 characters before being
substituted into the `{output}` placeholder to keep prompt size bounded.

**Cooldown:** 30 seconds (longer than `on_save`'s 5 seconds — tests take time).

**Re-entry guard:** `Editor::hooks_firing: bool` is set to `true` when an
`on_test_fail` agent fires and reset to `false` when the agent returns to
`AgentStatus::Idle`. While set, `run_tests_if_configured()` is a no-op, preventing
the cycle: agent edits → save → tests → agent fires again.

Example:

```toml
[agent.test]
run_on_save = true

[[agent.hooks]]
trigger = "on_test_fail"
glob    = "**/*.rs"
prompt  = "Tests are now failing. Output:\n{output}\nDiagnose and fix the root cause."
```

### Glob matching

A minimal, dependency-free glob dialect is implemented in `src/editor/hooks.rs`:

- `*` — matches any sequence of non-separator characters
- `**` — matches any sequence of characters including path separators
- `?` — matches a single non-separator character
- All other characters match literally

Examples: `*.rs`, `src/**/*.ts`, `**/*.test.js`, `config.toml`.

### Rate limiting

Each hook has a 5-second cooldown per `(hook_index, file_path)` pair. If the agent is
already active (`status != AgentStatus::Idle`), hook firing is silently skipped —
no queuing, no retry. This prevents runaway loops.

The cooldowns are tracked in `Editor::hook_cooldowns: HashMap<usize, Instant>`,
keyed by hook index. Resetting on new conversation is not necessary — the Instant
comparison naturally expires.

### Agent panel behaviour on hook fire

1. The agent panel is made visible (`agent_panel.visible = true`).
2. A system message is prepended to the chat: `── Hook: on_save → src/foo.rs ──`
   so the user knows what triggered the agent.
3. The hook prompt (with `{file}` substituted) is set as the user input and
   `submit()` is called using the same `block_in_place` pattern used by
   `MemorySave` and `JanitorCompress`.
4. Only the **first** matching hook fires per save (hooks are evaluated in config
   order; subsequent matching hooks are skipped). This keeps the behaviour
   predictable.

### Visibility / cancellation

Because the agent panel opens and the chat is populated, the user can:

- Watch the hook agent in real-time (tokens streaming in the panel).
- Press `Ctrl+C` (existing abort keybind) to cancel the hook agent mid-run.
- Disable the hook in config and reload to stop future fires.

No new mode or keybind is required.

---

## Files modified / created

| File | Change |
|------|--------|
| `src/config/mod.rs` | `AgentHook` struct; `hooks: Vec<AgentHook>` in `AgentConfig`; `TestConfig { command, run_on_save }` in `AgentConfig` |
| `src/editor/mod.rs` | `hook_cooldowns`, `last_test_passed`, `hooks_firing` fields; reset `hooks_firing` in tick loop |
| `src/editor/hooks.rs` | `glob_matches()`, `fire_hooks_for_save()`, `run_tests_if_configured()`, `fire_hooks_for_test_fail()`, `detect_test_command()` |
| `src/editor/actions.rs` | Call `fire_hooks_for_save()` and `run_tests_if_configured()` after `FileSave` |

---

## Alternatives considered

### Hook on external file change (via `notify` watcher)
Also a valid trigger. Deferred — external changes are initiated by other tools
(compilers, formatters) and are harder to rate-limit without causing loops.
`on_save` is the most predictable trigger for the first version.

### Queuing hook jobs when agent is busy
If the agent is running and a save fires, we could queue the hook. Rejected:
queued hooks fire after a user-initiated conversation ends, which is confusing.
Skip-if-busy is the safer default.

### Per-hook max_rounds override
Hooks don't need the same round budget as interactive sessions. A future revision
could add `max_rounds = 3` per hook. Deferred — default max_rounds is already
configurable globally.

### `on_lint_error` trigger
Would fire when LSP delivers `Error`-level diagnostics. Deferred — LSP diagnostics
arrive asynchronously and repeatedly during typing; rate-limiting without debouncing
is complex. `on_save` is a clean discrete event.

---

## Consequences

**Positive**

- Proactive AI assistance without user prompting.
- Zero new crates — glob matching is implemented inline.
- Reuses the existing `submit()` / `block_in_place` pattern.
- Fully opt-in: no hooks fire unless the user defines them in config.
- Agent panel auto-opens so the hook is always visible.

**Negative / trade-offs**

- `on_lint_error` deferred — LSP diagnostics arrive asynchronously during typing; rate-limiting without debouncing is complex.
- First matching hook wins; multiple hooks cannot fire for the same save event.
- No hook output is stored separately — it goes into the normal chat history,
  which may interrupt an in-progress conversation if the user was mid-type.
- `max_rounds` for hooks is the global default, not per-hook.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0011](0011-agentic-tool-calling-loop.md) | Agentic loop that hooks invoke |
| [0064](0064-filesystem-watcher-external-reload.md) | `notify` watcher already in place |
| [0112](0112-agent-checkpoints.md) | Hook agent edits are snapshotted like interactive edits |
