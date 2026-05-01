# Context Management — Living Spec

**Last updated:** 2026-05-01  
**Canonical ADR:** [ADR-0123](../adr/0123-context-management-v2-observation-masking-and-disk-persistence.md)

---

## Current behaviour

### 1. Manual janitor (`SPC a j`)

`compress_history()` in `src/agent/panel.rs` collapses `self.messages` into a
rolling summary via a separate LLM call, then archives the original messages to
`self.archived_messages`. The archived messages are never re-sent to the API;
only the summary is retained in the active window.

Status variants `AgentStatus::Compressing` and `AgentStatus::JanitorDone` drive
the UX during the LLM round.

**Auto-trigger is disabled by default** (`janitor_threshold_tokens = 0` in
`src/config/mod.rs`). Users who want the auto-trigger can set
`janitor_threshold_tokens = 50000` (or any positive value) in their config file.

### 2. 90% in-chat warning

When estimated session tokens reach 90% of the active model's context window,
the agent emits a one-line inline warning with the exact keybind (`SPC a j`)
before the next round. Controlled by `context_near_limit_warned: bool` on
`AgentPanel` to fire only once per session.

### 3. Observation masking (planned — ADR-0123 Phase 1)

Before the API payload is assembled in `agentic_loop.rs`, any assistant message
older than the most recent one whose character count exceeds
`observation_mask_threshold_chars` (config default: 2000) is replaced with a
stub:

```
[assistant output: ~N tokens — truncated for re-send; call the relevant tool again if needed]
```

`self.messages` (the display history visible to the user) is **never** modified —
only the outgoing slice is masked. User messages are never truncated.

Config key: `agent.observation_mask_threshold_chars` (`0` = disabled).

**Not yet implemented** — ADR-0123 step 2.

### 4. Disk persistence (planned — ADR-0123 Phase 1)

Before `compress_history()` archives messages, the full conversation is appended
to `~/.local/share/forgiven/history/<unix-ts>.jsonl` (one JSON line per message:
`role`, `content`, `ts`). This makes "what did the janitor throw away?" answerable
without disrupting the in-memory flow.

**Not yet implemented** — ADR-0123 step 3.

---

## Config knobs

| Key | Default | Effect |
|-----|---------|--------|
| `janitor_threshold_tokens` | `0` | Auto-trigger threshold; `0` = disabled |
| `agent.observation_mask_threshold_chars` | `2000` | Mask large assistant messages in API payloads; `0` = disabled |

---

## What was superseded

The auto-janitor accumulated three rounds of fixes (ADR-0117, -0120, -0121) that
addressed symptoms of an async LLM round inserted at unpredictable points in the
user's workflow (timing bugs, context amnesia, dropped pasted blocks). ADR-0123
disabled the auto-trigger by default rather than continuing that arms race.

| ADR | Status | Reason |
|-----|--------|--------|
| [0101](../adr/0101-auto-janitor-rolling-summary.md) | Partially superseded | Auto-trigger off; manual janitor + `compress_history()` retained |
| [0117](../adr/0117-janitor-fixes.md) | Superseded | Bug fixes for auto-trigger state machine no longer needed |
| [0120](../adr/0120-janitor-distinct-streaming-ux.md) | Superseded | Distinct UX was for auto-trigger flow |
| [0121](../adr/0121-janitor-deferred-trigger-and-context-grounding.md) | Superseded | Deferred-trigger state machine removed |

---

## Remaining cleanup (optional)

These symbols exist only to support the auto-trigger deferred flow and can be
removed once the auto-trigger is confirmed permanently off:

- `pending_resubmit_after_janitor: bool`
- `janitor_saved_pasted_blocks`, `janitor_saved_file_blocks`, `janitor_saved_image_blocks`
- `Action::AgentSubmitPending`
- The `pending_resubmit_after_janitor` tick-loop block in `editor/mod.rs`
- The `pending_janitor && self.has_pending_content()` block at the top of `submit()`
