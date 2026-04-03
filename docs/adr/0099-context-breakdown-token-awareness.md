# ADR 0099 — Context Breakdown: Per-Segment Token Awareness (Phase 1)

**Date:** 2026-04-03
**Status:** Accepted

---

## Context

ADR 0087 audited context bloat and introduced `total_session_prompt_tokens` and
the `SPC d` Agent Session section. ADR 0096 added `session_rounds` and a
per-invocation average. Both surface *how much* the session has consumed in
aggregate, but neither answers the question: **where is the budget going within
a single round?**

Without segment-level visibility, the engineer cannot distinguish between:

- A system-prompt blowout (large file injected, 12K tokens of rules)
- History growth (10 rounds of dense tool results)
- A genuinely large user message

`docs/context-optimization-speckit.md` defines a Context Optimisation roadmap.
Phase 1 ("Token Awareness System") calls for two deliverables:

1. **Cost breakdown** — per-segment token counts visible in `SPC d`
2. **Fuel gauge** — compact status-bar indicator of overall window pressure

A third pre-condition was adding a proper token counter. The existing codebase
uses the `len / 4` approximation everywhere; using the actual GPT-4 tokeniser
(`cl100k_base`) makes all downstream Phase 2 / Phase 3 optimisation decisions
trustworthy.

---

## Decision

### 1. `tiktoken-rs` crate (cl100k_base tokeniser)

Add `tiktoken-rs = "0.5"` to `Cargo.toml`. A new module
`src/agent/token_count.rs` exposes a single function:

```rust
pub fn count(text: &str) -> u32
```

The `CoreBPE` is initialised once via `std::sync::OnceLock` (first call, reused
for the process lifetime). If initialisation fails for any reason, the function
falls back to `text.len() / 4` so token counts always succeed.

`cl100k_base` was chosen because it is the tokeniser used by all current Copilot
models (GPT-4 family and Claude via the Copilot gateway).

### 2. `ContextBreakdown` struct on `AgentPanel`

```rust
pub struct ContextBreakdown {
    pub sys_rules_t: u32,  // system prompt rules + preamble (without open file)
    pub ctx_file_t: u32,   // open-file snippet injected into system prompt
    pub history_t: u32,    // chat history sent this round (post-truncation)
    pub user_msg_t: u32,   // new user message
    pub ctx_window: u32,   // model context window size
}
```

`total()` sums the four input segments. `used_pct()` expresses `total()` as a
percentage of `ctx_window`.

`AgentPanel` gains a `pub last_breakdown: Option<ContextBreakdown>` field,
populated at the end of each `submit()` call and reset to `None` only on
process restart (it is intentionally *not* reset by `new_conversation()` so the
last round's breakdown remains visible in `SPC d` after a reset).

### 3. Breakdown computation in `submit()`

After `send_messages` is fully assembled (system + truncated history + user
message), `submit()` computes each segment using `token_count::count`:

| Segment | Source |
|---------|--------|
| `ctx_file_t` | `context_snippet` (the capped open-file string) |
| `sys_rules_t` | `token_count::count(&system) − ctx_file_t` |
| `history_t` | Sum over `send_messages[1..n-1]` (content fields) |
| `user_msg_t` | `token_count::count(&user_text)` |

`sys_rules_t` is computed by subtraction rather than slicing `system` to avoid
byte-index fragility; the minor over-count from the "Currently open file:"
wrapper text (~8 tokens) is acceptable for a display value.

### 4. `SPC d` Context Breakdown section

`DiagnosticsData` gains `pub agent_ctx_breakdown: Option<ContextBreakdown>`.
`render_diagnostics_overlay()` renders a new section immediately after Agent
Session:

```
 Context Breakdown
  sys rules    2,400t  ████░░░░   24%
  open file      600t  █░░░░░░░    6%
  history      3,200t  █████░░░   32%
  user msg       180t  ░░░░░░░░    2%
  ─────────────────────────────────────
  total        6,380t  of 32,000t  (64%)
```

Each row shows token count, an 8-block ASCII bar, and percentage of the context
window. Bar colour follows the same green / yellow / red thresholds as the
existing Agent Session gauge (40% / 80%).

Popup width widened from 60 to 64 columns to accommodate the breakdown rows.

### 5. Status-bar fuel gauge

`render_status_line` gains an `agent_fuel: Option<u32>` parameter (the
`used_pct()` value computed by the caller from `agent_panel.last_breakdown`).

When `agent_fuel` is `Some` and no command/search prompt is active, a compact
gauge is appended to the status bar:

```
AGENT  src/main.rs  [████░░ 38%]
```

A 6-block bar is used (narrower than the overlay's 8-block bar) to minimise
status-bar width impact. The same colour thresholds apply.

---

## Implementation

### New file: `src/agent/token_count.rs`

```rust
use std::sync::OnceLock;

static BPE: OnceLock<Option<tiktoken_rs::CoreBPE>> = OnceLock::new();

fn bpe() -> Option<&'static tiktoken_rs::CoreBPE> {
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
}

pub fn count(text: &str) -> u32 {
    match bpe() {
        Some(bpe) => bpe.encode_with_special_tokens(text).len() as u32,
        None => (text.len() / 4) as u32,
    }
}
```

### `src/agent/mod.rs`

- `pub mod token_count;` added to module declarations.
- `ContextBreakdown` struct added with `total()` and `used_pct()` methods.
- `pub last_breakdown: Option<ContextBreakdown>` added to `AgentPanel`; initialised `None` in `new()`.

### `src/agent/panel.rs`

Breakdown computation inserted between `send_messages.push(user_msg)` and
`self.messages.push(ChatMessage { ... })` in `submit()`. At this point
`user_text` (not yet moved), `system`, `context_snippet`, and `context_limit`
are all still in scope.

### `src/ui/mod.rs`

- `agent_ctx_breakdown: Option<crate::agent::ContextBreakdown>` added to `DiagnosticsData`.
- `render_status_line` call extended with `agent_fuel` computed from
  `agent_panel.and_then(|p| p.last_breakdown).map(|b| b.used_pct())`.

### `src/editor/mod.rs`

`agent_ctx_breakdown: self.agent_panel.last_breakdown` added to the
`DiagnosticsData` struct literal in the `Mode::Diagnostics` render path.

### `src/ui/popups.rs`

Context Breakdown section rendered in `render_diagnostics_overlay()` when
`data.agent_ctx_breakdown.is_some()`. Popup width increased from 60 to 64.

### `src/ui/status.rs`

`agent_fuel: Option<u32>` added as final parameter of `render_status_line`.
Fuel gauge spans appended when `agent_fuel.is_some()` and no command/search
prompt is overriding the status bar.

---

## Consequences

**Positive**
- Engineers can immediately see which segment is the dominant cost driver after
  any submit, without grepping logs.
- The fuel gauge makes context pressure visible at a glance in every mode, not
  only inside `SPC d`.
- Using the actual `cl100k_base` tokeniser (rather than `len / 4`) makes all
  percentage figures trustworthy for Phase 2 (Spec Slicer) optimisation
  decisions.
- `token_count::count` is available to all future modules as a shared utility,
  so Phase 2's `SpecParser` and Phase 3's `MemoryJanitor` get accurate counts
  without duplicating logic.

**Negative / trade-offs**
- `tiktoken-rs` adds ~5 new transitive crates (`bstr`, `fancy-regex`,
  `rustc-hash`, `base64 v0.21`, `bit-vec`, `bit-set`). Build time increases
  marginally; the BPE vocabulary data is compiled into the binary (adds ~1–2 MB
  to the release binary).
- The OnceLock initialisation (~30 ms on first call) is absorbed into the first
  `submit()` latency; invisible to the user because the API call itself takes
  hundreds of milliseconds.
- `sys_rules_t` over-counts by ~8 tokens (the "Currently open file:" wrapper
  text is included in `system_t`). This is cosmetic: the display is labelled
  "sys rules" and engineers understand it as an approximation.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Keep `len / 4` throughout | Inaccurate for CJK text and dense code; percentages misleading for Phase 2 decisions |
| Derive `history_t` from `SubmitCtx.budget_for_history` | `budget_for_history` is the *allowed budget*, not actual tokens sent; would not reflect what was actually truncated |
| Right-align the fuel gauge in the status bar | Requires measuring all left spans and padding; adds complexity for marginal UX gain — left-append is sufficient |
| Add breakdown to the JSONL session log | Useful but separate concern; the JSONL log records API-confirmed token counts, not estimates |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Original context bloat audit — Phase 1 builds on the instrumentation started there |
| [0096](0096-session-rounds-and-avg-tokens-diagnostic.md) | Session rounds counter — Phase 1 adds per-segment detail below the per-round average |
| [0049](0049-diagnostics-overlay.md) | `SPC d` overlay — Context Breakdown section added here |
| [0040](0040-context-gauge.md) | Panel-title context gauge — fuel gauge is a companion indicator in the status bar |
| [0077](0077-agent-context-window-management.md) | History truncation logic — `history_t` reflects tokens after truncation |
| [0081](0081-importance-scored-history.md) | Importance-scored retention — `history_t` is the output of that algorithm |
