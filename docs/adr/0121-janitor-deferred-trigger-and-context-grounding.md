# ADR 0121 — Auto-Janitor: Deferred Trigger and Context Grounding

**Date:** 2026-04-08
**Status:** Accepted

---

## Context

ADR 0101 introduced the Auto-Janitor and ADR 0117 fixed several bugs. However, two
dev-workflow breakages were observed in practice:

### 1. specKit timing problem

The janitor fired in the editor tick-loop immediately (next tick) after
`StreamEvent::Done`. When a specKit phase (or any agentic run) ends with a
question to the user — "Do you want to continue with A or B?" — the janitor
compressed history before the user could respond. The user's subsequent answer
("B") arrived in a freshly compressed session where:

- The model had no direct conversational memory of asking the question.
- The compression summary might not have preserved the exact question (the
  prompt said "discard completed throwaway tasks", which could include pending
  questions).

The model was then confused by a bare "B" with no context.

### 2. Post-compression context amnesia

After compression, the summary was stored as two `Role::System` messages:

```
[System] "── Context compressed by Auto-Janitor ──"
[System] "**Session summary (Auto-Janitor):** ..."
```

Models treat `Role::System` messages as instructions or metadata, not as
conversational memory they "authored." In practice the model would respond to
follow-up questions as if starting fresh — e.g., saying "there is no .gitignore"
even though the summary stated ".gitignore was added."

---

## Decision

Three coordinated changes, all targeting the same session:

### 1. Defer janitor to submit-time (not tick-loop-time)

**Old flow:**
```
model answers → Done → pending_janitor=true → next tick → janitor fires
→ user types answer → submits → model has no question context
```

**New flow:**
```
model answers → Done → pending_janitor=true (held)
→ user types answer → hits Enter → submit() checks pending_janitor
  → compress_history() fires (saves user's answer to janitor_saved_input)
  → compression round submitted and completes
  → Done handler: restores saved answer, sets pending_resubmit_after_janitor=true
  → tick-loop: Action::AgentSubmitPending → user's answer sent
  → model receives [summary with question context] + [user: "B"]
```

The janitor now always compresses **together with** the user's response, so the
compression context includes both sides of any pending question/answer pair.

**Implementation:**
- Removed the `pending_janitor` tick-loop block from `editor/mod.rs`.
- Added a `pending_janitor` check at the top of `panel.rs submit()`: if pending
  and input is non-empty, calls `compress_history()` inline, then falls through
  to send the compression prompt.
- Added `pending_resubmit_after_janitor: bool` to `AgentPanel`.
- Done handler (janitor branch): sets `pending_resubmit_after_janitor = true`
  when restored input is non-empty.
- Added `Action::AgentSubmitPending` (tick-loop trigger, no keybind) that submits
  the restored input with normal session settings.

**Edge cases:**
- If `pending_janitor=true` but input is empty, the flag stays set and fires on
  the next non-empty submit. This preserves the old "immediately compress" path
  for the rare case where the user submits an empty input.
- Manual `SPC a j` is unaffected — it calls `compress_history()` + `submit()`
  directly through `Action::AgentJanitorCompress`.
- `janitor_threshold_tokens = 0` (disabled) is unaffected.

### 2. Store summary as User/Assistant exchange

**Old:**
```rust
Role::System  "── Context compressed by Auto-Janitor ──"
Role::System  "**Session summary (Auto-Janitor):** {summary}"
```

**New:**
```rust
Role::System    "── Context compressed by Auto-Janitor ──"
Role::User      "Briefly recap what we accomplished."
Role::Assistant  {summary}
```

The System separator is kept for visual anchoring in the chat panel. The
User/Assistant pair grounds the model: it "remembers" the summary as a response
it gave, not as an external instruction. This fixes the amnesia where the model
ignored the summary and reverted to a fresh-context response.

### 3. Improved summarisation prompt

Added explicit instruction to preserve pending questions:

> IMPORTANT: also preserve verbatim any open questions you posed to the user,
> pending decisions, and the immediate next step — the user may be about to
> reply to these.

This is a defence-in-depth addition; Fix 1 (deferred trigger) is the primary
fix for the specKit scenario. Fix 3 improves summary quality even when the
janitor fires manually via `SPC a j`.

---

## Implementation

| File | Change |
|---|---|
| `src/agent/mod.rs` | Added `pending_resubmit_after_janitor: bool` to `AgentPanel` |
| `src/agent/panel.rs` | `submit()`: deferred janitor check; Done handler: User/Assistant summary + resubmit flag; `compress_history()`: improved prompt |
| `src/editor/mod.rs` | Replaced `pending_janitor` tick-loop block with `pending_resubmit_after_janitor` block |
| `src/keymap/mod.rs` | Added `Action::AgentSubmitPending` (no keybind) |
| `src/editor/actions.rs` | Implemented `Action::AgentSubmitPending` handler |

---

## Consequences

**Positive**
- specKit and any other workflow ending with a model question is no longer
  disrupted by janitor firing between the question and the answer.
- Post-compression amnesia ("there is no .gitignore") is eliminated because the
  model owns its summary as a conversation turn.
- Summaries are higher quality: pending questions and next steps are preserved
  even when `SPC a j` is triggered manually.
- The deferred trigger is simpler to reason about: compression always happens
  at a known, user-driven boundary (submit), never between turns.

**Negative / trade-offs**
- When the user submits a message that triggers deferred compression, there is a
  brief visible pause (the compression round) before their message is actually
  sent. The existing "Auto-Janitor" visual indicator in the panel title covers
  this, but it's a small UX change from the previous "invisible background
  compression."
- If the user never submits again after a threshold is crossed, the janitor
  never fires. This is acceptable — there is nothing to compress for future
  rounds if there are no future rounds.

---

## Related ADRs

| ADR | Relation |
|---|---|
| [0101](0101-auto-janitor-rolling-summary.md) | Original Auto-Janitor: compression mechanism, threshold, keybind |
| [0117](0117-janitor-fixes.md) | Prior bug fixes: archive preservation, input restore, status variants |
| [0120](0120-janitor-distinct-streaming-ux.md) | UX: distinct streaming header, pre-compression marker |
| [0097](0097-spec-framework-auto-clear.md) | specKit auto-clear on phase boundaries — this ADR fixes the orthogonal intra-phase timing issue |
