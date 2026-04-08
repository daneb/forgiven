# ADR 0123 — Context Management v2: Observation Masking + Disk Persistence

**Date:** 2026-04-08  
**Status:** Implemented

---

## Context

ADR 0101 introduced the Auto-Janitor: a rolling-summary compression that fires
automatically when session prompt tokens exceed a configurable threshold. Over the
following sessions (ADR 0117, 0120, 0121) significant complexity was added to fix
timing bugs, context amnesia, dropped pasted blocks, and stale state across sessions.

A research pass (`docs/context-management-research.md`) evaluated what production tools
do and what the empirical evidence says about rolling-summary compression. Key findings:

1. **JetBrains NeurIPS 2025**: simple observation masking (stripping large tool output
   payloads from re-sent history, keeping the action/reasoning record intact) matches or
   beats LLM summarisation in 4 of 5 benchmarks at lower cost.
2. **The auto-trigger's bugs are intrinsic**, not incidental. The deferred-trigger state
   machine (ADR 0121), the janitor-saved block fields, and the stale-state bug in
   `new_conversation()` are all symptoms of an async LLM round inserted at an
   unpredictable point in the user's workflow.
3. **forgiven's real token costs** are already largely solved: open-file injection capped
   (ADR 0092); specKit auto-clears between phases (ADR 0097). The remaining cost is large
   tool payloads re-sent every round — exactly what observation masking addresses directly.
4. **A 90% in-chat warning** (implemented 2026-04-08) bridges the gap: the user gets a
   clear, actionable nudge with the exact keybind (`SPC a j`) before the session breaks,
   without requiring an auto-trigger.

---

## Decision

### 1. Disable auto-trigger by default

Change `default_janitor_threshold()` in `src/config/mod.rs` to return `0`.

The auto-janitor is still available as an explicit opt-in via config
(`janitor_threshold_tokens = 50000`) and always available manually via `SPC a j`.
The deferred-trigger state machine added in ADR 0121 (`pending_resubmit_after_janitor`,
`Action::AgentSubmitPending`) can be removed once the auto-trigger is disabled by default.

### 2. Implement observation masking

In the history assembly path inside `submit()` (specifically in `send_messages()` or
wherever `self.messages` is serialised into the outgoing API payload), truncate any
single message whose estimated token count exceeds a threshold (suggested: 500 tokens,
≈ 2,000 characters) to a one-line stub:

```
[tool result: ~1,240 tokens — truncated for re-send; call the tool again if needed]
```

Rules:
- Apply only to `Role::Assistant` messages that contain tool results (identified by a
  leading `✓` tool-done marker or by content length alone as a heuristic).
- **Never** truncate the most recent assistant message — the model needs that in full.
- **Never** truncate user messages — user instructions must be re-sent verbatim.
- The truncation is applied only to the API payload; `self.messages` (the display history)
  is unchanged, so the user can still scroll up and read full tool outputs.

Config key (suggested): `agent.observation_mask_threshold_chars = 2000` (0 = disabled).

### 3. Implement disk persistence before any compaction

Before `compress_history()` archives messages to `self.archived_messages`, write the full
current conversation to disk:

**Path:** `~/.local/share/forgiven/history/<ISO-date>-<session-hash>.jsonl`  
**Format:** One JSON line per message: `{"role": "user"|"assistant"|"system", "content": "…", "ts": <unix>}`

This is idempotent (each compaction appends to or creates a new file) and requires no new
infrastructure — forgiven already creates `~/.local/share/forgiven/` for `sessions.jsonl`.

Expose a `search_session_history(query: str)` tool in the agentic loop that does a simple
case-insensitive substring search over the most recent history file and returns matching
messages. The model can call this when it realises it needs something that was compressed.

**Phase 1 (simpler):** Write to disk only. No search tool yet. The user can open the file
manually or `cat` it. Validate the file format and path.

**Phase 2:** Add the `search_session_history` tool.

---

## Implementation Plan (for next session)

Work to be done in order:

| Step | File | Change |
|---|---|---|
| 1 | `src/config/mod.rs` | `default_janitor_threshold()` → return `0` |
| 2 | `src/agent/panel.rs` — `send_messages()` or API payload build | Add observation masking: truncate large assistant messages in the outgoing slice |
| 3 | `src/agent/panel.rs` — `compress_history()` | Write full history to disk before archiving to `archived_messages` |
| 4 | `src/agent/mod.rs` + `src/agent/agentic_loop.rs` | Add `search_session_history` tool (Phase 2, optional) |
| 5 | `src/config/mod.rs` | Add `observation_mask_threshold_chars: usize` config key |
| 6 | **Optional cleanup** | Remove `pending_resubmit_after_janitor`, `janitor_saved_pasted_blocks/file_blocks/image_blocks`, `Action::AgentSubmitPending` now that auto-trigger is off by default. Keep `compress_history()` and `SPC a j` intact. |
| 7 | ADR README | Add ADR 0123 entry |

### Step 2 detail — observation masking in API payload

Find where `self.messages` is converted to the API JSON (likely in
`src/agent/agentic_loop.rs` or `src/agent/panel.rs` around the `send_messages` call).
Add a masking pass over the slice before serialisation:

```rust
const MASK_THRESHOLD_CHARS: usize = 2000; // or from config
let masked: Vec<_> = messages
    .iter()
    .enumerate()
    .map(|(i, msg)| {
        let is_last = i == messages.len() - 1;
        if !is_last
            && matches!(msg.role, Role::Assistant)
            && msg.content.len() > MASK_THRESHOLD_CHARS
        {
            let tok_est = msg.content.len() / 4;
            ChatMessage {
                role: msg.role.clone(),
                content: format!(
                    "[assistant output: ~{tok_est} tokens — \
                     truncated for re-send; call the relevant tool again if needed]"
                ),
                images: vec![],
            }
        } else {
            msg.clone()
        }
    })
    .collect();
```

### Step 3 detail — disk persistence path

```rust
fn history_file_path(session_start: std::time::SystemTime) -> Option<PathBuf> {
    let ts = session_start
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let base = /* XDG_DATA_HOME or ~/.local/share */;
    Some(base.join("forgiven").join("history").join(format!("{ts}.jsonl")))
}
```

Write in `compress_history()` before the `std::mem::take`:

```rust
if let Some(path) = history_file_path(self.session_start) {
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&path)
        .ok();
    if let Some(ref mut f) = out {
        use std::io::Write as _;
        for msg in &self.messages {
            let line = serde_json::json!({
                "role": msg.role.as_str(),
                "content": msg.content,
                "ts": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
            });
            let _ = writeln!(f, "{}", line);
        }
    }
}
```

`session_start` should be added to `AgentPanel` — set in `new_conversation()` and at
first submit if not yet set.

---

## What Stays

- `compress_history()` and `SPC a j` manual keybind — unchanged
- `AgentStatus::Compressing` / `JanitorDone` — unchanged
- `janitor_compressing` flag — unchanged
- 90% in-chat warning (`context_near_limit_warned`) — implemented 2026-04-08
- `pending_janitor` flag — kept for users who opt back in via config

## What Can Be Removed (optional cleanup, Step 6)

These were added solely to support the auto-trigger deferred flow:

- `pending_resubmit_after_janitor: bool` (mod.rs)
- `janitor_saved_pasted_blocks`, `janitor_saved_file_blocks`, `janitor_saved_image_blocks` (mod.rs + panel.rs)
- `Action::AgentSubmitPending` (keymap/mod.rs, editor/actions.rs)
- The `pending_resubmit_after_janitor` tick-loop block in `editor/mod.rs`
- The `pending_janitor && self.has_pending_content()` block at the top of `submit()`

Only do Step 6 if the auto-trigger is confirmed off and no user reports need it.

---

## Consequences

**Positive**
- Eliminates the intrinsic timing and state-machine bugs of the auto-trigger.
- Observation masking is JetBrains NeurIPS 2025 validated; lower complexity than
  summarisation with comparable or better task performance.
- Disk persistence turns "did the compression lose that?" from an unanswerable question
  into an on-demand lookup.
- Total implementation complexity is lower than the current auto-janitor state machine.

**Negative / trade-offs**
- Observation masking loses verbatim prior tool results from in-context re-send. The
  model must re-call the tool if it needs the exact output again. For forgiven's common
  patterns (read_file, search_files) this is low-cost.
- Without an auto-trigger, users with very long sessions who ignore the 90% warning will
  eventually hit a hard API limit error. This is an explicit, recoverable error — better
  than silent quality degradation.
- The search_session_history tool (Phase 2) adds a new code path to maintain.

---

## Related ADRs

| ADR | Relation |
|---|---|
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Original audit |
| [0092](0092-persistent-session-metrics.md) | Persistent metrics — `sessions.jsonl` infrastructure reused for history files |
| [0097](0097-spec-framework-auto-clear.md) | specKit auto-clear on phase boundaries — reduces session length naturally |
| [0101](0101-auto-janitor-rolling-summary.md) | Auto-Janitor this ADR partially supersedes |
| [0117](0117-janitor-fixes.md) | Bug fixes this ADR's cleanup step will remove |
| [0120](0120-janitor-distinct-streaming-ux.md) | UX this ADR's cleanup step will simplify |
| [0121](0121-janitor-deferred-trigger-and-context-grounding.md) | Deferred-trigger state machine this ADR's cleanup step will remove |
