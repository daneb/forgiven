//! Phase 3: Insights Dashboard overlay (Mode::InsightsDashboard, ADR 0129).
//!
//! Triggered by `SPC a I`. Renders a full-screen centred overlay with five
//! tabs driven entirely by local data — no network calls required.
//!
//! ## Tabs
//!
//! | # | Name       | Contents |
//! |---|------------|---------|
//! | 0 | Summary    | At-a-glance: sessions, LLM requests, active days, date range |
//! | 1 | Activity   | Time-of-day bar chart + sessions-per-day sparkline |
//! | 2 | Models     | Request counts by model and provider; agentic vs chat-only split |
//! | 3 | Efficiency | Rounds/session histogram, token totals, files/session |
//! | 4 | Errors     | Tool error breakdown (Phase 2+ data; greyed out when absent) |

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{BarChart, Block, Borders, Clear, Paragraph, Tabs, Wrap},
    Frame,
};

use crate::insights::aggregator::AggregatedInsights;

// ─────────────────────────────────────────────────────────────────────────────
// Tab enum
// ─────────────────────────────────────────────────────────────────────────────

/// Which tab is currently shown in the Insights overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightsTab {
    Summary = 0,
    Activity = 1,
    Models = 2,
    Efficiency = 3,
    Errors = 4,
}

impl InsightsTab {
    const LABELS: [&'static str; 5] =
        [" Summary ", " Activity ", " Models ", " Efficiency ", " Errors "];

    pub fn next(self) -> Self {
        match self {
            Self::Summary => Self::Activity,
            Self::Activity => Self::Models,
            Self::Models => Self::Efficiency,
            Self::Efficiency => Self::Errors,
            Self::Errors => Self::Summary,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Summary => Self::Errors,
            Self::Activity => Self::Summary,
            Self::Models => Self::Activity,
            Self::Efficiency => Self::Models,
            Self::Errors => Self::Efficiency,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

/// All state for `Mode::InsightsDashboard`.
pub struct InsightsDashboardState {
    /// Pre-computed analytics from all data sources.
    pub insights: AggregatedInsights,
    /// Which tab is currently showing.
    pub active_tab: InsightsTab,
    /// Vertical scroll offset within the active tab's content.
    pub scroll: usize,
    /// Live Copilot quota snapshot (None for non-Copilot providers).
    pub copilot_quota: Option<crate::agent::CopilotQuota>,
}

impl InsightsDashboardState {
    pub fn new(
        insights: AggregatedInsights,
        copilot_quota: Option<crate::agent::CopilotQuota>,
    ) -> Self {
        Self { insights, active_tab: InsightsTab::Summary, scroll: 0, copilot_quota }
    }

    pub fn next_tab(&mut self) {
        self.active_tab = self.active_tab.next();
        self.scroll = 0;
    }

    pub fn prev_tab(&mut self) {
        self.active_tab = self.active_tab.prev();
        self.scroll = 0;
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level renderer
// ─────────────────────────────────────────────────────────────────────────────

/// Render the full Insights overlay onto `frame` within `area`.
///
/// Called from `src/ui/mod.rs` when `mode == Mode::InsightsDashboard`.
pub fn render_insights_dashboard(frame: &mut Frame, state: &InsightsDashboardState, area: Rect) {
    let popup = centered_rect(88, 92, area);
    frame.render_widget(Clear, popup);

    let outer = Block::default()
        .title(
            " Forgiven Insights  \
             [Tab/Shift-Tab: switch tab  ·  j/k or ↓/↑: scroll  ·  Esc: close] ",
        )
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);

    // ── Layout: tab bar (1 row) + content area ────────────────────────────
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    render_tab_bar(frame, state, layout[0]);

    match state.active_tab {
        InsightsTab::Summary => render_summary(frame, state, layout[1]),
        InsightsTab::Activity => render_activity(frame, state, layout[1]),
        InsightsTab::Models => render_models(frame, state, layout[1]),
        InsightsTab::Efficiency => render_efficiency(frame, state, layout[1]),
        InsightsTab::Errors => render_errors(frame, state, layout[1]),
    }
}

fn render_tab_bar(frame: &mut Frame, state: &InsightsDashboardState, area: Rect) {
    let titles: Vec<Line> = InsightsTab::LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let active = i == state.active_tab as usize;
            let style = if active {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(*label, style))
        })
        .collect();

    let tabs = Tabs::new(titles)
        .select(state.active_tab as usize)
        .divider(Span::styled("│", Style::default().fg(Color::DarkGray)));
    frame.render_widget(tabs, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab: Summary
// ─────────────────────────────────────────────────────────────────────────────

fn render_summary(frame: &mut Frame, state: &InsightsDashboardState, area: Rect) {
    let log = &state.insights.log;
    let ses = &state.insights.sessions;

    let date_range = match (&log.first_date, &log.last_date) {
        (Some(f), Some(l)) if f == l => f.clone(),
        (Some(f), Some(l)) => format!("{f} → {l}"),
        _ => "no data yet".to_string(),
    };

    let total_requests = log.llm_request_count + log.one_shot_count;
    let msgs_per_day =
        if log.active_days > 0 { total_requests as f64 / log.active_days as f64 } else { 0.0 };

    let mut lines: Vec<Line> = Vec::new();

    // ── Header ────────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "  At a Glance",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    let header_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let val_style = Style::default().fg(Color::Yellow);
    let dim = Style::default().fg(Color::DarkGray);

    macro_rules! kv {
        ($label:expr, $val:expr) => {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<22}", $label), header_style),
                Span::styled($val, val_style),
            ]));
        };
    }

    kv!("Date range", date_range);
    kv!("Active days", log.active_days.to_string());
    kv!("Editor sessions", log.session_count.to_string());
    kv!("LLM requests", format!("{total_requests}  ({msgs_per_day:.1}/day)"));
    if log.llm_request_count > 0 {
        kv!(
            "  agentic rounds",
            log.llm_request_count.saturating_sub(log.chat_only_count).to_string()
        );
        kv!("  chat-only rounds", log.chat_only_count.to_string());
    }
    if log.one_shot_count > 0 {
        kv!("  one-shot (inline)", log.one_shot_count.to_string());
    }
    kv!("Buffer saves", log.buffer_save_count.to_string());

    // ── sessions.jsonl summary ────────────────────────────────────────────
    lines.push(Line::default());
    if ses.sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No sessions.jsonl data yet — run more sessions to unlock Efficiency tab.",
            dim,
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Session records (sessions.jsonl)",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
        kv!("Recorded sessions", ses.sessions.len().to_string());
        kv!("Avg rounds/session", format!("{:.1}", ses.avg_rounds()));
        kv!("Avg files/session", format!("{:.1}", ses.avg_files()));
        if ses.sessions_with_tokens > 0 {
            kv!("Total prompt tokens", fmt_tokens(ses.total_prompt_tokens));
            kv!("Total completion tokens", fmt_tokens(ses.total_completion_tokens));
        }
    }

    // ── Copilot quota ─────────────────────────────────────────────────────
    if let Some(ref quota) = state.copilot_quota {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  Copilot Premium Requests",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());

        let pct_used = 100.0 - quota.premium_percent_remaining;
        let used = quota.premium_entitlement.saturating_sub(quota.premium_remaining);
        let bar_filled = ((pct_used / 100.0) * 30.0).round() as usize;
        let bar_empty = 30usize.saturating_sub(bar_filled);
        let bar_color = if pct_used >= 90.0 {
            Color::Red
        } else if pct_used >= 70.0 {
            Color::Yellow
        } else {
            Color::Green
        };

        lines.push(Line::from(vec![
            Span::styled("  [", Style::default().fg(Color::DarkGray)),
            Span::styled("█".repeat(bar_filled), Style::default().fg(bar_color)),
            Span::styled("░".repeat(bar_empty), Style::default().fg(Color::DarkGray)),
            Span::styled("] ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{pct_used:.1}% used"),
                Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::default());
        kv!("Used / Included", format!("{used} / {}", quota.premium_entitlement));
        kv!("Remaining", quota.premium_remaining.to_string());
        if quota.overage_permitted {
            kv!("Overage", format!("{} (permitted)", quota.overage_count));
        }
        kv!("Resets", quota.reset_date.clone());
    }

    // ── Warnings / errors ─────────────────────────────────────────────────
    if log.warn_count > 0 || log.error_count > 0 {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  Warnings & Errors (log)",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
        if log.warn_count > 0 {
            kv!("Warnings", log.warn_count.to_string());
        }
        if log.error_count > 0 {
            kv!(
                "Errors",
                Span::styled(log.error_count.to_string(), Style::default().fg(Color::Red))
                    .content
                    .to_string()
            );
        }
    }

    // ── Phase 4 narrative ─────────────────────────────────────────────────
    lines.push(Line::default());
    match &state.insights.narrative {
        Some(narrative) => {
            lines.push(Line::from(Span::styled(
                "  Narrative",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::default());
            for text_line in narrative.lines() {
                let styled = if text_line.starts_with("## ") {
                    Span::styled(
                        format!("  {text_line}"),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::styled(format!("  {text_line}"), Style::default().fg(Color::White))
                };
                lines.push(Line::from(styled));
            }
        },
        None => {
            lines.push(Line::from(Span::styled(
                "  No narrative yet — run  :insights summarize  to generate one.",
                Style::default().fg(Color::DarkGray),
            )));
        },
    }

    render_scrollable_paragraph(frame, lines, state.scroll, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab: Activity
// ─────────────────────────────────────────────────────────────────────────────

fn render_activity(frame: &mut Frame, state: &InsightsDashboardState, area: Rect) {
    let log = &state.insights.log;

    // ── Layout: top half = time-of-day bars, bottom = sparkline ──────────
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1), Constraint::Min(4)])
        .split(area);

    // ── Time-of-day BarChart ──────────────────────────────────────────────
    let bands: [(&str, std::ops::Range<usize>); 4] = [
        ("Night\n00–06", 0..6),
        ("Morning\n06–12", 6..12),
        ("Afternoon\n12–18", 12..18),
        ("Evening\n18–24", 18..24),
    ];
    let bar_data: Vec<(&str, u64)> = bands
        .iter()
        .map(|(label, range)| {
            let total: usize = range.clone().map(|h| log.requests_by_hour[h]).sum();
            (*label, total as u64)
        })
        .collect();

    let bar_chart = BarChart::default()
        .block(
            Block::default()
                .title(" LLM requests by time of day (UTC) ")
                .title_style(Style::default().fg(Color::Cyan))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .data(&bar_data)
        .bar_width(14)
        .bar_gap(2)
        .bar_style(Style::default().fg(Color::Yellow))
        .value_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(bar_chart, layout[0]);

    // ── Sessions per day (text sparkline) ─────────────────────────────────
    let mut spark_lines: Vec<Line> = Vec::new();
    spark_lines.push(Line::from(Span::styled(
        " Sessions per day",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    spark_lines.push(Line::default());

    if log.sessions_by_date.is_empty() {
        spark_lines.push(Line::from(Span::styled(
            "  No session starts recorded yet.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let max_count = *log.sessions_by_date.values().max().unwrap_or(&1).max(&1);
        for (date, &count) in &log.sessions_by_date {
            let filled = (count * 12 / max_count).min(12);
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(12 - filled));
            spark_lines.push(Line::from(vec![
                Span::styled(format!("  {date}  "), Style::default().fg(Color::DarkGray)),
                Span::styled(bar, Style::default().fg(Color::Yellow)),
                Span::styled(format!("  {count}"), Style::default().fg(Color::White)),
            ]));
        }
    }

    let para = Paragraph::new(spark_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(para, layout[2]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab: Models
// ─────────────────────────────────────────────────────────────────────────────

fn render_models(frame: &mut Frame, state: &InsightsDashboardState, area: Rect) {
    let log = &state.insights.log;
    let total = log.llm_request_count + log.one_shot_count;
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // ── Models ────────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "  Models",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    if log.models.is_empty() {
        lines.push(Line::from(Span::styled("  No model data yet.", dim)));
    } else {
        let mut by_count: Vec<_> = log.models.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1));
        let max = *by_count[0].1;
        for (model, count) in &by_count {
            let bar_len = (*count * 20 / max).min(20);
            let bar = "█".repeat(bar_len);
            let pct = (*count * 100).checked_div(total).unwrap_or(0);
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<36}", model), Style::default().fg(Color::White)),
                Span::styled(format!("{bar:<20}"), Style::default().fg(Color::Yellow)),
                Span::styled(format!("  {} ({pct}%)", count), Style::default().fg(Color::White)),
            ]));
        }
    }

    // ── Providers ─────────────────────────────────────────────────────────
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Providers",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    if log.providers.is_empty() {
        lines.push(Line::from(Span::styled("  No provider data yet.", dim)));
    } else {
        let mut by_count: Vec<_> = log.providers.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1));
        for (provider, count) in &by_count {
            let pct = (*count * 100).checked_div(total).unwrap_or(0);
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<24}", provider), Style::default().fg(Color::White)),
                Span::styled(
                    format!("{count} requests ({pct}%)"),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }
    }

    // ── Agentic vs chat-only split ─────────────────────────────────────────
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Agentic vs Chat-only",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    let agentic = log.llm_request_count.saturating_sub(log.chat_only_count);
    let chat = log.chat_only_count;
    let one_shot = log.one_shot_count;
    let total_agentic_chat = agentic + chat;
    if total_agentic_chat == 0 && one_shot == 0 {
        lines.push(Line::from(Span::styled("  No requests recorded yet.", dim)));
    } else {
        if total_agentic_chat > 0 {
            let a_pct = agentic * 100 / total_agentic_chat.max(1);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<22}", "Agentic (tool-calling)"),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!("{agentic} ({a_pct}%)"), Style::default().fg(Color::Yellow)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<22}", "Chat-only"), Style::default().fg(Color::White)),
                Span::styled(
                    format!("{chat} ({}%)", 100 - a_pct),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }
        if one_shot > 0 {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<22}", "One-shot (inline)"),
                    Style::default().fg(Color::White),
                ),
                Span::styled(one_shot.to_string(), Style::default().fg(Color::Yellow)),
            ]));
        }
    }

    render_scrollable_paragraph(frame, lines, state.scroll, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab: Efficiency
// ─────────────────────────────────────────────────────────────────────────────

fn render_efficiency(frame: &mut Frame, state: &InsightsDashboardState, area: Rect) {
    let ses = &state.insights.sessions;
    let dim = Style::default().fg(Color::DarkGray);

    if ses.sessions.is_empty() {
        let lines = vec![
            Line::default(),
            Line::from(Span::styled("  No sessions.jsonl data yet.", dim)),
            Line::default(),
            Line::from(Span::styled(
                "  The Efficiency tab unlocks once sessions.jsonl has records.",
                dim,
            )),
            Line::from(Span::styled(
                "  session_end records are written when you start a new conversation",
                dim,
            )),
            Line::from(Span::styled("  (SPC a n) or quit the editor.", dim)),
        ];
        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para, area);
        return;
    }

    // ── Layout: histogram top + stats bottom ──────────────────────────────
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1), Constraint::Min(4)])
        .split(area);

    // ── Rounds-per-session histogram ──────────────────────────────────────
    let hist_data: Vec<(String, u64)> = ses
        .rounds_histogram
        .iter()
        .map(|(k, v)| {
            let label = if *k == 20 { "20+".to_owned() } else { k.to_string() };
            (label, *v as u64)
        })
        .collect();
    let hist_refs: Vec<(&str, u64)> = hist_data.iter().map(|(l, v)| (l.as_str(), *v)).collect();

    let bar_chart = BarChart::default()
        .block(
            Block::default()
                .title(" Rounds per session ")
                .title_style(Style::default().fg(Color::Cyan))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .data(&hist_refs)
        .bar_width(5)
        .bar_gap(1)
        .bar_style(Style::default().fg(Color::Yellow))
        .value_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(bar_chart, layout[0]);

    // ── Stats ─────────────────────────────────────────────────────────────
    let mut stat_lines: Vec<Line> = Vec::new();
    stat_lines.push(Line::from(Span::styled(
        " Session statistics",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    stat_lines.push(Line::default());

    let lbl = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let val = Style::default().fg(Color::Yellow);

    macro_rules! stat {
        ($label:expr, $val:expr) => {
            stat_lines.push(Line::from(vec![
                Span::styled(format!("  {:<28}", $label), lbl),
                Span::styled($val, val),
            ]));
        };
    }

    stat!("Recorded sessions", ses.sessions.len().to_string());
    stat!("Avg rounds/session", format!("{:.1}", ses.avg_rounds()));
    stat!("Max rounds (single session)", ses.max_rounds.to_string());
    stat!("Avg files changed/session", format!("{:.1}", ses.avg_files()));
    stat!("Total files changed", ses.total_files_changed.to_string());

    if ses.sessions_with_tokens > 0 {
        stat_lines.push(Line::default());
        stat!("Sessions with token data", ses.sessions_with_tokens.to_string());
        stat!("Total prompt tokens", fmt_tokens(ses.total_prompt_tokens));
        stat!("Total completion tokens", fmt_tokens(ses.total_completion_tokens));
        let ratio = if ses.total_prompt_tokens > 0 {
            ses.total_completion_tokens as f64 / ses.total_prompt_tokens as f64
        } else {
            0.0
        };
        stat!("Completion/prompt ratio", format!("{ratio:.2}"));
    }

    let para = Paragraph::new(stat_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(para, layout[2]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab: Errors
// ─────────────────────────────────────────────────────────────────────────────

fn render_errors(frame: &mut Frame, state: &InsightsDashboardState, area: Rect) {
    let log = &state.insights.log;
    let ses = &state.insights.sessions;
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // ── Log-level warnings / errors ───────────────────────────────────────
    lines.push(Line::from(Span::styled(
        "  Log warnings & errors",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    if log.warn_count == 0 && log.error_count == 0 {
        lines.push(Line::from(Span::styled("  No warnings or errors recorded.", dim)));
    } else {
        let lbl = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
        if log.warn_count > 0 {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<22}", "Warnings"), lbl),
                Span::styled(log.warn_count.to_string(), Style::default().fg(Color::Yellow)),
            ]));
        }
        if log.error_count > 0 {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<22}", "Errors"), lbl),
                Span::styled(
                    log.error_count.to_string(),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }

    // ── Tool errors (Phase 2+ data) ───────────────────────────────────────
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Tool errors (sessions.jsonl)",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    if ses.errors_by_type.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No tool_error records — these are written from Phase 2 onwards.",
            dim,
        )));
    } else {
        let mut by_count: Vec<_> = ses.errors_by_type.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1));
        let max = *by_count[0].1;
        for (err_type, count) in &by_count {
            let bar_len = (*count * 16 / max).min(16);
            let bar = "█".repeat(bar_len);
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<28}", err_type), Style::default().fg(Color::White)),
                Span::styled(format!("{bar:<16}"), Style::default().fg(Color::Red)),
                Span::styled(format!("  {}", count), Style::default().fg(Color::White)),
            ]));
        }
    }

    render_scrollable_paragraph(frame, lines, state.scroll, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Render `lines` as a scrolled paragraph with a subtle border.
fn render_scrollable_paragraph(
    frame: &mut Frame,
    lines: Vec<Line<'static>>,
    scroll: usize,
    area: Rect,
) {
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .scroll((scroll as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

/// Return a centred [`Rect`] that is `percent_x`% wide and `percent_y`% tall
/// relative to `r`.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Format a large token count with `k` / `M` suffix for readability.
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
