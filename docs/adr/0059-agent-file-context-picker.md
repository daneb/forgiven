# ADR 0059 — Agent File Context Picker (Ctrl+P)

**Date:** 2026-03-12
**Status:** Accepted

---

## Context

The agent panel already injects the **currently open buffer** into the system prompt at submit time. This covers the common "I'm looking at this file, help me with it" case, but leaves three gaps:

1. **Multi-file reasoning** — tasks that span several files (e.g. "refactor the auth module to match the pattern in the config module") require the user to manually paste content or rely on the agent's `read_file` tool, which costs tool-call round-trips and is invisible to the model until it decides to call it.

2. **Deliberate context selection** — the user may want to say "look at *this* file specifically" rather than whichever buffer happens to be in focus.

3. **Discoverability** — there was no affordance in the UI to suggest that adding extra file context was possible at all.

### What other editors do

| Editor | Mechanism | Notes |
|---|---|---|
| VS Code Copilot Chat | `#file:path` picker | File shown as chip in input |
| Cursor | `@file` inline trigger | File shown as pill badge |
| Zed | `/file` slash command | Fuzzy picker in command bar |

All inject full file content at send time. All cap or warn on large files. None use the LSP/tool layer for this — it is an explicit user gesture, not automatic retrieval.

### Design constraints

- **No new `Mode` variant** — the picker must be a sub-state of `Mode::Agent`, consistent with the existing `slash_menu` and `asking_user` patterns.
- **Reuse existing infrastructure** — `scan_files()`, `fuzzy_score()`, and `file_all` already exist for `Mode::PickFile` (Ctrl+O); they should not be duplicated.
- **Large file safety** — files over 500 lines must be truncated with a visible notice to prevent accidental context-window saturation.
- **`@` is off-limits as a trigger** — `@` appears legitimately in Rust code, email addresses, and decorator syntax; intercepting it mid-input would cause false positives.

---

## Decision

### Trigger: `Ctrl+P` in agent mode

`Ctrl+P` was confirmed unbound across all `KeyModifiers::CONTROL` usages in `editor/mod.rs`. It opens a fuzzy file picker overlay anchored directly above the agent input box, closing when the user confirms or cancels.

### `AtPickerState` — sub-state on `AgentPanel`

```rust
pub const AT_PICKER_MAX_LINES: usize = 500;

pub struct AtPickerState {
    pub query: String,
    pub results: Vec<(PathBuf, Vec<usize>)>,  // (path, fuzzy match indices)
    pub selected: usize,
}
```

`AgentPanel.at_picker: Option<AtPickerState>` — `None` when the picker is closed, `Some` while open. This follows the same pattern as `slash_menu: Option<SlashMenuState>`.

### `file_blocks` — attached file accumulator

```rust
// on AgentPanel:
pub file_blocks: Vec<(String, String, usize)>,  // (display_name, content, line_count)
```

Files accumulate here until `submit()` drains them via `std::mem::take`, exactly as `pasted_blocks` works.

### Key interception priority in `handle_agent_mode()`

```
1. asking_user.is_some()  →  ask_user dialog      (highest — agent is blocked)
2. at_picker.is_some()    →  handle_at_picker_key() (NEW)
3. slash_menu.is_some()   →  slash menu navigation
4. normal input match     →  includes Ctrl+P → open_at_picker()
```

### File reading and truncation

`read_file_for_context(path, project_root)` — a free function in `editor/mod.rs`:

- Returns `(display_name, content, line_count)`.
- `display_name` is the cwd-relative path (for badge display and message header).
- Files exceeding `AT_PICKER_MAX_LINES` (500) are truncated; a `[Truncated: showing 500/N lines]` notice is appended to the content.
- Binary files produce a UTF-8 decode error from `read_to_string`, which surfaces as a status-bar message rather than a crash.

### Message assembly in `submit()`

File blocks are prepended before pasted blocks and typed input, in structured fenced form:

```
File: src/auth/mod.rs

```rust
// ... file content (possibly truncated) ...
```

File: src/config/mod.rs

```rust
// ... file content ...
```

[pasted block 1]

[typed input]
```

The order (files → pastes → typed text) reflects information hierarchy: files are explicit structured context; pastes are ad-hoc snippets; typed text is the user's question.

### UI

**Picker overlay** (`render_at_picker`):
- Positioned directly above the input box (same anchor as `render_slash_menu`).
- Width: matches input box width.
- Height: `1 (query line) + min(results, 15) + 1 (hint) + 2 (borders)`, capped at the vertical space above the input.
- Border: `LightGreen`, title: ` Attach file  (↑/↓ navigate · Enter attach · Esc cancel) `.
- Fuzzy match characters highlighted `Yellow + Bold`; selected row `Rgb(40, 60, 90)` background — identical to `render_file_picker`.

**File block badges** (in input area, above pasted-block badges):
- Style: `LightGreen + DIM` — distinguishable from pasted blocks (`Cyan + DIM`).
- Label: `  📎 src/foo.rs (142 lines)`.

**Hint text** updated: `Ctrl+P=attach file` added to the input box title.

---

## Implementation

| File | Change |
|---|---|
| `src/agent/mod.rs` | `AT_PICKER_MAX_LINES` constant; `AtPickerState` struct; `file_blocks` + `at_picker` fields on `AgentPanel` and `AgentPanel::new()`; `submit()` guard extended; file block assembly (fenced, before pasted blocks) |
| `src/editor/mod.rs` | `read_file_for_context()` free fn; `open_at_picker()`, `refilter_at_picker()`, `handle_at_picker_key()` methods; `Ctrl+P` arm in `handle_agent_mode()`; `at_picker` interception block |
| `src/ui/mod.rs` | `AtPickerState` import; `render_at_picker()` function; file block badge rendering (LightGreen+DIM) in `render_agent_panel()`; input height calculation includes `file_blocks.len()`; hint text updated |

No new dependencies. Existing `scan_files()`, `fuzzy_score()`, and `file_all` are reused unchanged.

---

## Consequences

- **Positive**: Users can attach any project file as explicit context with a single keypress — closes the multi-file reasoning gap without requiring extra tool-call round-trips.
- **Positive**: Fuzzy search handles large projects; the picker re-uses the battle-tested `fuzzy_score()` already used by `Mode::PickFile`.
- **Positive**: 500-line truncation prevents accidental context-window exhaustion; the notice tells the model (and the user) that the file was truncated.
- **Positive**: File block badges are visually distinct from pasted blocks, making it clear at a glance what context will be sent.
- **Positive**: No new `Mode` variant — the feature slots cleanly into the existing `AgentPanel` sub-state pattern, keeping the mode machine simple.
- **Negative**: File content is snapshotted at attachment time, not at submit time. If the user edits the file between attaching and submitting, the attached version is stale. This matches the existing behaviour for pasted blocks and the current-buffer context.
- **Negative**: `scan_files()` is called unconditionally on every `Ctrl+P` press. For large repos (> 5k files) this is a brief, perceptible scan. A background-refresh strategy (e.g. watching for fs-notify events) is a future improvement.
- **Negative**: No per-file removal — attached files accumulate until submit (same limitation as `pasted_blocks`). A future iteration could add a backspace-on-empty-input gesture to pop the last file block.
