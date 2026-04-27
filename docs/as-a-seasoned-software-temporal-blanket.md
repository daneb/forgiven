# Forgiven — Technical Debt Review (v0.9.2-alpha.4)

*Reviewer's stance: 20 years, Rust + distributed systems. Direct where it matters, no padding. This is a strong codebase — the kind built by someone who reads, thinks, then writes. The criticisms below are about the next 12 months, not the last 12.*

---

## Context

You asked for an honest tech-debt review. Forgiven is ~32K LOC of Rust, 121 source files, 142 tests, 144 ADRs, and version `0.9.2-alpha.4`. The architecture in [CLAUDE.md](CLAUDE.md) is unusually well-described — most projects this size don't even have one. `unsafe_code = "forbid"`, clippy at `-D warnings`, `cargo-deny`, `cargo-audit`, all in CI. That foundation is rare.

That said, the codebase is starting to show the friction of fast iteration without periodic structural pauses. The biggest risks are not bugs — they're *velocity decay*: each new feature is becoming more expensive than the last because the seams haven't been re-cut to match where the product actually went. This document calls those seams out, ranks them, and proposes a sequencing.

---

## What's working (so we don't accidentally undo it)

- **Discipline of decisions.** 144 ADRs is a lot, but they exist. Most teams don't write the second one.
- **Drop impls are honest.** [src/lsp/mod.rs:779](src/lsp/mod.rs:779) kills the LSP child + process group on shutdown. MCP transport tasks are owned by handles. No dangling children.
- **`unsafe_code = "forbid"` enforced** — zero `#[allow(unsafe_code)]` overrides found.
- **Path traversal check is present** in [src/agent/tools.rs:360](src/agent/tools.rs:360) (`safe_path`) — and it's actually unit-tested.
- **No mutex held across `.await`.** Verified — concurrency hygiene is good.
- **ADR 0138 render decomposition** (the 631→240 line split) shows you *can* refactor when you commit to it. That muscle exists.

Hold on to all of that.

---

## Critical (address in the next sprint)

### C1. The `Editor` god-struct has metastasized — 60+ fields, ~10 in-flight `oneshot::Receiver`s
**Evidence:** [src/editor/mod.rs:52-330](src/editor/mod.rs:52). Receivers: `pending_completion`, `copilot_auth_rx`, `search_rx`, `insights_narrative_rx`, `mcp_rx`, `pending_goto_definition`, `pending_references`, `pending_symbols`, `pending_hover`, `pending_rename`. Plus `watcher_rx` (mpsc). Plus 5+ sidecar fields. Plus per-mode buffers (`rename_buffer`, `new_folder_buffer`, `lsp_rename_buffer`, …).

**Why it matters:** Every new async feature requires (a) a new field on `Editor`, (b) hand-written poll-and-clear in `event_loop.rs`, (c) a new mode + popup + buffer field. The 50ms loop currently does ~10 `.try_recv()`s per tick by hand. This is O(features) work per feature added, and it's why each new LSP/AI capability now takes longer than the previous one.

**Direction:** Introduce a single `RequestDispatcher { in_flight: HashMap<RequestId, oneshot::Receiver<Value>> }` that the event loop polls in one pass, with typed callbacks per request type. Cluster fields into sub-structs by concern: `LspState`, `SearchState`, `ExplorerPopupState`, `SidecarState`. Target: Editor drops from 60 fields to ~12 sub-struct handles. Don't try to do this in one PR — do it module by module behind no-op refactors.

### C2. `agent/panel.rs` is 2,447 lines and is now where every concern lives
**Evidence:** Largest single file in the project. Holds Copilot OAuth, multi-provider HTTP, streaming SSE handling, message history, token accounting, janitor logic, MCP tool routing, and rendering glue.

**Why it matters:** `panel.rs` has accumulated five subsystems and zero direct unit tests. Every fix to one concern (e.g. ADR 0117 janitor fixes) risks any of the other four. The fact that the janitor needed 5 ADRs (0101, 0117, 0120, 0121, 0123) over three weeks is the smoking gun — a focused module with tests would have caught those interactions in hours, not sprints.

**Direction:** Carve into:
- `agent/auth/` — Copilot device flow + key resolution
- `agent/transport/` — provider-keyed HTTP client (one impl behind a trait, see C3)
- `agent/session.rs` — message history, token accounting (this exists at 759 LOC; pull more in)
- `agent/panel.rs` — *only* the UI panel state and key handling
- Janitor and observation-masking go into `agent/context_management/`

### C3. No provider trait — every new LLM is a new match arm in three places
**Evidence:** [src/agent/provider.rs](src/agent/provider.rs) is a `ProviderKind` enum; per-provider endpoint URLs, auth headers, model-list fetches, and timeouts are sprinkled across `panel.rs` and `models.rs`. Adding OpenRouter/Gemini meant editing several locations in lockstep — the kind of change where one of them quietly drifts.

**Why it matters:** The product's moat is *agentic UX*, not *which provider you're talking to*. The provider layer should be the most boring, most uniform part of the codebase. Right now it's the most fan-out-prone.

**Direction:**
```rust
trait ChatProvider {
    fn endpoint(&self, model: &str) -> Url;
    fn auth_header(&self) -> Option<HeaderValue>;
    fn build_request(&self, msgs: &[Message], tools: &[Tool]) -> serde_json::Value;
    async fn stream(&self, req: Request) -> Result<impl Stream<Item = Delta>>;
}
```
Implementations: `Copilot`, `Anthropic`, `OpenAI`, `Gemini`, `OpenRouter`, `Ollama`. Test each one with a recorded fixture (no network).

### C4. Zero coverage on LSP transport, MCP transport, UI rendering, and config
**Evidence:** 142 tests across 32K LOC = ~0.4%. `lsp/` (1,121 LOC), `mcp/` (886), `ui/` (~3K), `config/` (799) all sit at zero. Hot files `agent/panel.rs`, `editor/actions.rs` also at zero.

**Why it matters:** These are exactly the layers that touch external systems (language servers, MCP servers, terminal escape sequences) and external schemas (config TOML). They are the most likely to break on Rust-edition bumps, ratatui upgrades, or LSP-spec changes — and you'd find out from a user report, not a CI failure. For a tool that *edits user code*, the asymmetry is wrong.

**Direction:** Pick the three highest-ROI tests this month:
1. LSP request/response round-trip with a stubbed `lsp-server` channel pair.
2. MCP stdio handshake + tool call against `cargo run --example` style harness.
3. Config round-trip: load every example from `docs/`, assert no panics, no warnings.

You don't need 80% coverage — you need a tripwire on the parts that talk to the outside world.

---

## High (address this quarter)

### H1. MCP SSE endpoint accepts arbitrary absolute URLs — open-redirect-style trust gap
**Evidence:** [src/mcp/mod.rs:186-190](src/mcp/mod.rs:186):
```rust
let post_url = if endpoint_path.starts_with("http") {
    endpoint_path
} else {
    format!("{}{}", base_url.trim_end_matches('/'), endpoint_path)
};
```
A compromised or malicious MCP server can send any URL on `endpoint`, and your client will POST tool-call payloads (potentially containing source code or secrets) to it.

**Direction:** Validate `post_url`'s origin matches `base_url`'s origin. One-line fix with a `url::Url` parse + origin compare. Add a test that asserts a cross-origin endpoint is rejected.

### H2. Blocking `std::fs::read_dir` inside `async fn execute_tool`
**Evidence:** [src/agent/tools.rs:372](src/agent/tools.rs:372) is `pub async fn`, [src/agent/tools.rs:618](src/agent/tools.rs:618) calls `std::fs::read_dir`. Same pattern around line 933.

**Why it matters:** On a slow filesystem (NFS, network drive, or just a huge directory), this stalls the entire tokio runtime — including the render loop. In a TUI, that shows up as the screen freezing while the agent "thinks." Subtle and very hard to debug from a user report.

**Direction:** Switch to `tokio::fs::read_dir` or wrap in `tokio::task::spawn_blocking`. Two-line change.

### H3. ADR sprawl is signaling design churn, not just diligence
**Evidence:** Janitor cluster: 0101, 0117, 0120, 0121, 0123. Context-management cluster: 0077, 0081, 0099, 0100, 0123, 0130, 0132. Token-efficiency cluster: 0087, 0088, 0093, 0102, 0119, 0126.

**Why it matters:** When one feature has five ADRs in three weeks, the ADR isn't capturing a *decision* anymore — it's capturing a *bug fix that wishes it were a decision*. That dilutes the entire ADR archive. New contributors won't know which ADRs are load-bearing and which are historical detritus.

**Direction:**
- Add explicit `Status: Superseded by 0123` headers to the obsolete janitor ADRs.
- Promote multi-ADR features into a single living spec under `docs/specs/`. ADRs become *decisions*; specs describe *current behavior*.
- Drop the auto-numbered ADR habit for bug-fix postmortems — those go in commit messages or `docs/incidents/`.

### H4. Silent `let _ = send(…)` on LSP notification channels
**Evidence:** [src/lsp/mod.rs:354,439,442,450,471,476,487](src/lsp/mod.rs:354). Seven send-and-pray calls.

**Why it matters:** When an LSP notification is dropped (channel closed), you lose diagnostics. The user sees stale red squiggles or none at all and doesn't know why. This is the kind of thing that erodes trust in the editor over months without anyone filing a bug.

**Direction:** Replace each `let _ = ` with `if let Err(e) = … { tracing::warn!(?e, "lsp notification dropped"); }`. Mechanical change. Already integrates with the in-app diagnostics overlay.

### H5. Caches without explicit eviction policies
**Evidence:** `HighlightCache`, `MarkdownCache`, `FoldCache`, `StickyScrollCache`, `ts_cache: HashMap<usize, TsSnapshot>` — all live as long as their buffer.

**Why it matters:** Today, fine — buffers are user-controlled, count is small. But the moment you add background pre-parsing (which you'll want for fuzzy "jump to symbol" across the project), this assumption breaks and you get a slow leak that's invisible in dev and obvious in 8-hour sessions.

**Direction:** Document the invalidation contract for each cache (already partly done in `state.rs`). Add a memory-budget config knob (`max_cached_buffers`) and an LRU eviction at >N. This is the kind of change that's free now and impossible later.

---

## Medium

### M1. `editor/input.rs` (1,168 lines) blends mode dispatch, motion parsing, and command execution
Pull `:` command parsing into `editor/command_parser.rs`. Vim motion parsing into `editor/motion.rs`. The dispatch table stays in `input.rs`.

### M2. `tracing` is well-used (179 sites) but unstructured
You log strings. For a tool whose value depends on understanding *cost* (tokens, latency, cache hits), you want structured fields:
```rust
tracing::info!(provider = %p, model = %m, tokens_in = pi, tokens_out = po, "round complete");
```
Then `cat sessions.jsonl | jq` becomes a real analysis tool. ~1 hour of work. Massive long-term leverage.

### M3. Sparse `///` doc comments on public API
[src/editor/mod.rs](src/editor/mod.rs) has narrative comments above field groups but few `///` on public methods. For a project you want others contributing to, this is the cheapest onboarding investment.

### M4. Config has accumulated escape-hatch flags
`auto_compress_tool_results`, `janitor_threshold_tokens`, `observation_mask_threshold_chars`, `expand_threshold_chars`, `default_copilot_model` (deprecated). Some of these were added during janitor iteration and aren't truly user-facing — they're "things we couldn't decide yet." Move those under `[experimental]` and remove the ones that ADR 0123 supersedes.

### M5. Version `0.9.2-alpha.4` undersells the maturity
Multi-provider LLMs, MCP support, prompt caching, observation masking, full Vim modal editing, Tree-sitter, LSP — this isn't alpha. Consider:
- Cut a `1.0-beta.1` once C1–C4 are landed.
- Reserve `alpha` for genuinely experimental modules (Companion sidecar, intent translator, Glimpse graphics).

---

## Low

- **L1.** `tree-sitter::query.rs` has 6 `.unwrap()` calls on region results — defensive `.ok_or_else()` would prevent rare panics on malformed parses.
- **L2.** `render.rs` (838 LOC) and `ui/` could share a `Widget` trait, but ADR 0138 already addressed the worst of it. Defer.
- **L3.** Config files at `~/.config/forgiven/config.toml` not enforced to `0o600` — low risk on single-user systems but worth a one-time chmod on creation.
- **L4.** `let _ = std::fs::create_dir_all(parent)` at [src/main.rs:108](src/main.rs:108) — log on failure so users can tell why logging silently went to stderr.

---

## Vision: where this product can go

You're not building a Vim clone. You're building **the first editor where the AI is a peer process, not a sidebar**. That framing changes the priority list:

1. **The Editor god-struct (C1) is the single biggest blocker to where you're going.** Every future capability — multi-agent, project-scoped indices, background analyzers, MCP tool ecosystems — adds an in-flight async operation. The current pattern caps you at ~20 concurrent operations before the event loop becomes painful to reason about. Fix this and the next year of features become 30% cheaper.

2. **The Companion sidecar (Phase 3 / Nexus) is the right bet.** TUI for action, GUI for ambient context. Most editors got this wrong by trying to do both in one window. Don't waste the architectural separation by re-merging concerns later — keep the IPC schema (`NexusEvent`) frozen and versioned now, before you have N consumers.

3. **Observation masking (ADR 0123) over rolling-summary was the right call.** The JetBrains finding will hold up. But the next step is *retrieval*, not compression — `search_session_history` should be a first-class tool, not a "Phase 2" item. That's the moat: an agent that remembers what you discussed yesterday, not just what's in the current 200K window.

4. **The MCP bet is correct, and undertested.** MCP is the USB of agent tools. Every IDE will support it within 18 months. Your code transport already works — what's missing is the *test suite that proves it works*. Ship that and you can integrate with any tool ecosystem without fear.

5. **The token observability problem (memory: open-file injection, observation masking) is the right one to obsess about.** Cost-per-task is the metric that matters. Make it visible in the UI, persist it in `sessions.jsonl` with structured fields (M2), and you'll be the only editor that can answer "did your last refactor cost $0.30 or $3.00?" — which is a question every paying user will eventually ask.

6. **What I'd be careful of:** the temptation to add a "second AI feature" before fixing C1. Multi-cursor, integrated terminal, debugging UI — all tempting, all will accelerate the god-struct rot. Pay the structural debt down first; the features compound after.

---

## Recommended sequencing

| Order | Item | Effort | Unlocks |
|---|---|---|---|
| 1 | H1 MCP origin check | 1 day | Removes a real security concern |
| 2 | H2 blocking `read_dir` | 1 day | Removes UI freezes |
| 3 | C3 Provider trait | 3–5 days | Cuts panel.rs by ~600 LOC and unblocks C2 |
| 4 | C2 panel.rs carve-out | 1–2 weeks | Makes janitor/context work testable |
| 5 | C1 RequestDispatcher | 1–2 weeks | Compounds for every future feature |
| 6 | C4 transport tests | 1 week | Catches LSP/MCP regressions before users do |
| 7 | H3 ADR consolidation | 2 days | Restores decision-archive integrity |
| 8 | M2 structured logging | 1 day | Unlocks cost analytics |

Don't try to do everything. The first three buy you the next quarter; the next three buy you the year.

---

## Verification

This is an analysis, not an implementation. The "verification" is reading the file, disagreeing with parts of it, and choosing the 2–3 items you actually want me to implement. Specifically I'd suggest:

- Read the Critical section against [src/editor/mod.rs](src/editor/mod.rs) and [src/agent/panel.rs](src/agent/panel.rs) and confirm the field/line counts match what you remember.
- Read H1 ([src/mcp/mod.rs:186](src/mcp/mod.rs:186)) and decide whether the origin check is worth a same-day PR.
- Decide which item I should turn into an implementation plan next.
