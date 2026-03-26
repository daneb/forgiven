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
    split_thinking, AgentPanel, AgentTask, AskUserState, AtPickerState, ContentSegment, Role,
    SlashMenuState,
};
use crate::buffer::{Cursor, Selection};
use crate::editor::{DiffLine, LocationListState};
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

/// Data for the full-screen apply-diff overlay (Mode::ApplyDiff).
pub struct ApplyDiffView<'a> {
    pub target: &'a str,
    pub lines: &'a [DiffLine],
    pub scroll: usize,
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
    /// Agent session token totals: (prompt_total, completion_total, context_window).
    /// None when no agent session has been active yet.
    pub agent_session_tokens: Option<(u32, u32, u32)>,
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

// Buffer data tuple: (name, is_modified, cursor, scroll_row, scroll_col, lines, selection)
type BufferData = (String, bool, Cursor, usize, usize, Vec<String>, Option<Selection>);
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
    /// Apply-diff overlay data (Mode::ApplyDiff only).
    pub apply_diff: Option<&'a ApplyDiffView<'a>>,
    /// Inactive split pane buffer data; `None` = no split active.
    pub split_buffer_data: Option<&'a BufferData>,
    /// Pre-computed highlighted spans for the inactive split pane.
    pub split_highlighted_lines: Option<&'a [Vec<Span<'static>>]>,
    /// `true` when the right pane is the focused pane.
    pub split_right_focused: bool,
    /// Editable commit message buffer (Mode::CommitMsg only).
    pub commit_msg: Option<&'a str>,
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
}

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
        let apply_diff = ctx.apply_diff;
        let split_buffer_data = ctx.split_buffer_data;
        let split_highlighted_lines = ctx.split_highlighted_lines;
        let split_right_focused = ctx.split_right_focused;
        let commit_msg = ctx.commit_msg;
        let release_notes = ctx.release_notes;
        let diag_overlay = ctx.diag_overlay;
        let binary_file_path = ctx.binary_file_path;
        let startup_elapsed = ctx.startup_elapsed;
        let file_info = ctx.file_info;
        let location_list = ctx.location_list;
        let _in_file_search_query = ctx.in_file_search_query;

        let size = frame.area();

        if mode == Mode::ApplyDiff {
            if let Some(view) = apply_diff {
                Self::render_apply_diff_overlay(frame, view, size);
            }
            return;
        }

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

        // ── Horizontal splits: [explorer/tasks?] | [editor] | [agent?] ─────────────
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
                            Constraint::Percentage(35),
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
                        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
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
            );
        }

        // Render status line
        Self::render_status_line(
            frame,
            buffer_data,
            mode,
            status_message,
            command_buffer,
            key_sequence,
            status_area,
            diagnostics,
        );

        // Render agent panel if visible
        if let (Some(panel), Some(area)) = (agent_panel, agent_area) {
            Self::render_agent_panel(frame, panel, mode, area);
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
            Self::render_commit_msg_popup(frame, msg, size);
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

        // Render file-info popup if active (explorer `i` key)
        if let Some(info) = file_info {
            let explorer_right_edge = if explorer_visible { 25u16 } else { 0 };
            Self::render_file_info_popup(frame, info, size, explorer_right_edge);
        }
    }
}
