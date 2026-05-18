use lsp_types::Diagnostic;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::path::PathBuf;

use crate::agent::{
    split_thinking, AgentPanel, AgentTask, AskUserInputState, AskUserState, AtPickerState,
    ChatMessage, ContentSegment, ProviderKind, Role, SlashMenuState,
};
use crate::buffer::{Cursor, Selection};
use crate::editor::{HoverPopupState, InlineAssistPhase, LocationListState};
use crate::explorer::FileExplorer;
use crate::keymap::Mode;
use crate::search::{SearchFocus, SearchState, SearchStatus};

mod agent_panel;
mod buffer_view;
mod markdown;
mod pickers;
mod popups;
mod search_lsp;
mod status;

/// Per-frame render cache for the agent panel.
/// Avoids re-parsing markdown and re-running split_thinking() for messages
/// that have not changed since the last frame.
struct PanelRenderCache {
    /// Completed-message lines + their exact ratatui row count.
    /// Valid when (msg_count, content_width) match.
    msg_count: usize,
    content_width: usize,
    msg_lines: Vec<Line<'static>>,
    msg_row_count: usize,
    /// Streaming-reply lines + their exact ratatui row count.
    /// Valid when (streaming_len, streaming_width) match.
    streaming_len: usize,
    streaming_width: usize,
    streaming_lines: Vec<Line<'static>>,
    streaming_row_count: usize,
    /// Cached MCP status bottom-bar line.
    /// Valid when (connected_count, failed_count) match.
    mcp_status_key: (usize, usize),
    mcp_bottom: Option<Line<'static>>,
}

impl Default for PanelRenderCache {
    fn default() -> Self {
        Self {
            msg_count: usize::MAX, // force initial render
            content_width: 0,
            msg_lines: Vec::new(),
            msg_row_count: 0,
            streaming_len: usize::MAX, // force initial render
            streaming_width: 0,
            streaming_lines: Vec::new(),
            streaming_row_count: 0,
            mcp_status_key: (usize::MAX, usize::MAX),
            mcp_bottom: None,
        }
    }
}

/// Returns the exact number of terminal rows that `lines` would occupy when
/// rendered in a [`Paragraph`] with `Wrap { trim: false }` at `inner_width`
/// columns.  Falls back to `lines.len()` (one row each) when `inner_width`
/// is zero to avoid a division-by-zero in ratatui's layout code.
fn wrapped_line_count(lines: &[Line<'static>], inner_width: usize) -> usize {
    if inner_width == 0 || lines.is_empty() {
        return lines.len();
    }
    Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false }).line_count(inner_width as u16)
}

thread_local! {
    static PANEL_CACHE: std::cell::RefCell<PanelRenderCache> =
        std::cell::RefCell::new(PanelRenderCache::default());
}

/// Data for the release notes popup (Mode::ReleaseNotes).
pub struct ReleaseNotesView<'a> {
    /// User's count input (phase 1: count-entry).
    pub count_input: &'a str,
    /// True while the AI request is in flight (phase 2: generating).
    pub generating: bool,
    /// Completed release notes text (phase 3: displaying).
    pub notes: &'a str,
    /// Vertical scroll offset for the notes display.
    pub scroll: u16,
}

/// Data for the diagnostics overlay (Mode::Diagnostics).
pub struct DiagnosticsData<'a> {
    /// Crate version string, e.g. "0.3.1".
    pub version: &'static str,
    /// MCP: (server_name, tool_count) for each connected server.
    pub mcp_connected: Vec<(&'a str, usize)>,
    /// MCP: (server_name, error) for each failed server.
    pub mcp_failed: &'a [(String, String)],
    /// LSP server names that are active.
    pub lsp_servers: Vec<&'a str>,
    /// Path to the log file.
    pub log_path: &'a str,
    /// Recent log entries (level, message) newest-last.
    pub recent_logs: &'a [(String, String)],
    /// Agent session token totals: (prompt_total, completion_total, context_window, rounds).
    /// prompt_total is cumulative re-send cost; divide by rounds for avg per-invocation.
    /// None when no agent session has been active yet (rounds == 0).
    pub agent_session_tokens: Option<(u32, u32, u32, u32)>,
    /// Per-segment context breakdown from the most recent agent invocation.
    /// Drives the Context Breakdown section in this overlay.
    pub agent_ctx_breakdown: Option<crate::agent::ContextBreakdown>,
    /// Current observation-masking threshold in characters (0 = disabled).
    pub observation_mask_threshold_chars: usize,
    /// Recent MCP tool calls this session (newest-last).
    pub mcp_call_log: Vec<crate::mcp::McpCallRecord>,
    /// Retrieval tool call counts this session: (read_file, get_symbol_context, get_file_outline).
    /// None when no agent session has been active yet.
    pub tool_retrieval_counts: Option<(u32, u32, u32)>,
    /// Codified context: (constitution_tokens, max_tokens, specialist_count, knowledge_count).
    /// None when the feature is disabled.
    pub codified_context_info: Option<(usize, usize, usize, usize)>,
    /// Companion sidecar status: (socket_bound, process_running, client_connected).
    pub sidecar_status: (bool, bool, bool),
}

/// Data for the file-info popup shown when `i` is pressed in the explorer.
pub struct FileInfoData {
    /// Full absolute path of the selected entry.
    pub path: std::path::PathBuf,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// File size in bytes (None for directories).
    pub size_bytes: Option<u64>,
    /// Last-modified time.
    pub modified: Option<std::time::SystemTime>,
    /// Creation time (not available on all platforms/filesystems).
    pub created: Option<std::time::SystemTime>,
    /// Unix permission string e.g. "rwxr-xr-x". None on non-Unix platforms.
    pub permissions: Option<String>,
}

/// View data for the inline assist overlay (Mode::InlineAssist, ADR 0111).
pub struct InlineAssistView<'a> {
    pub prompt: &'a str,
    pub response: &'a str,
    pub phase: InlineAssistPhase,
}

// Buffer data tuple: (name, is_modified, cursor, scroll_row, scroll_col, lines, selection)
type BufferData = (String, bool, Cursor, usize, usize, Vec<String>, Option<Selection>);

/// Fold rendering data passed to `render_buffer` (ADR 0106).
///
/// Contains which buffer rows are hidden (inside a closed fold) and which rows
/// are the start of a closed fold (displayed as a `··· N lines` stub).
pub struct FoldData {
    /// Buffer rows that must not be rendered (they are inside a closed fold).
    pub hidden_rows: std::collections::HashSet<usize>,
    /// Mapping from fold-start row → fold-end row for currently closed folds.
    /// Used to render the `··· N lines` stub on the fold-start line.
    pub fold_starts: std::collections::HashMap<usize, usize>,
}
// Buffer list tuple: (buffer names with modified flags, selected index)
type BufferList = (Vec<(String, bool)>, usize);
// File list tuple: (fuzzy-filtered entries with match indices, selected index, query)
type FileList = (Vec<(PathBuf, Vec<usize>)>, usize, String);

/// All per-frame data required to render the editor UI.
///
/// Adding a new mode means adding a field here (and populating it at the call
/// site) rather than growing the `UI::render` parameter list. `frame` is kept
/// as a direct parameter because its mutable borrow lifetime cannot be stored
/// in a struct.
pub struct RenderContext<'a> {
    pub mode: Mode,
    pub buffer_data: Option<&'a BufferData>,
    pub status_message: Option<&'a str>,
    pub command_buffer: Option<&'a str>,
    pub which_key_options: Option<&'a [(String, String)]>,
    pub key_sequence: &'a str,
    pub buffer_list: Option<&'a BufferList>,
    pub file_list: Option<&'a FileList>,
    /// LSP diagnostics for the current buffer.
    pub diagnostics: &'a [Diagnostic],
    /// Ghost-text inline suggestion: (text, buffer_row, buffer_col).
    pub ghost_text: Option<(&'a str, usize, usize)>,
    /// Agent chat panel; `None` = hidden.
    pub agent_panel: Option<&'a AgentPanel>,
    /// Pre-computed syntax-highlighted spans for the visible viewport.
    pub highlighted_lines: Option<&'a [Vec<Span<'static>>]>,
    /// File explorer panel; `None` = hidden.
    pub file_explorer: Option<&'a FileExplorer>,
    /// Pre-rendered markdown lines (Mode::MarkdownPreview only).
    pub preview_lines: Option<&'a [Line<'static>]>,
    /// Project-wide search overlay (Mode::Search only).
    pub search_state: Option<&'a SearchState>,
    /// Rename popup filename buffer (Mode::RenameFile only).
    pub rename_buffer: Option<&'a str>,
    /// Delete confirmation entry name (Mode::DeleteFile only).
    pub delete_name: Option<&'a str>,
    /// New folder name buffer (Mode::NewFolder only).
    pub new_folder_buffer: Option<&'a str>,
    /// Inactive split pane buffer data; `None` = no split active.
    pub split_buffer_data: Option<&'a BufferData>,
    /// Pre-computed highlighted spans for the inactive split pane.
    pub split_highlighted_lines: Option<&'a [Vec<Span<'static>>]>,
    /// `true` when the right pane is the focused pane.
    pub split_right_focused: bool,
    /// Editable commit message buffer (Mode::CommitMsg only).
    pub commit_msg: Option<&'a str>,
    /// Byte-offset cursor position within `commit_msg` (Mode::CommitMsg only).
    pub commit_msg_cursor: usize,
    /// Release notes popup data (Mode::ReleaseNotes only).
    pub release_notes: Option<&'a ReleaseNotesView<'a>>,
    /// Diagnostics overlay data (Mode::Diagnostics only).
    pub diag_overlay: Option<&'a DiagnosticsData<'a>>,
    /// Path of the binary file that triggered Mode::BinaryFile; `None` otherwise.
    pub binary_file_path: Option<&'a std::path::Path>,
    /// Time from process start to the editor being ready; shown on welcome screen.
    pub startup_elapsed: Option<std::time::Duration>,
    /// File-info popup data (explorer `i` key); `None` = hidden.
    pub file_info: Option<&'a FileInfoData>,
    /// LSP location list overlay (Mode::LocationList only).
    pub location_list: Option<&'a LocationListState>,
    /// In-file search query string (Mode::InFileSearch only).
    pub in_file_search_query: Option<&'a str>,
    /// Hover popup (Mode::LspHover only).
    pub hover_popup: Option<&'a HoverPopupState>,
    /// Text in the LSP rename input (Mode::LspRename only).
    pub lsp_rename_buffer: Option<&'a str>,
    /// Code fold data for the primary buffer: hidden rows + fold stubs (ADR 0106).
    /// `None` when no folds are active for the current buffer.
    pub fold_data: Option<&'a FoldData>,
    /// Sticky scroll context header text (ADR 0107).
    /// First line of the innermost enclosing scope that started above `scroll_row`.
    pub sticky_header: Option<&'a str>,
    /// Inline assist overlay data (Mode::InlineAssist, ADR 0111).
    pub inline_assist: Option<InlineAssistView<'a>>,
    /// Review changes overlay data (Mode::ReviewChanges, ADR 0113).
    pub review_changes: Option<&'a crate::editor::ReviewChangesState>,
    /// Insights dashboard overlay data (Mode::InsightsDashboard, ADR 0129).
    pub insights_dashboard: Option<&'a crate::insights::panel::InsightsDashboardState>,
    /// When `true`, long lines are visually wrapped at the viewport edge.
    pub soft_wrap: bool,
    /// Syntax highlighter — used for code blocks inside markdown rendering.
    pub highlighter: &'a crate::highlight::Highlighter,
    /// Debt metrics for the welcome-screen dashboard; `None` while loading.
    pub debt_report: Option<&'a crate::debt::DebtReport>,
    /// Qualitative LLM narrative for the debt dashboard; `None` until Ollama responds.
    pub debt_narrative: Option<&'a str>,
}

/// Agent panel default width as a percentage of total terminal width when the panel
/// is visible alongside the editor but WITHOUT the file explorer.
/// Tune this constant to adjust the agent-to-editor split without touching layout code.
const AGENT_PANEL_PCT_ALONE: u16 = 55;

/// Agent panel default width as a percentage of total terminal width when the panel
/// is visible alongside BOTH the editor and the file explorer.
/// The explorer takes a fixed 25 columns; the editor fills whatever remains.
const AGENT_PANEL_PCT_WITH_EXPLORER: u16 = 50;

/// UI rendering for the editor
pub struct UI;

impl UI {
    /// Render the entire UI for one frame.
    pub fn render(frame: &mut Frame, ctx: &RenderContext<'_>) {
        // Unpack context into same-named locals so the body below is unchanged.
        let mode = ctx.mode;
        let buffer_data = ctx.buffer_data;
        let status_message = ctx.status_message;
        let command_buffer = ctx.command_buffer;
        let which_key_options = ctx.which_key_options;
        let key_sequence = ctx.key_sequence;
        let buffer_list = ctx.buffer_list;
        let file_list = ctx.file_list;
        let diagnostics = ctx.diagnostics;
        let ghost_text = ctx.ghost_text;
        let agent_panel = ctx.agent_panel;
        let highlighted_lines = ctx.highlighted_lines;
        let file_explorer = ctx.file_explorer;
        let preview_lines = ctx.preview_lines;
        let search_state = ctx.search_state;
        let rename_buffer = ctx.rename_buffer;
        let delete_name = ctx.delete_name;
        let new_folder_buffer = ctx.new_folder_buffer;
        let split_buffer_data = ctx.split_buffer_data;
        let split_highlighted_lines = ctx.split_highlighted_lines;
        let split_right_focused = ctx.split_right_focused;
        let commit_msg = ctx.commit_msg;
        let commit_msg_cursor = ctx.commit_msg_cursor;
        let release_notes = ctx.release_notes;
        let diag_overlay = ctx.diag_overlay;
        let binary_file_path = ctx.binary_file_path;
        let startup_elapsed = ctx.startup_elapsed;
        let file_info = ctx.file_info;
        let location_list = ctx.location_list;
        let in_file_search_query = ctx.in_file_search_query;
        let hover_popup = ctx.hover_popup;
        let lsp_rename_buffer = ctx.lsp_rename_buffer;
        let fold_data = ctx.fold_data;
        let sticky_header = ctx.sticky_header;
        let soft_wrap = ctx.soft_wrap;

        let size = frame.area();

        // If in PickBuffer mode, show buffer picker
        if mode == Mode::PickBuffer {
            Self::render_buffer_picker(frame, buffer_list, size);
            return;
        }

        // If in PickFile mode, show file picker
        if mode == Mode::PickFile {
            Self::render_file_picker(frame, file_list, size);
            return;
        }

        // If in Search mode, show the ripgrep search overlay
        if mode == Mode::Search {
            if let Some(state) = search_state {
                Self::render_search_panel(frame, state, size);
            }
            return;
        }

        // ── Layout: panel split ratios, pane ownership, and z-order ──────────────
        //
        // HORIZONTAL COLUMNS (left → right):
        //   1. Explorer sidebar : Constraint::Length(25)                  — fixed 25-col tree
        //   2. Editor area      : Constraint::Min(1)                      — grows to fill remainder
        //   3. Agent panel      : Constraint::Percentage(AGENT_PANEL_PCT) — see constants below
        //
        // Split cases:
        //   Explorer + Agent visible  → agent = AGENT_PANEL_PCT_WITH_EXPLORER %
        //   Agent only (no explorer)  → agent = AGENT_PANEL_PCT_ALONE %
        //   Explorer only (no agent)  → editor fills all remaining cols (Constraint::Min(1))
        //   Neither                   → editor fills entire terminal width
        //
        // VERTICAL SPLIT (when two buffers open side-by-side):
        //   Left pane  : Percentage(50)
        //   Separator  : Length(1)  — single │ glyph in DarkGray
        //   Right pane : Percentage(50)
        //
        // Z-ORDER (paint order — later items render on top of earlier ones):
        //   1. Editor buffer    (background; clips to editor_area)
        //   2. Explorer sidebar (left overlay, clips to left_sidebar_area)
        //   3. Agent panel      (right overlay, clips to agent_area)
        //   4. Status line      (always above editor, below any modal)
        //   5. Modal popups     (rename, delete, binary, commit-msg, etc. — topmost)
        //
        // AGENT PANEL VERTICAL LAYOUT (inside agent_area):
        //   title_bar  : Length(1)  — provider · model · session id          (P0-S4)
        //   history    : Min(1)     — scrollable message history
        //   task_strip : Length(N)  — agentic plan steps (omitted when empty)
        //   token_bar  : Length(1)  — token budget footer                     (P0-S3)
        //   input      : Length(H)  — user input box (1–10 text lines + badges + 2 borders)
        //
        // ─────────────────────────────────────────────────────────────────────────────

        let explorer_visible = file_explorer.map(|e| e.visible).unwrap_or(false);
        let agent_visible = agent_panel.map(|p| p.visible).unwrap_or(false);
        let left_sidebar_visible = explorer_visible;

        let (left_sidebar_area, content_area, agent_area) =
            match (left_sidebar_visible, agent_visible) {
                (true, true) => {
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Length(25),
                            Constraint::Min(1),
                            Constraint::Percentage(AGENT_PANEL_PCT_WITH_EXPLORER),
                        ])
                        .split(size);
                    (Some(cols[0]), cols[1], Some(cols[2]))
                },
                (true, false) => {
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Length(25), Constraint::Min(1)])
                        .split(size);
                    (Some(cols[0]), cols[1], None)
                },
                (false, true) => {
                    let cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Percentage(100 - AGENT_PANEL_PCT_ALONE),
                            Constraint::Percentage(AGENT_PANEL_PCT_ALONE),
                        ])
                        .split(size);
                    (None, cols[0], Some(cols[1]))
                },
                (false, false) => (None, size, None),
            };
        let editor_area = content_area;

        // ── Vertical layout (buffer + status) inside editor_area ─────────────
        let constraints = if let Some(wk) = which_key_options {
            // 2 (borders) + 1 (header) + number of options
            let wk_height = (wk.len() as u16) + 3;
            vec![
                Constraint::Min(1),            // Main buffer area
                Constraint::Length(wk_height), // Which-key popup (dynamic)
                Constraint::Length(1),         // Status line
            ]
        } else {
            vec![
                Constraint::Min(1),    // Main buffer area
                Constraint::Length(1), // Status line
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(editor_area);

        let main_area = chunks[0];
        let status_area = if let Some(wk) = which_key_options {
            let which_key_area = chunks[1];
            Self::render_which_key(frame, wk, which_key_area);
            chunks[2]
        } else {
            chunks[1]
        };

        // Render buffer content — single pane or vertical split
        if let Some(split_data) = split_buffer_data {
            // 3-column layout: [left pane | 1-char separator | right pane]
            let split_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(50),
                    Constraint::Length(1),
                    Constraint::Percentage(50),
                ])
                .split(main_area);

            // Determine which buffer goes in which pane.
            // current buffer (buffer_data) is always the focused pane.
            let (left_data, right_data): (Option<&BufferData>, Option<&BufferData>) =
                if split_right_focused {
                    (Some(split_data), buffer_data)
                } else {
                    (buffer_data, Some(split_data))
                };
            let (left_hl, right_hl) = if split_right_focused {
                (split_highlighted_lines, highlighted_lines)
            } else {
                (highlighted_lines, split_highlighted_lines)
            };
            let (left_ghost, right_ghost) =
                if split_right_focused { (None, ghost_text) } else { (ghost_text, None) };
            let left_preview = if split_right_focused { None } else { preview_lines };

            Self::render_buffer(
                frame,
                left_data,
                mode,
                split_chunks[0],
                diagnostics,
                left_ghost,
                left_hl,
                left_preview,
                !split_right_focused,
                startup_elapsed,
                None,
                None,
                soft_wrap,
                None,
                None,
            );

            // Draw vertical separator
            let sep_lines: Vec<Line> = (0..split_chunks[1].height)
                .map(|_| Line::from(Span::styled("│", Style::default().fg(Color::DarkGray))))
                .collect();
            frame.render_widget(Paragraph::new(sep_lines), split_chunks[1]);

            Self::render_buffer(
                frame,
                right_data,
                mode,
                split_chunks[2],
                diagnostics,
                right_ghost,
                right_hl,
                None,
                split_right_focused,
                startup_elapsed,
                None,
                None,
                soft_wrap,
                None,
                None,
            );
        } else {
            Self::render_buffer(
                frame,
                buffer_data,
                mode,
                main_area,
                diagnostics,
                ghost_text,
                highlighted_lines,
                preview_lines,
                true,
                startup_elapsed,
                fold_data,
                sticky_header,
                soft_wrap,
                ctx.debt_report,
                ctx.debt_narrative,
            );
        }

        // Render status line
        let agent_fuel = agent_panel.and_then(|p| p.last_breakdown).map(|b| b.used_pct());
        Self::render_status_line(
            frame,
            buffer_data,
            mode,
            status_message,
            command_buffer,
            in_file_search_query,
            key_sequence,
            status_area,
            diagnostics,
            agent_fuel,
        );

        // Render agent panel if visible
        if let (Some(panel), Some(area)) = (agent_panel, agent_area) {
            Self::render_agent_panel(frame, panel, mode, area, ctx.highlighter);
        }

        // Render left sidebar (explorer only now)
        if let Some(area) = left_sidebar_area {
            if let Some(explorer) = file_explorer {
                if explorer.visible {
                    Self::render_file_explorer(frame, explorer, mode, area);
                }
            }
        }

        // Render rename popup if active
        if let Some(name) = rename_buffer {
            Self::render_rename_popup(frame, name, size);
        }

        // Render delete confirmation popup if active
        if let Some(name) = delete_name {
            Self::render_delete_popup(frame, name, size);
        }

        // Render binary file popup if active
        if let Some(path) = binary_file_path {
            Self::render_binary_file_popup(frame, path, size);
        }

        // Render new folder popup if active
        if let Some(name) = new_folder_buffer {
            Self::render_new_folder_popup(frame, name, size);
        }

        // Render commit message popup if active
        if let Some(msg) = commit_msg {
            Self::render_commit_msg_popup(frame, msg, commit_msg_cursor, size);
        }

        // Render release notes popup if active
        if let Some(view) = release_notes {
            Self::render_release_notes_popup(frame, view, size);
        }

        // Render diagnostics overlay if active
        if let Some(diag) = diag_overlay {
            Self::render_diagnostics_overlay(frame, diag, size);
        }

        // Render LSP location list overlay if active
        if let Some(list) = location_list {
            Self::render_location_list(frame, list, size);
        }

        // Render hover popup (Mode::LspHover)
        if let Some(popup) = hover_popup {
            Self::render_hover_popup(frame, popup, size);
        }

        // Render LSP rename input popup (Mode::LspRename)
        if let Some(buf) = lsp_rename_buffer {
            Self::render_lsp_rename_popup(frame, buf, size);
        }

        // Render file-info popup if active (explorer `i` key)
        if let Some(info) = file_info {
            let explorer_right_edge = if explorer_visible { 25u16 } else { 0 };
            Self::render_file_info_popup(frame, info, size, explorer_right_edge);
        }

        // Render inline assist overlay (Mode::InlineAssist, ADR 0111)
        if let Some(view) = &ctx.inline_assist {
            Self::render_inline_assist_overlay(frame, view, size);
        }

        // Render review changes overlay (Mode::ReviewChanges, ADR 0113)
        if let Some(review) = ctx.review_changes {
            Self::render_review_changes_overlay(frame, review, size);
        }

        // Render insights dashboard overlay (Mode::InsightsDashboard, ADR 0129)
        if let Some(dashboard) = ctx.insights_dashboard {
            crate::insights::panel::render_insights_dashboard(frame, dashboard, size);
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use ratatui::{backend::TestBackend, Terminal};

    use super::*;
    use crate::agent::AgentPanel;
    use crate::highlight::Highlighter;

    /// Build a minimal RenderContext that exercises the agent panel rendering path.
    /// All optional fields are None; the agent panel is visible and focused.
    fn render_with_agent_panel_at(cols: u16, rows: u16) {
        let backend = TestBackend::new(cols, rows);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut panel = AgentPanel::new();
        panel.visible = true;

        let highlighter = Highlighter::new();

        terminal
            .draw(|frame| {
                let ctx = RenderContext {
                    mode: Mode::Agent,
                    buffer_data: None,
                    status_message: None,
                    command_buffer: None,
                    which_key_options: None,
                    key_sequence: "",
                    buffer_list: None,
                    file_list: None,
                    diagnostics: &[],
                    ghost_text: None,
                    agent_panel: Some(&panel),
                    highlighted_lines: None,
                    file_explorer: None,
                    preview_lines: None,
                    search_state: None,
                    rename_buffer: None,
                    delete_name: None,
                    new_folder_buffer: None,
                    split_buffer_data: None,
                    split_highlighted_lines: None,
                    split_right_focused: false,
                    commit_msg: None,
                    commit_msg_cursor: 0,
                    release_notes: None,
                    diag_overlay: None,
                    binary_file_path: None,
                    startup_elapsed: None,
                    file_info: None,
                    location_list: None,
                    in_file_search_query: None,
                    hover_popup: None,
                    lsp_rename_buffer: None,
                    fold_data: None,
                    sticky_header: None,
                    inline_assist: None,
                    review_changes: None,
                    insights_dashboard: None,
                    soft_wrap: false,
                    highlighter: &highlighter,
                    debt_report: None,
                    debt_narrative: None,
                };
                UI::render(frame, &ctx);
            })
            .unwrap();

        // Verify the buffer dimensions match what was requested — overflow or a
        // geometry panic would have already aborted the draw call above.
        let buf = terminal.backend().buffer().clone();
        assert_eq!(buf.area().width, cols, "buffer width mismatch at {cols} cols");
        assert_eq!(buf.area().height, rows, "buffer height mismatch at {cols} cols");
    }

    #[test]
    fn agent_panel_renders_at_80_cols() {
        render_with_agent_panel_at(80, 40);
    }

    #[test]
    fn agent_panel_renders_at_120_cols() {
        render_with_agent_panel_at(120, 40);
    }

    #[test]
    fn agent_panel_renders_at_200_cols() {
        render_with_agent_panel_at(200, 40);
    }
}
