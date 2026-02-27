# ADR 0031 — Agent-Driven Plan Strip (create_task / complete_task)

**Status:** Accepted (supersedes draft that referenced `TaskPanel`)

---

## Context

ADR 0029 introduced a separate `TaskPanel` for manual task tracking — a left-sidebar
component with its own `Mode::Tasks`, JSON persistence, and keyboard-driven CRUD. ADR 0011
established the agentic tool-calling loop. These two features were entirely disconnected.

When the agent described a multi-step plan in chat (e.g., "1. Create Program.cs 2. Add
HttpClient logic 3. Write NuGet config"), those steps existed only as prose. The user had
to read the plan and manually add tasks — defeating the purpose of an integrated editor.

A first-pass attempt added a `create_task` tool that drained into the existing `TaskPanel`.
During review, two additional problems surfaced:

1. **Separation of concerns** — the separate panel popped over the file explorer, disrupting
   layout, and its status never updated as the agent worked through steps.
2. **No completion signal** — the agent had no way to mark a task done. The listed items
   were purely static once created.
3. **Manual task panel complexity** — with the agent now owning the task lifecycle, the full
   `TaskPanel` (persistence, CRUD, Mode::Tasks, keybindings) became unnecessary overhead.

The decision was to remove `TaskPanel` entirely and replace it with a lightweight
**plan strip** embedded directly inside the agent panel, with full lifecycle managed by two
agent tools: `create_task` and `complete_task`.

---

## Decision

### Removed: `TaskPanel`, `Mode::Tasks`, `src/tasks/mod.rs`

The entire `src/tasks/mod.rs` module was deleted. `Mode::Tasks`, `Action::TasksToggle`,
`Action::TasksFocus`, the `SPC t` leader bindings, and the `render_task_panel()` UI
function were all removed. The editor no longer holds a `task_panel` field.

### New tools: `create_task` and `complete_task`

Two tools are added alongside `read_file`, `write_file`, `edit_file`, and `list_directory`:

```json
{
  "name": "create_task",
  "description": "Add a task to the user's task panel. Call this once per planned step at the start of any multi-step job so the user can track progress.",
  "parameters": { "title": "string" }
}
```

```json
{
  "name": "complete_task",
  "description": "Mark a previously created task as done. Use the exact same title passed to create_task.",
  "parameters": { "title": "string" }
}
```

Both executors (`execute_tool`) validate the `title` argument and return a plain
acknowledgement string. The actual UI update is handled out-of-band via the event channel,
keeping the tool executor free of UI dependencies.

### `AgentTask` struct in `AgentPanel`

```rust
pub struct AgentTask {
    pub title: String,
    pub done: bool,
}
```

`AgentPanel` gains a `tasks: Vec<AgentTask>` field. Tasks are created with `done: false`
and are cleared at the start of each `submit()` call so each new agent session starts with
a fresh plan.

### Event channel variants

```rust
StreamEvent::TaskCreated  { title: String }
StreamEvent::TaskCompleted { title: String }
```

After `execute_tool` returns for a `create_task` or `complete_task` call, the agentic loop
emits the corresponding event via a unified block:

```rust
if matches!(call.name.as_str(), "create_task" | "complete_task") && !result.starts_with("error") {
    if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
        let event = if call.name == "create_task" {
            StreamEvent::TaskCreated { title: title.to_string() }
        } else {
            StreamEvent::TaskCompleted { title: title.to_string() }
        };
        let _ = tx.send(event);
    }
}
```

`poll_stream()` handles these by pushing/updating entries in `panel.tasks`. No editor-level
drain loop is needed — the panel manages its own state.

### Plan strip UI: `render_task_strip()`

When `panel.tasks` is non-empty, a bordered "Plan (N/M)" block is rendered between the
chat history and the input box inside the agent panel:

```
╭─ Plan (1/3) ──────────────────────╮
│  ✓ Create Program.cs               │  ← done (dark gray)
│  ⊙ Add HttpClient logic            │  ← current (yellow, first incomplete)
│  ○ Write NuGet config              │  ← pending (white)
╰────────────────────────────────────╯
```

Icons and colours:
- `✓` dark gray — completed (`done: true`)
- `⊙` yellow — current step (first task where `done == false`)
- `○` white — pending (subsequent incomplete tasks)

The strip height is capped at 8 rows to avoid crowding the chat history.

### Stream display: task tools suppressed

`ToolStart` and `ToolDone` events for `create_task`/`complete_task` are suppressed from the
chat stream. The plan strip already communicates this information; repeating it in prose
would be noisy.

`StreamEvent::ToolDone` was extended with a `name: String` field so the display layer can
filter without a separate tracking structure:

```rust
Ok(StreamEvent::ToolStart { name, args_summary }) => {
    if !matches!(name.as_str(), "create_task" | "complete_task") {
        // append to streaming_reply
    }
}
Ok(StreamEvent::ToolDone { name, result_summary }) => {
    if !matches!(name.as_str(), "create_task" | "complete_task") {
        // append result to streaming_reply
    }
}
```

### Tool call paragraph fix (`\n` → `\n\n`)

Each tool call line is now prefixed with `\n\n` instead of `\n`. In CommonMark, a single
newline is a soft break (rendered as a space), so consecutive tool calls merged into one
paragraph in the markdown renderer. Double-newline creates a paragraph boundary, giving
each call its own block.

### System prompt

Rule 0 is updated to require both tools:

```
0. For any multi-step job, call create_task ONCE per planned step BEFORE doing any
   file work. After completing each step, call complete_task with the exact same title.
   This lets the user track live progress. Keep titles short and imperative.
```

---

## Consequences

**Positive**
- The plan strip is embedded inside the agent panel — it never disrupts the file explorer
  or any other layout component
- Tasks show live progress as the agent works: each `complete_task` call immediately marks
  the step done and advances the `⊙` indicator to the next step
- No persistence complexity: tasks are ephemeral, scoped to a single agent session, and
  cleared automatically on the next `submit()`
- The executor / UI separation is preserved: `execute_tool` returns plain strings; side-
  effects reach the UI only via the typed event channel
- Removing `TaskPanel` reduced total codebase by ~500 lines and eliminated `Mode::Tasks`,
  two `Action` variants, JSON load/save, and an entire left-sidebar render path

**Negative / trade-offs**
- Manual task tracking (the original purpose of ADR 0029) is no longer available. Users
  who want to track tasks independent of an agent session have no in-editor mechanism.
- The model must be instructed to call both tools — it will not do so unless the system
  prompt makes this mandatory. Models occasionally skip `create_task` for single-step tasks.
- Task titles must match exactly between `create_task` and `complete_task`. A mismatch
  causes `complete_task` to silently do nothing (the task remains incomplete).

**Future enhancements**
- Fuzzy-match title lookup for `complete_task` to tolerate minor rewording
- Persist the last plan strip for review after the agent session ends
- Show an elapsed time or step counter alongside the current task

---

## Files Changed

| File | Change |
|------|--------|
| `src/agent/tools.rs` | Added `create_task` and `complete_task` to `tool_definitions()`; added executor arms |
| `src/agent/mod.rs` | Added `AgentTask` struct; `tasks: Vec<AgentTask>` on `AgentPanel`; `StreamEvent::TaskCreated/TaskCompleted`; `name` field on `StreamEvent::ToolDone`; `poll_stream()` handlers; task tool stream suppression; `tasks.clear()` on `submit()`; updated system prompt |
| `src/ui/mod.rs` | Added `render_task_strip()`; task strip layout in `render_agent_panel()`; `\n\n` tool prefix; `AgentTask` import; removed `render_task_panel()` |
| `src/keymap/mod.rs` | Removed `Mode::Tasks`, `Action::TasksToggle`, `Action::TasksFocus`, `SPC t` bindings |
| `src/editor/mod.rs` | Removed `task_panel` field, `handle_tasks_mode()`, `Action::TasksToggle/Focus` arms, `Mode::Tasks` arm, pending-tasks drain block |
| `src/tasks/mod.rs` | **Deleted** |
| `src/main.rs` | Removed `mod tasks;` |

---

## Related

- **ADR 0011** — Agentic Tool-Calling Loop (tool execution and event channel architecture)
- **ADR 0012** — Agent UX, Context and File Refresh (`pending_reloads` / event drain pattern)
- **ADR 0029** — Task Panel for Work Tracking (superseded; see Amendment 2 in that document)
- **ADR 0022** — Markdown Rendering (CommonMark paragraph rules relevant to `\n\n` fix)
