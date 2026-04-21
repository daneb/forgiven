# ADR 0135 — Agent Panel Space-A Cleanup

**Status:** Implemented
**Date:** 2026-04-21

---

## Context

The `SPC a` keybinding tree had grown to 14 entries over time, added incrementally as
features shipped. The addition of the Insights Dashboard (ADR 0129) prompted a review of
whether all entries were still pulling their weight.

Two findings drove the cleanup:

1. **The `SPC a` which-key popup was too crowded to scan at a glance.** Fourteen entries
   across a single level made it hard to locate the most-used commands (toggle, new
   conversation, review, revert).

2. **Two commands were mis-placed.** `SPC a j` (janitor compress) and `SPC a t` (intent
   translator toggle) are contextual, in-session operations — they are reached for *while
   the agent panel is open and focused*, making them a better fit as slash commands typed
   directly in the input box than as leader-key shortcuts.

The Insights Dashboard does **not** make any agent panel functionality redundant. The
panel shows live per-session token state; the dashboard aggregates historical data across
sessions. They answer different questions and complement each other.

---

## Decision

### 1. Remove `SPC a j` — demote to `/compress`

`SPC a j` triggered `Action::AgentJanitorCompress`. The manual janitor is an escape hatch
used *while focused in the panel*, so a slash command is the natural home.

`/compress` is now intercepted in the Enter key handler in `editor/input.rs` before
`panel.submit()` is called. It dispatches `Action::AgentJanitorCompress` and clears the
input, identical behaviour to the old keybinding.

### 2. Remove `SPC a t` — demote to `/translate`

`SPC a t` toggled `intent_translator_enabled` on the live panel struct — a per-session
debug toggle for an off-by-default feature. It was the only way to toggle the feature
in-session, but it did not persist to config and was not worth a top-level leader slot.

`/translate` is intercepted in the same way as `/compress`, dispatching
`Action::AgentIntentTranslatorToggle`.

### 3. Move `SPC a c/C/k` → `SPC a x c/C/k`

The three codified context file openers (`constitution`, `specialist`, `knowledge`) were
taking three top-level `SPC a` slots for what are essentially file-opening shortcuts. They
are grouped under a new `x` sub-node, freeing three slots while keeping them discoverable
via which-key as `SPC a x`.

Note: `CodifiedContextOpenConstitution` still creates `.forgiven/constitution.md` as a
stub when the file does not exist — this bootstrapping side-effect is preserved.

### 4. Move `SPC a I` → `SPC d i`

The Insights Dashboard is the only *observational/historical* command in an otherwise
*operational* tree. It sits more naturally under `SPC d` (diagnostics) alongside
`SPC d d` (diagnostics overlay) and `SPC d l` (log file).

### 5. Slash command autocomplete always shows built-ins

`update_slash_menu` previously only populated the dropdown when a `spec_framework` was
loaded. Built-in action commands (`compress`, `translate`) are now merged into the menu
regardless of whether a framework is configured, so they are always discoverable by
typing `/` in the panel input.

---

## Result

| Before | After |
|---|---|
| `SPC a a` — toggle | `SPC a a` — toggle |
| `SPC a f` — focus | `SPC a f` — focus |
| `SPC a n` — new conversation | `SPC a n` — new conversation |
| `SPC a s` — save to memory | `SPC a s` — save to memory |
| `SPC a j` — compress history | `/compress` in panel input |
| `SPC a i` — inline assist | `SPC a i` — inline assist |
| `SPC a u` — revert session | `SPC a u` — revert session |
| `SPC a v` — investigate | `SPC a v` — investigate |
| `SPC a r` — review changes | `SPC a r` — review changes |
| `SPC a I` — insights dashboard | `SPC d i` — insights dashboard |
| `SPC a t` — intent translator | `/translate` in panel input |
| `SPC a c` — open constitution | `SPC a x c` — open constitution |
| `SPC a C` — open specialist | `SPC a x C` — open specialist |
| `SPC a k` — open knowledge | `SPC a x k` — open knowledge |

14 entries → 9 entries in `SPC a`.

---

## Implementation

| File | Change |
|---|---|
| `src/keymap/mod.rs` | Remove `SPC a j`, `SPC a t`, `SPC a I`; add `SPC a x` sub-tree for c/C/k; add `SPC d i` |
| `src/agent/panel.rs` | `BUILTIN_SLASH_COMMANDS` const; `update_slash_menu` always includes builtins |
| `src/editor/input.rs` | Intercept `/compress` and `/translate` on Enter before `panel.submit()` |

---

## Trade-offs accepted

**`SPC a j` muscle memory breaks.** Users who relied on the keybinding must retrain to
`/compress`. The slash command is more discoverable (shown in autocomplete on `/`) and
closer to where the action is taken (inside the panel with context visible).

**`SPC a I` relocation.** Users who opened insights from the agent sub-menu must learn
`SPC d i`. The new location is semantically correct and consistent with the diagnostics
namespace.

**`/translate` not persistent.** The intent translator toggle still does not persist to
config — it remains a session-only flag. A `[agent] intent_translator.enabled = true`
config option would be the natural next step, but that is out of scope here.
