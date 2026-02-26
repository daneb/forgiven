# ADR 0029: Task Panel for Work Tracking

## Status
Accepted

## Context

During the development of the forgiven editor, the user observed VS Code's Copilot TODO panel feature and requested similar functionality for tracking tasks within the IDE. The existing workflow required external task management (GitHub issues, markdown files, sticky notes), which broke the flow of coding.

Key user requirements:
1. **In-IDE Task Management**: Add, edit, delete, and track tasks without leaving the editor
2. **Status Tracking**: Visual indication of task progress (Not Started, In Progress, Completed)
3. **Project-Local Storage**: Tasks should be stored per-project, not globally
4. **Keyboard-Driven UX**: Fast navigation and manipulation with vim-style keybindings
5. **Minimal Layout Impact**: Panel should not interfere with editor/agent workflow

The goal was to provide lightweight task tracking for developers working on features, bugs, or refactoring efforts without needing to context-switch to external tools.

## Decision

Implement a dedicated task panel system with JSON persistence and integrate it into the three-panel layout.

### 1. Task Data Model

Created a simple but extensible task structure in `src/tasks/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    NotStarted,
    InProgress,
    Completed,
}

impl TaskStatus {
    pub fn icon(&self) -> &str {
        match self {
            TaskStatus::NotStarted => "○",
            TaskStatus::InProgress => "⦿",
            TaskStatus::Completed => "✓",
        }
    }
    
    pub fn color(&self) -> Color {
        match self {
            TaskStatus::NotStarted => Color::DarkGray,
            TaskStatus::InProgress => Color::Yellow,
            TaskStatus::Completed => Color::Green,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: usize,
    pub title: String,
    pub status: TaskStatus,
}
```

**Design choices:**
- Three states map to typical development workflow: TODO → Doing → Done
- Visual icons (○ ⦿ ✓) provide instant status recognition
- Color coding (gray/yellow/green) reinforces visual hierarchy
- Simple flat structure (no nesting, tags, or dates) to minimize complexity

### 2. Task Panel Component

Implemented `TaskPanel` struct with core operations:

```rust
pub struct TaskPanel {
    pub visible: bool,
    pub focused: bool,
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub scroll: usize,
    pub input_buffer: String,
    pub editing: Option<usize>,
    pub next_id: usize,
}

impl TaskPanel {
    pub fn load() -> Self { /* ... */ }
    pub fn save(&self) { /* ... */ }
    pub fn add_task(&mut self, title: String) { /* ... */ }
    pub fn toggle_selected(&mut self) { /* ... */ }
    pub fn delete_selected(&mut self) { /* ... */ }
    pub fn start_edit(&mut self) { /* ... */ }
    pub fn finish_edit(&mut self) { /* ... */ }
    pub fn cancel_edit(&mut self) { /* ... */ }
    pub fn move_up(&mut self) { /* ... */ }
    pub fn move_down(&mut self) { /* ... */ }
}
```

**Key features:**
- `visible`/`focused` control rendering and input handling
- `selected` tracks cursor position for keyboard navigation
- `input_buffer` and `editing` support inline text entry
- `next_id` ensures unique task identifiers

### 3. JSON Persistence

Tasks are stored in `.forgiven/tasks.json` at the project root:

```json
{
  "tasks": [
    {"id": 1, "title": "Implement authentication", "status": "InProgress"},
    {"id": 2, "title": "Write unit tests", "status": "NotStarted"},
    {"id": 3, "title": "Update documentation", "status": "Completed"}
  ]
}
```

**Storage decisions:**
- **Project-local**: Each project has its own `.forgiven/` directory (gitignored)
- **JSON format**: Human-readable, supports manual editing if needed
- **Auto-save**: Tasks are saved after every mutation (add/edit/delete/toggle)
- **Lazy load**: Tasks loaded when panel is first opened, not on IDE startup

### 4. Keybindings

Added Mode::Tasks and new action variants:

```rust
// In keymap/mod.rs
pub enum Action {
    // ... existing actions
    TasksToggle,  // Toggle task panel visibility
    TasksFocus,   // Focus task panel from other modes
}

// Leader key bindings (SPC t)
KeyNode::Branch {
    description: Some("task"),
    children: {
        let mut map = HashMap::new();
        map.insert('t', Action::TasksToggle);
        map.insert('f', Action::TasksFocus);
        map
    },
}
```

**In-panel keybindings (Mode::Tasks):**
- `j/k` or `↓/↑` - Navigate tasks
- `a` - Add new task
- `e` - Edit selected task
- `d` - Mark selected as done
- `x` - Delete selected task
- `Space` - Toggle status (NotStarted → InProgress → Completed → NotStarted)
- `Enter` - Confirm input (when adding/editing)
- `Esc` - Cancel edit / return to Normal mode

### 5. UI Layout Integration

Modified the three-panel layout to support tasks alongside explorer:

```rust
// In ui/mod.rs
let left_sidebar_visible = explorer_visible || tasks_visible;

// Task panel takes priority over explorer when both are visible
if let Some(task_panel) = task_panel {
    if task_panel.visible {
        self.render_task_panel(frame, left_area, task_panel);
    } else if explorer_visible {
        self.render_explorer(frame, left_area, explorer, explorer_focused);
    }
}
```

**Layout strategy:**
- Tasks and explorer are mutually exclusive in the left sidebar
- Task panel gets priority when both `task_panel.visible` and `explorer.visible` are true
- Editor and agent panels remain unaffected
- Focused panel has yellow border, unfocused has dark gray border

### 6. Visual Design

The task panel displays:

```
╭─ Tasks (2/5) ────────────────────╮
│                                   │
│ ○ Fix login bug                   │
│ ⦿ Add user dashboard             ← selected (yellow highlight)
│ ✓ Setup database                  │
│                                   │
│ [a]dd [e]dit [d]one [x]delete    │
│ [Space]toggle                     │
╰───────────────────────────────────╯
```

**Visual elements:**
- **Title**: Shows completion count (2 completed out of 5 total)
- **Icons**: ○ (not started), ⦿ (in progress), ✓ (completed)
- **Colors**: Gray/Yellow/Green for status, yellow highlight for selection
- **Hints**: Bottom bar shows available actions
- **Input mode**: When adding/editing, shows text input at top with cursor

### 7. Mode Handling

Added comprehensive Mode::Tasks handling in `editor/mod.rs`:

```rust
fn handle_tasks_mode(&mut self, event: &Event) -> Result<(), Box<dyn std::error::Error>> {
    if let Event::Key(key) = event {
        match key.code {
            KeyCode::Esc => {
                if self.task_panel.editing.is_some() {
                    self.task_panel.cancel_edit();
                } else {
                    self.mode = Mode::Normal;
                    self.task_panel.focused = false;
                }
            }
            KeyCode::Enter if self.task_panel.editing.is_some() => {
                self.task_panel.finish_edit();
            }
            KeyCode::Char('a') if self.task_panel.editing.is_none() => {
                self.task_panel.start_edit();
            }
            // ... more key handlers
        }
    }
    Ok(())
}
```

**Input States:**
1. **Normal Navigation**: j/k/arrows move, Space toggles, d marks done, x deletes
2. **Input Mode**: When adding/editing, keystrokes go to input_buffer
3. **Esc Behavior**: Cancel edit if editing, else return to Normal mode

## Consequences

### Positive

1. **Improved Developer Flow**: Track work-in-progress without leaving the IDE
2. **Visual Task Progress**: Icons and colors provide instant status overview at a glance
3. **Project-Local Storage**: Tasks stay with the project, not cluttering global configs
4. **Keyboard Efficiency**: All operations accessible via vim-style keybindings
5. **Minimal Overhead**: Lazy loading and lightweight JSON storage (~1-2KB per project)
6. **Flexible Layout**: Task panel shares space with explorer, doesn't reduce editor area

### Negative

1. **UI Complexity**: Added ~500 lines of code across 5 files (tasks/mod.rs, editor/mod.rs, ui/mod.rs, keymap/mod.rs, main.rs)
2. **Storage Management**: Each project now has a `.forgiven/` directory to maintain
3. **Limited Features**: No task descriptions, due dates, priorities, or tags (intentional simplicity)
4. **Manual Sync**: Tasks don't sync with external systems (GitHub issues, Jira, etc.)

### Neutral

1. **Gitignore Recommendation**: Users should add `.forgiven/` to `.gitignore` to keep tasks local
2. **Export Capability**: Tasks can be manually exported by copying `.forgiven/tasks.json`
3. **Extensibility**: Task data model can be enhanced later (add fields, metadata, filters)

## Implementation Details

**Files Modified:**
- `src/tasks/mod.rs` (236 lines, new file) - Core task panel logic
- `src/editor/mod.rs` (~50 lines added) - Mode handling and panel integration
- `src/ui/mod.rs` (~100 lines added) - Task panel rendering
- `src/keymap/mod.rs` (~20 lines added) - Mode::Tasks, Action variants, keybindings
- `src/main.rs` (1 line added) - Module declaration

**Total LOC**: ~400 lines

**Testing Strategy:**
1. Manual testing: Create/edit/delete tasks, verify persistence
2. Edge cases: Empty task list, long task titles, rapid status toggling
3. Performance: JSON load time for 100+ tasks (acceptable < 10ms)
4. Integration: Verify layout switching between tasks/explorer

## Alternatives Considered

### 1. Markdown File-Based Tasks

Store tasks in `TODO.md` and parse with regex.

**Rejected because:**
- Requires markdown parser integration
- Harder to maintain structured data (status, IDs)
- File operations are slower than in-memory + JSON save

### 2. SQLite Database

Use SQLite for task storage with full query support.

**Rejected because:**
- Overkill for simple task tracking (< 50 tasks per project)
- Adds external dependency (sqlite3 crate)
- Complicates manual editing and debugging

### 3. Simple Text Line List

Store tasks as plain text lines without status.

**Rejected because:**
- No progress tracking or state management
- Can't distinguish between started/completed work
- Loses the visual benefits of icons and colors

### 4. Global Task Storage

Store all tasks in `~/.config/forgiven/tasks.toml`.

**Rejected because:**
- Tasks from different projects would mix together
- No clear association between tasks and project context
- Harder to clean up completed work

### 5. Integration with GitHub Issues

Fetch and display GitHub issues directly.

**Deferred because:**
- Requires GitHub authentication (out of scope for this ADR)
- Network dependency would slow down panel opening
- Not all projects use GitHub
- Could be added later as an optional enhancement

## Related Documents

- ADR 0010: File Explorer Tree Sidebar (similar sidebar component pattern)
- ADR 0006: Agent Chat Panel (three-panel layout strategy)
- ADR 0007: Vim Modal Keybindings (Mode enum extension pattern)

## Future Enhancements

Potential improvements not included in initial implementation:

1. **Task Filtering**: Filter by status (show only in-progress tasks)
2. **Task Search**: Fuzzy search task titles like file picker
3. **Task Metadata**: Add created_at, updated_at timestamps
4. **Task Priorities**: Support high/medium/low priority levels
5. **Task Descriptions**: Multi-line descriptions for complex tasks
6. **Bulk Operations**: Mark all complete, archive completed tasks
7. **GitHub Sync**: Optional sync with GitHub issues/projects
8. **Export Options**: Export to Markdown, CSV, or JSON
9. **Task Templates**: Quick-add common task types (bug, feature, refactor)
10. **Statistics View**: Show completion rate, time tracking, burndown

These can be implemented incrementally based on user feedback and usage patterns.
