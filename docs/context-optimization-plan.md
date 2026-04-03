# Context Optimization Plan

Derived from `context-optimization-speckit.md`. Maps the spec into actionable phases for the forgiven codebase.

---

## Phase 0 — Foundation (Already Complete)

Work done as of ADR 0092 (2026-04-03). Baseline established before the spec work begins.

| Task | Status |
|---|---|
| Cap open-file injection to 150 lines (`MAX_CTX_LINES`) | Done |
| Move model fetch before budget computation in `submit()` | Done |
| XDG-aware persistent log at `~/.local/share/forgiven/forgiven.log` | Done |
| `session_rounds` counter + avg tokens/invocation in `SPC d` overlay | Done |

---

## Phase 1 — Token Awareness UI

**Goal:** Make token consumption visible and categorized so improvement from later phases can be measured.

1. Integrate `tiktoken-rs` to count tokens for each context segment before the API call.
2. Extend the `SPC d` overlay (or add a new `SPC t` panel) to show a cost breakdown: System Prompt | Open File | Spec Injection | Chat History | New Message.
3. Add a footer "fuel gauge" — a compact percentage bar showing where the context budget is going.

This phase is read-only to behaviour — no prompt changes, just instrumentation.

---

## Phase 2 — Spec Slicer

**Goal:** Replace raw file dumps with surgical context injection.

1. Add a `SpecParser` module in `src/agent/` that parses `tasks.md` and `spec.md` into a section tree.
2. Implement `get_active_context(task: &str) -> String` — given the active task, return only its parent spec section + referenced schema entries.
3. Implement the Archive pattern: completed `[x]` tasks are stripped from the prompt payload automatically (excluded from `ContextManager` output).
4. Wire `get_active_context()` into `submit()` in `panel.rs`, replacing any raw file injection.

This phase directly targets the "SpecKit Tax" — the primary remaining token source after Phase 0.

---

## Phase 3 — Auto-Janitor (Rolling Summary)

**Goal:** Prevent chat history from growing unbounded.

1. Implement `MemoryJanitor` struct as a background `tokio` task that monitors cumulative prompt tokens.
2. Add a manual "Summarize & Clear" keybind (e.g., `SPC a s`) as the first trigger — validate summary quality before automating.
3. Automate trigger: when session prompt tokens exceed a configurable threshold (default: 10K), spawn a cheap summarizer call (e.g., Haiku) with the pruning prompt from the spec (section 4.1).
4. Implement ephemeral vs. persistent message tagging: architectural decisions auto-append to `plan.md`; throwaway turns are discarded on reset.

---

## Phase 4 — Spec Navigator UI (Optional / Future)

**Goal:** Surface spec.md as an interactive tree, not a text file.

1. Add a sidebar panel rendering spec sections as a navigable tree (ratatui `Tree` widget or custom).
2. Selecting a node triggers `get_active_context()` and updates the active task context.
3. Visual indicator showing which spec sections are currently injected.

This phase is UI polish and has no token-reduction effect beyond Phase 2.

---

## Complexity Table

| Phase | Task | Complexity | Risk |
|---|---|---|---|
| **1** | Integrate `tiktoken-rs`, count per-segment | Low | Low — additive only |
| **1** | Cost breakdown in `SPC d` overlay | Low | Low — ratatui widget work |
| **1** | Footer fuel gauge bar | Low | Low — purely visual |
| **2** | `SpecParser` — parse spec.md/tasks.md into section tree | Medium | Medium — regex/markdown parsing edge cases |
| **2** | `get_active_context()` — dependency resolution between spec sections | High | High — correctness hard to validate automatically |
| **2** | Archive pattern for completed tasks | Low | Low — filter pass on input |
| **2** | Wire into `submit()` in `panel.rs` | Medium | Medium — regression risk to existing chat flow |
| **3** | `MemoryJanitor` struct + token threshold monitoring | Medium | Low — tokio task, clear boundary |
| **3** | Manual "Summarize & Clear" keybind | Low | Low — calls existing new_conversation() + summarize prompt |
| **3** | Automated summarizer trigger + Haiku dispatch | High | High — async coordination, model cost, summary quality |
| **3** | Ephemeral/persistent message tagging + auto-write to plan.md | Medium | Medium — file mutation from agent context is tricky |
| **4** | Spec Navigator sidebar (ratatui tree) | High | Low — UI risk only, no token logic |
| **4** | Active context indicator | Medium | Low |

---

## Recommended Sequencing

Phase 1 first (measure before optimizing) → Phase 2 (biggest remaining token win) → Phase 3 (history management) → Phase 4 only if spec navigation becomes a workflow bottleneck.
