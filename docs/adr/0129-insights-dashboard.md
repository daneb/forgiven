# ADR 0129 — Insights Dashboard: Collaboration Analytics

**Date:** 2026-04-16  
**Status:** Accepted — All four phases implemented

---

## Context

After building Forgiven through 128 ADRs, there is a rich signal in the editor's own data
files — `forgiven.log`, `sessions.jsonl`, and `history/*.jsonl` — about how the
Human↔Agent collaboration is actually working. None of that data is surfaced to the user.

The reference implementation is the Claude Code Insights dashboard, which shows:
- Quantitative metrics: message counts, line changes, files touched, active days, msgs/day
- Work topic breakdown (feature/bug/doc/refactor by session)
- Tool usage statistics (what tools were called, how often)
- Language distribution across changed files
- Session type classification (single-task, multi-task, iterative refinement)
- Time-of-day activity heatmap
- Tool error breakdown by error type
- Qualitative narrative: "what worked", "what's hindering", "quick wins"

The goal is to build a Forgiven-native equivalent that reads local-only data, displays
inside the editor itself, and grows in depth as more session data accumulates.

---

## Available Data Sources

| Source | Path | Contents | Status |
|--------|------|----------|--------|
| Log file | `~/.local/share/forgiven/forgiven.log` | Session starts, LLM requests, model/provider, buffer saves, MCP events, warnings/errors | **Exists** — 4,142 lines across Apr 2–15 |
| Session metrics | `~/.local/share/forgiven/sessions.jsonl` | Per-session: model, prompt tokens, completion tokens, rounds, files_changed, ended_by | Written at `new_conversation()` and janitor completion |
| Conversation history | `~/.local/share/forgiven/history/<ts>.jsonl` | Per-message: role, content, images, timestamps | Written during agentic loop |

---

## Decision

Build the insights feature in four phases, proving value at each phase before proceeding.

### Phase 1 — Log parser (this ADR)

Mine `forgiven.log` to produce an `InsightSummary` with everything derivable from the
existing log without any code changes to the instrumentation path.

**`src/insights/` module:**
- `mod.rs` — re-exports, public API
- `log_parser.rs` — ANSI-stripping line parser, event extraction, aggregation

**`:insights` command** (`:` in normal mode) — pushes a formatted markdown report as an
Assistant message into the agent panel. No new TUI panel needed for Phase 1.

**Metrics derivable from log alone:**

| Metric | Log signal |
|--------|-----------|
| Session count | `"Starting forgiven"` in `forgiven` module |
| Active days | Unique YYYY-MM-DD prefixes across all timestamps |
| LLM request count | `"Sending completion request"` in `agentic_loop` |
| Chat-only rounds | `"tool calling disabled"` in `agentic_loop` |
| One-shot count | `"one_shot request sending"` in `editor::ai` |
| Model usage | `model="..."` key-value in request lines |
| Provider usage | `provider=...` key-value in request lines |
| Buffer saves | `"Saved buffer"` in `buffer::buffer` |
| Time-of-day | Hour extracted from RFC3339 timestamp |
| Warnings / errors | Log level field |

### Phase 2 — Data enrichment

Add three small JSONL records to capture what the log can't:

1. **`session_start`** record in `sessions.jsonl` — project root, model, provider, timestamp.
   Currently only `session_end` is written; without a matching start record, token efficiency
   ratios and session duration cannot be computed.

2. **Per-turn telemetry** — extend `history/<ts>.jsonl` entries with `tool_calls: [{name, success}]`
   and `char_count`. The messages are already written; this adds two fields per entry.

3. **Tool error events** — a `tool_error` record in `sessions.jsonl` when `tool_dispatch`
   returns an error variant. Currently lost after the log warning.

**Status: implemented.** `append_session_start_record` (`session.rs:63`), `append_round_tools` (`session.rs:84`), and `tool_error` JSONL writes (`tool_dispatch.rs:235`) are all live. Phase 4 now has structured data to work with.

### Phase 3 — Aggregator + TUI panel

Once Phase 2 data exists (now live), build a proper overlay panel triggered by `SPC a I`:

```
src/insights/
  aggregator.rs     — parse sessions.jsonl + history JSONL, join with log metrics
  panel.rs          — Ratatui rendering: tabs, bar charts, sparklines
```

**Five tabs:**
- **Summary** — total sessions, messages, active days, msgs/day, date range (mirrors the "At a Glance" header)
- **Activity** — time-of-day BarChart (4 bands), day-by-day activity sparkline
- **Models** — model/provider request counts, agentic vs chat-only split
- **Efficiency** — rounds/session histogram, token trend, files/session (from `sessions.jsonl`)
- **Errors** — tool error type breakdown (from Phase 2 records)

### Phase 4 — LLM-powered narrative

The qualitative "what worked / what's hindering / quick wins" layer:

A `:insights summarize` command that:
1. Reads the last N history JSONL files (configurable, default 20)
2. Builds a structured analysis prompt: user message patterns, tool call outcomes,
   round counts, error frequency, session type distribution
3. Sends through the active LLM (whatever model is currently selected)
4. Writes the narrative to `~/.local/share/forgiven/insights_narrative.md`
5. Displays in the Summary tab alongside the quantitative stats

This reuses the existing single-round agent infrastructure (same pattern as the
Investigation Subagent, ADR 0128) — no new LLM plumbing needed.

---

## Phase 1 Implementation

### ANSI stripping

`forgiven.log` is written with ANSI colour codes because the tracing subscriber uses a
TTY-aware formatter. Lines must have escape sequences stripped before text matching.
Implemented as a byte-level state machine in `log_parser.rs::strip_ansi()` — no
external dependency required.

### Key-value extraction

LLM request lines carry structured fields: `model="qwen2.5-coder:7b" provider=Ollama`.
Two helper functions handle quoted and unquoted values:
- `extract_quoted(line, key)` — scans for `key="..."` and returns the inner string
- `extract_unquoted(line, key)` — scans for `key=...` and returns up to the next space

### `:insights` command

Adds a single `match` arm to `execute_command()` in `src/editor/input.rs`:
1. Calls `crate::insights::parse_log_file(&log_path)` (sync, reads file)
2. Formats the result via `InsightSummary::format_report()`
3. Makes the agent panel visible
4. Pushes the report as `Role::Assistant` message

No async, no new UI widgets, no new dependencies.

---

## Phase 3 Implementation

### Aggregator (`src/insights/aggregator.rs`)

Parses `sessions.jsonl` line-by-line, routing each record by a fast `contains()`
pre-filter before handing it to serde:

- **`SessionEndRecord`** — captures `model`, `ended_by`, `files_changed`,
  `session_prompt_total`, `session_completion_total`, `session_rounds`, `ts`.
- **`ToolErrorRecord`** — captures `tool`, `error_type`, `ts` (written from Phase 2 onwards).
- **`SessionMetrics`** — aggregated totals: token sums, rounds histogram (buckets 1–19, 20+),
  total files changed, errors grouped by type.
- **`build_insights(data_dir)`** — joins `parse_log_file` (Phase 1) with
  `parse_sessions_jsonl` into a single `AggregatedInsights` struct consumed by the panel.

Missing or unreadable files produce an empty `SessionMetrics` — the overlay degrades
gracefully when Phase 2 data does not exist yet.

### Panel (`src/insights/panel.rs`)

Full-screen centred overlay (88 × 92% of terminal) rendered as a ratatui `Block` with:

- **Tab bar** — five tabs driven by `InsightsTab` enum; `Tab` / `Shift-Tab` / `1–5` to
  navigate.
- **Summary** — KV table of all headline numbers from both data sources.
- **Activity** — `BarChart` of LLM requests by time-of-day band (UTC) + sessions-per-day
  text sparkline.
- **Models** — inline bar chart per model (with % share), provider table, agentic/chat-only
  split.
- **Efficiency** — rounds-per-session `BarChart` histogram + stats panel (avg rounds,
  avg files, token totals, completion/prompt ratio). Empty-state message shown until
  `sessions.jsonl` data exists.
- **Errors** — log-level WARN/ERROR counts + `tool_error` breakdown by type. Empty-state
  message shown until Phase 2 data exists.

### Keybinding and mode

| Binding | Action | Mode |
|---------|--------|------|
| `SPC a I` | `InsightsDashboardOpen` | `Mode::InsightsDashboard` |
| `Tab` / `Shift-Tab` | next / prev tab | (inside overlay) |
| `1`–`5` | jump to tab | (inside overlay) |
| `j` / `k`, `↓` / `↑` | scroll | (inside overlay) |
| `Ctrl-d` / `Ctrl-u` | page down / up (10 lines) | (inside overlay) |
| `q` / `Esc` | close | (inside overlay) |

`InsightsDashboardOpen` reads `data_dir` from `Config::log_path().parent()` so it reuses
the same XDG-aware path resolution as the log itself.  No async, no new dependencies.

---

## Consequences

**Positive**
- Immediate value from data that already exists — no wait for Phase 2 data to accumulate.
- Zero new dependencies — pure `std` implementation.
- Establishes the `insights` module and `InsightSummary` type that all later phases extend.
- `:insights` is a natural place to add more metrics as data sources mature.

**Negative / trade-offs**
- Log parsing is fragile relative to structured data — log format changes break the parser.
  Mitigated by Phase 2 enrichment moving the canonical source to JSONL.
- ANSI codes in the log are an implementation detail that could change.
- Session boundaries inferred from "Starting forgiven" don't distinguish crash restarts from
  intentional sessions.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0092](0092-persistent-session-metrics-jsonl.md) | `sessions.jsonl` — the Phase 2 data source |
| [0095](0095-persistent-log-file.md) | `forgiven.log` — the Phase 1 data source |
| [0096](0096-session-rounds-and-avg-tokens-diagnostic.md) | Session round counter feeding `sessions.jsonl` |
| [0126](0126-token-efficiency-llm-interaction-quality-analysis.md) | Prior analysis using `sessions.jsonl` |
| [0128](0128-investigation-subagent.md) | Single-round agent pattern reused by Phase 4 narrative |
