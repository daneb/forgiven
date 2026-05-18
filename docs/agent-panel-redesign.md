# Agent Panel Redesign

## Goal

Promote the agent panel from a secondary side-pane to the primary workspace.
The editor buffer shrinks to a reference/edit surface; the agent panel becomes
the dominant pane and gains persistent chrome (title bar, token budget footer).

---

## Phase 0 — Layout & Chrome

### Slices

| ID    | Name                    | Status |
|-------|-------------------------|--------|
| P0-S1 | Audit current layout    | ☑      |
| P0-S2 | Promote agent panel     | ☑      |
| P0-S3 | Token budget status bar | ☑      |
| P0-S4 | Panel title bar         | ☑      |
| P0-S5 | Layout render tests     | ☑      |

---

### P0-S1 — Audit current layout

Document the existing split percentages, pane ownership, and z-order in a
comment block at the top of `src/ui/mod.rs`.  No behaviour change.

### P0-S2 — Promote agent panel to primary pane

Introduce two named constants in `src/ui/mod.rs`:

```rust
/// Agent panel width (%) when visible alongside editor only (no explorer).
const AGENT_PANEL_PCT_ALONE: u16 = 55;

/// Agent panel width (%) when visible alongside editor AND the file explorer.
/// The explorer takes a fixed 25 cols; editor fills whatever remains.
const AGENT_PANEL_PCT_WITH_EXPLORER: u16 = 50;
```

Replace the hard-coded `Percentage(40)` / `Percentage(35)` literals with these
constants.  At 120 cols with no explorer the agent panel grows from 48 to 66
columns.

### P0-S3 — Token budget status bar

Add a `Constraint::Length(1)` row between the task-strip and the input box in
the agent panel's vertical layout.  Render a single dimmed line:

```
 Tokens: 12,450 / 200,000 (6%)
```

- Source: `panel.conversation.last_prompt_tokens` and `panel.context_window_size()`
- No new counting logic; these fields are already populated by `StreamEvent::Usage`
- Colour: `DarkGray` when ≤ 49 %; `Yellow` when 50–79 %; `Red` when ≥ 80 %
- Show `" Tokens: —"` before the first submit

### P0-S4 — Panel title bar

Add a `Constraint::Length(1)` row at the **top** of the agent panel's vertical
layout (above the history block).  Render a single line:

```
 Anthropic  ·  Claude Sonnet 4  ·  S3a2f
```

Fields:
- Provider: `panel.provider.display_name()`
- Model: `panel.selected_model_display()`
- Session ID: 4-char hex derived from `panel.conversation.session_start_secs & 0xFFFF`;
  shows `"new"` before first submit

Style: `Color::Cyan` + BOLD when agent panel is focused; `Color::DarkGray`
otherwise.  Single line — never wraps.

Remove the redundant provider/model prefix from the history block's border title
(keep the scroll indicator and status suffix).

### P0-S5 — Layout render tests

Add `#[cfg(test)]` tests in `src/ui/mod.rs` using ratatui's `TestBackend`.
Create a minimal `AgentPanel` (via `AgentPanel::new()`) and call `UI::render()`
at 80, 120, and 200 column widths.  Assert the draw call does not panic and the
buffer dimensions match the requested size.

---

## Layout after Phase 0

```
┌─────────────────────────────────────────────────────────────┐
│ title_bar (Length 1)  Provider · Model · Session            │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  history (Min 1) — scrollable chat                          │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│  task_strip (Length N, hidden when empty)                   │
├─────────────────────────────────────────────────────────────┤
│  token_bar (Length 1)  Tokens: X / Y (Z%)                   │
├─────────────────────────────────────────────────────────────┤
│  input (dynamic height, 1–10 lines + badges + 2 borders)    │
└─────────────────────────────────────────────────────────────┘
```

Horizontal split (terminal width W):

| Visible panes          | Explorer | Editor       | Agent panel              |
|------------------------|----------|--------------|--------------------------|
| Explorer + Agent       | 25 cols  | remainder    | `AGENT_PANEL_PCT_WITH_EXPLORER` % |
| Agent only             | —        | remainder    | `AGENT_PANEL_PCT_ALONE` %         |
| Explorer only          | 25 cols  | remainder    | —                        |
| Neither                | —        | 100 %        | —                        |

---

## Future phases (not in scope here)

- **Phase 1** — Conversation thread UX (message editing, branching)
- **Phase 2** — Diff viewer integration
- **Phase 3** — Embedded terminal pane
