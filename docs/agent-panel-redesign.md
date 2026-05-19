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

---

## Phase 1 — Streaming & Copy UX

### Slices

| ID     | Name                              | Status |
|--------|-----------------------------------|--------|
| P1-S1  | Raise MAX_TOKENS_PER_FRAME → 256  | ☑      |
| P1-S2  | Per-paragraph markdown cache      | ☑      |
| P1-S3  | Throughput benchmark              | ☑      |
| P1-S4  | AgentNavState struct              | ☑      |
| P1-S5  | Tab / j / k nav mode              | ☑      |
| P1-S6  | `y` yanks current line            | ☑      |
| P1-S7  | `Y` copies fenced code block      | ☑      |
| P1-S8  | Nav state + yank tests            | ☑      |

### Decisions that diverged from the original plan

**Nav mode activation**: Tab toggles `AgentNavState.active`; Esc or Tab again
exits.  The original plan described a vim-style visual selection; the final
implementation uses a lighter cursor-line highlight to avoid conflicts with the
existing Normal/Visual key dispatch.

**Render rate cap**: `AGENT_RENDER_INTERVAL` of 50 ms (≤ 20 Hz) is applied only
when streaming is the sole reason to repaint.  When another source (keyboard,
watcher) already set `needs_render`, the frame fires immediately.

---

## Phase 2 — Harness Capabilities

### Slices

| ID      | Name                                       | Status |
|---------|--------------------------------------------|--------|
| P2-S1   | Extend build_structural_map() to non-.rs   | ☑      |
| P2-S2   | Symbol reference graph                     | ☑      |
| P2-S3   | PageRank over reference graph              | ☑      |
| P2-S4   | Ranked repo-map injection                  | ☑      |
| P2-S5   | Repo map token cap (default 4 096)         | ☑      |
| P2-S6   | PageRank tests                             | ☑      |
| P2-S7   | Project init / constitution.md             | ☑      |
| P2-S8   | Session serialisation to .forgiven/        | ☑      |
| P2-S9   | Load most-recent session on startup        | ☑      |
| P2-S10  | /plan command + plan.md persistence        | ☑      |
| P2-S11  | Session harness tests                      | ☑      |

### Decisions that diverged from the original plan

**Repo-map algorithm**: The plan specified simple reference counting.
Implementation uses a proper PageRank (damping = 0.85, convergence 1e-6) over
the symbol reference graph so transitively important files rank higher.  No
new crate dependency — iterative matrix multiplication in stdlib.

**Session resume UI**: Restore happens automatically on the first submit if a
`.forgiven/sessions/<id>.json` exists from a previous session.  A system
message shows "Session restored" and the user can dismiss with Ctrl+Backspace.
The opt-out key binding (Ctrl+D at the prompt) was not implemented; the
message serves as the notification.

**Constitution prompt**: Emitted as the first user message on `project_init()`
and stored to `.forgiven/constitution.md`.  The prompt is fixed (not
configurable per project in this phase).

---

## Phase 3 — Auto-Compaction & Token Management

### Slices

| ID     | Name                                       | Status |
|--------|--------------------------------------------|--------|
| P3-S1  | Auto-compact at 70 % of context window     | ☑      |
| P3-S2  | Hysteresis — no double-fire                | ☑      |
| P3-S3  | ⚡ / ✓ compact UX with N→M token counts   | ☑      |
| P3-S4  | ADR invariant debug_assert                 | ☑      |
| P3-S5  | Per-tool token cost in activity log        | ☑      |
| P3-S6  | /compact manual command                    | ☑      |
| P3-S7  | Threshold / hysteresis / invariant tests   | ☑      |

### Decisions that diverged from the original plan

**No `janitor_threshold_tokens` config key**: The plan proposed a new config
field.  Implementation computes the threshold dynamically as
`context_window * 70 / 100` each round using the live `context_window_size()`
value.  This automatically adapts to model switches mid-session and avoids
stale config values when the user switches providers.

**`run_janitor_compress()` shared method**: The compress-then-submit logic was
extracted from the `Action::AgentJanitorCompress` match arm into a shared
`Editor::run_janitor_compress()` called by both the action arm (`SPC a j`) and
the event-loop auto-compact path.  This was not in the original plan but
eliminates duplication.

**`/compact` routes through `pending_auto_compact`**: Rather than a separate
code path, `/compact` detected in `submit()` sets `pending_auto_compact = true`
and returns early.  The event loop picks it up next tick and calls
`run_janitor_compress()`, giving identical behaviour to the auto-trigger.

**`StreamEvent::ToolDone` token cost**: Added `token_cost: u32` (chars/4
heuristic) to the existing event rather than a separate `TokenDelta` event.
This keeps the event enum flat and avoids ordering dependencies between events.
