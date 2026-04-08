mod actions;
mod ai;
mod file_ops;
mod hooks;
mod input;
mod lsp;
mod mode_handlers;
mod pickers;
mod search;
use ai::strip_markdown_fence;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;

use crate::agent::AgentPanel;
use crate::buffer::Buffer;
use crate::config::Config;
use crate::explorer::FileExplorer;
use crate::highlight::Highlighter;
use crate::keymap::{Action, KeyHandler, Mode};
use crate::lsp::{parse_first_inline_completion, LspManager};
use crate::mcp::McpManager;
use crate::search::{SearchState, SearchStatus};
use crate::spec_framework;
use crate::ui::{RenderContext, UI};
use lsp_types::Diagnostic;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::text::Span;

/// Whether the clipboard was populated by a line-wise or char-wise operation.
/// Controls how `p`/`P` pastes the content.
#[derive(Clone)]
enum ClipboardType {
    /// Produced by `yy`/`dd`/`cc` — paste inserts whole new line(s).
    Linewise,
    /// Produced by `yw`/`y$`/visual-y etc — paste inserts inline at cursor.
    Charwise,
}

/// Cached syntax-highlight spans for the visible viewport.
///
/// The key is `(buffer_idx, scroll_row, lsp_version)`. When any of these change the
/// cache is stale and syntect is re-run; otherwise the spans are reused without touching
/// the highlighter at all.  A full re-highlight of 40 visible lines takes ~3–8 ms; with
/// the cache that cost drops to ~0 for all frames where the user is just moving the
/// cursor or reading.
struct HighlightCache {
    buffer_idx: usize,
    scroll_row: usize,
    lsp_version: i32,
    spans: Arc<Vec<Vec<ratatui::text::Span<'static>>>>,
}

/// Cached sticky-scroll context header.
///
/// Keyed on `(buffer_idx, scroll_row, lsp_version)` — the same staleness
/// signal used by `HighlightCache`.  Walking the tree-sitter CST on every
/// render frame is measurable (~0.5 ms/frame); this cache drops that to ~0
/// for the common case where the viewport does not move between frames.
struct StickyScrollCache {
    buffer_idx: usize,
    scroll_row: usize,
    lsp_version: i32,
    header: Option<String>,
}

/// Cached rendered markdown lines for Mode::MarkdownPreview.
/// Keyed on `(buffer_idx, lsp_version, viewport_width)` — regenerated only when
/// the active buffer changes, the content changes, or the terminal is resized.
struct MarkdownCache {
    buffer_idx: usize,
    lsp_version: i32,
    viewport_width: usize,
    lines: Vec<ratatui::text::Line<'static>>,
}

// ── LSP location list ─────────────────────────────────────────────────────────

/// A single navigable entry produced by goto-definition, find-references, or
/// document-symbols requests.
pub struct LocationEntry {
    /// Human-readable label shown in the list.
    pub label: String,
    /// Absolute path of the target file.
    pub file_path: std::path::PathBuf,
    /// 0-based target line.
    pub line: u32,
    /// 0-based target column.
    pub col: u32,
}

/// State for Mode::LocationList — a lightweight overlay listing LSP locations.
pub struct LocationListState {
    /// Title shown in the popup border.
    pub title: String,
    pub entries: Vec<LocationEntry>,
    pub selected: usize,
}

// ── Inline assistant (ADR 0111) ───────────────────────────────────────────────

/// Lifecycle phase of the inline assist overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineAssistPhase {
    /// User is typing their transformation directive.
    Input,
    /// LLM request is in-flight; tokens are accumulating.
    Generating,
    /// Response complete; waiting for user to accept or reject.
    Preview,
}

/// All state owned by `Editor` while `Mode::InlineAssist` is active.
/// Dropped when the user accepts or cancels.
pub struct InlineAssistState {
    /// Directive typed by the user during `Phase::Input`.
    pub prompt: String,
    /// Original selected text (empty when invoked without a selection).
    pub original_text: String,
    /// Buffer selection at the moment `InlineAssistStart` was fired.
    /// Used to locate and replace the text on accept.
    pub original_selection: Option<crate::buffer::Selection>,
    /// Buffer index the assist is targeting.
    pub target_buffer_idx: usize,
    /// File language hint derived from the buffer's extension (e.g. "Rust", "Python").
    /// Injected into the system prompt so the model knows what language to produce.
    pub language: Option<String>,
    /// LLM response accumulator.
    pub response: String,
    pub phase: InlineAssistPhase,
    /// Populated when the LLM request is launched (Input → Generating).
    pub stream_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::agent::StreamEvent>>,
    /// Kept alive to abort on cancel; dropped (fires abort) when `inline_assist` is set to None.
    #[allow(dead_code)]
    pub abort_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// State for Mode::LspHover — a scrollable popup showing hover documentation.
pub struct HoverPopupState {
    /// Hover text (plain text or Markdown).
    pub content: String,
    /// Vertical scroll offset in lines.
    pub scroll: u16,
}

// ── Multi-file review / change set view (ADR 0113) ───────────────────────────

/// A single line in a per-file unified diff.
pub enum DiffLine {
    /// Unchanged context line (shown dimmed).
    Context(String),
    /// Line added in the current version (shown green).
    Added(String),
    /// Line removed relative to the snapshot (shown red).
    Removed(String),
    /// Start of hunk `n` (0-indexed).  Replaces the old `HunkSep` and carries
    /// the hunk index so the renderer can look up per-hunk verdict.
    HunkStart(usize),
}

/// Whether the user has accepted or rejected a file's (or hunk's) changes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pending,
    Accepted,
    Rejected,
}

/// Diff data for one agent-modified file.
pub struct FileDiff {
    /// Project-relative path (key in `session_snapshots`).
    pub rel_path: String,
    /// Unified-diff lines with `HunkStart(idx)` separators.
    pub lines: Vec<DiffLine>,
    /// Per-hunk accept/reject decision.  Length equals the number of hunks.
    pub hunk_verdicts: Vec<Verdict>,
    /// File content before any agent edits (from `session_snapshots`, or `""` for new files).
    pub original: String,
    /// File content after all agent edits (read from disk when the overlay was opened).
    pub agent_version: String,
}

impl FileDiff {
    /// Derive the file-level verdict from the hunk verdicts.
    /// Accepted if all hunks accepted; Rejected if all rejected; Pending otherwise.
    pub fn file_verdict(&self) -> Verdict {
        if self.hunk_verdicts.is_empty() {
            return Verdict::Accepted;
        }
        let all_acc = self.hunk_verdicts.iter().all(|v| *v == Verdict::Accepted);
        let all_rej = self.hunk_verdicts.iter().all(|v| *v == Verdict::Rejected);
        if all_acc {
            Verdict::Accepted
        } else if all_rej {
            Verdict::Rejected
        } else {
            Verdict::Pending
        }
    }
}

/// All state for `Mode::ReviewChanges`.
pub struct ReviewChangesState {
    /// One entry per agent-touched file, sorted by path.
    pub diffs: Vec<FileDiff>,
    /// Vertical scroll offset into the flat rendered line list.
    pub scroll: usize,
    /// Index of the currently focused file (target for file-level `y` / `n`).
    pub focused_file: usize,
    /// Precomputed first flat-rendered-line index for each file's header.
    /// `file_offsets[i]` is the scroll offset that brings file `i` to the top.
    pub file_offsets: Vec<usize>,
    /// Focused hunk index within the focused file (`None` = no hunk focused).
    pub focused_hunk: Option<usize>,
    /// For each file, the flat line index of each `HunkStart` within that file's block.
    /// `hunk_line_offsets[file_idx][hunk_idx]` is the absolute flat line index.
    pub hunk_line_offsets: Vec<Vec<usize>>,
}

impl ReviewChangesState {
    /// Build from agent session state vs the current on-disk state.
    /// `created_paths` lists files newly created by the agent (original = "").
    pub fn build(
        snapshots: &std::collections::HashMap<String, String>,
        created_paths: &[String],
        project_root: &std::path::Path,
    ) -> Self {
        let mut diffs: Vec<FileDiff> = snapshots
            .iter()
            .map(|(rel_path, original)| {
                let abs = project_root.join(rel_path);
                let agent_version = std::fs::read_to_string(&abs).unwrap_or_default();
                let (lines, hunk_count) = review_diff_lines(original, &agent_version);
                let hunk_verdicts = vec![Verdict::Pending; hunk_count];
                FileDiff {
                    rel_path: rel_path.clone(),
                    lines,
                    hunk_verdicts,
                    original: original.clone(),
                    agent_version,
                }
            })
            .collect();

        // Add newly created files (original = empty string)
        for rel_path in created_paths {
            let abs = project_root.join(rel_path);
            let agent_version = std::fs::read_to_string(&abs).unwrap_or_default();
            let (lines, hunk_count) = review_diff_lines("", &agent_version);
            let hunk_verdicts = vec![Verdict::Pending; hunk_count];
            diffs.push(FileDiff {
                rel_path: rel_path.clone(),
                lines,
                hunk_verdicts,
                original: String::new(),
                agent_version,
            });
        }

        diffs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let (file_offsets, hunk_line_offsets) = review_compute_offsets(&diffs);
        Self {
            diffs,
            scroll: 0,
            focused_file: 0,
            file_offsets,
            focused_hunk: None,
            hunk_line_offsets,
        }
    }
}

/// Compute flat-line offsets for files and hunks within each file.
/// Returns `(file_offsets, hunk_line_offsets)`.
fn review_compute_offsets(diffs: &[FileDiff]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let mut file_offsets = Vec::with_capacity(diffs.len());
    let mut hunk_line_offsets: Vec<Vec<usize>> = Vec::with_capacity(diffs.len());
    let mut acc = 0usize;
    for d in diffs {
        file_offsets.push(acc);
        acc += 1; // file header line
        let mut hunks_in_file = Vec::new();
        for dl in &d.lines {
            if let DiffLine::HunkStart(_) = dl {
                hunks_in_file.push(acc);
            }
            acc += 1;
        }
        acc += 1; // blank spacer
        hunk_line_offsets.push(hunks_in_file);
    }
    (file_offsets, hunk_line_offsets)
}

/// Produce unified-diff lines (with 3-line context groups) via the `similar` crate.
/// Returns `(lines, hunk_count)`.
fn review_diff_lines(original: &str, current: &str) -> (Vec<DiffLine>, usize) {
    use similar::{ChangeTag, TextDiff};
    if original == current {
        return (vec![], 0);
    }
    let diff = TextDiff::from_lines(original, current);
    let mut out = Vec::new();
    let mut hunk_idx = 0usize;
    for group in diff.grouped_ops(3) {
        out.push(DiffLine::HunkStart(hunk_idx));
        for op in &group {
            for change in diff.iter_changes(op) {
                let line = change.value().trim_end_matches('\n').to_string();
                match change.tag() {
                    ChangeTag::Delete => out.push(DiffLine::Removed(line)),
                    ChangeTag::Insert => out.push(DiffLine::Added(line)),
                    ChangeTag::Equal => out.push(DiffLine::Context(line)),
                }
            }
        }
        hunk_idx += 1;
    }
    (out, hunk_idx)
}

/// Reconstruct file content by selectively reverting rejected hunks.
///
/// For each hunk: if `Rejected`, use the original lines; otherwise use the
/// agent's version.  Lines outside any hunk group (far context) are taken from
/// the agent version unchanged.
pub(crate) fn apply_hunk_verdicts(
    original: &str,
    agent_version: &str,
    verdicts: &[Verdict],
) -> String {
    use similar::TextDiff;

    if verdicts.iter().all(|v| *v != Verdict::Rejected) {
        return agent_version.to_string();
    }
    if verdicts.iter().all(|v| *v == Verdict::Rejected) {
        return original.to_string();
    }

    let diff = TextDiff::from_lines(original, agent_version);
    let groups = diff.grouped_ops(3);

    let orig_lines: Vec<&str> = original.split_inclusive('\n').collect();
    let curr_lines: Vec<&str> = agent_version.split_inclusive('\n').collect();

    let mut out = String::new();
    let mut orig_consumed = 0usize;

    for (hunk_idx, group) in groups.iter().enumerate() {
        let group_old_start = group[0].old_range().start;
        // Emit "far context" lines between previous group and this one (identical in both versions)
        for i in orig_consumed..group_old_start {
            if let Some(l) = orig_lines.get(i) {
                out.push_str(l);
            }
        }

        let rejected = matches!(verdicts.get(hunk_idx), Some(Verdict::Rejected));
        let group_old_end = group.last().unwrap().old_range().end;

        if rejected {
            // Revert: emit original lines for this hunk's old range
            for i in group_old_start..group_old_end {
                if let Some(l) = orig_lines.get(i) {
                    out.push_str(l);
                }
            }
        } else {
            // Accept: emit current (agent) lines for this hunk's new range
            let group_new_start = group[0].new_range().start;
            let group_new_end = group.last().unwrap().new_range().end;
            for i in group_new_start..group_new_end {
                if let Some(l) = curr_lines.get(i) {
                    out.push_str(l);
                }
            }
        }

        orig_consumed = group_old_end;
    }

    // Emit remaining original lines after all groups
    for i in orig_consumed..orig_lines.len() {
        if let Some(l) = orig_lines.get(i) {
            out.push_str(l);
        }
    }

    out
}

// ── Mode-specific sub-states ──────────────────────────────────────────────────
// Each struct owns all fields that are active only during a single Mode variant.
// Grouping them prevents the top-level Editor struct from growing unboundedly as
// new modes are added.

/// State for the vertical split pane (Mode::Normal with an active split).
#[derive(Default)]
struct SplitState {
    /// Index of the background pane's buffer; `None` = no split active.
    other_idx: Option<usize>,
    /// `true` when the right pane has focus.
    right_focused: bool,
    /// Per-viewport highlight cache for the inactive (background) pane.
    highlight_cache: Option<HighlightCache>,
}

/// State for the commit message generation popup (Mode::CommitMsg).
#[derive(Default)]
struct CommitMsgState {
    /// Editable commit message buffer.
    buffer: String,
    /// In-flight AI generation task.
    rx: Option<oneshot::Receiver<anyhow::Result<String>>>,
    /// `true` = generated from staged diff (`SPC g s`); `false` = last commit (`SPC g l`).
    from_staged: bool,
}

/// State for the release notes generation popup (Mode::ReleaseNotes).
#[derive(Default)]
struct ReleaseNotesState {
    /// Commit count input string (count-entry phase).
    count_input: String,
    /// In-flight AI generation task.
    rx: Option<oneshot::Receiver<anyhow::Result<String>>>,
    /// Completed release notes text (display phase).
    buffer: String,
    /// Scroll offset for the display popup.
    scroll: u16,
}

/// The Editor manages the overall application state: buffers, current buffer, mode, etc.
pub struct Editor {
    /// All open buffers
    buffers: Vec<Buffer>,

    /// Index of the currently active buffer
    current_buffer_idx: usize,

    /// Current editing mode (Normal, Insert, Command, Visual, PickBuffer)
    mode: Mode,

    /// Command buffer for command mode (when user types :w, :q, etc.)
    command_buffer: String,

    /// Key handler for processing input
    key_handler: KeyHandler,

    /// Terminal backend
    terminal: Terminal<CrosstermBackend<io::Stdout>>,

    /// Whether the editor should quit
    should_quit: bool,

    /// Status message to display (for feedback)
    status_message: Option<String>,

    /// Currently selected buffer in PickBuffer mode
    buffer_picker_idx: usize,

    /// Currently selected file in PickFile mode
    file_picker_idx: usize,

    /// Full file list populated by scan_files() — never filtered.
    file_all: Vec<PathBuf>,

    /// Live search query typed in PickFile mode.
    file_query: String,

    /// Fuzzy-filtered results: (path, match-char indices in the display string).
    /// Recomputed whenever file_query or file_all changes.
    file_list: Vec<(PathBuf, Vec<usize>)>,

    /// Most-recently-opened files, most recent first. Capped at 5. Persisted across sessions.
    recent_files: Vec<PathBuf>,

    /// LSP manager for language server protocol support
    lsp_manager: LspManager,

    /// Diagnostics for the current buffer
    current_diagnostics: Vec<Diagnostic>,

    // ── Inline completion / ghost text ────────────────────────────────────────
    /// Current ghost text suggestion and the buffer position it belongs to.
    /// Format: (text, row, col)
    ghost_text: Option<(String, usize, usize)>,

    /// In-flight inline completion request; polled non-blocking each frame.
    pending_completion: Option<oneshot::Receiver<serde_json::Value>>,

    /// Timestamp of the last buffer edit, used to debounce completion requests.
    last_edit_instant: Option<Instant>,

    // ── Copilot auth ──────────────────────────────────────────────────────────
    /// In-flight Copilot auth request (checkStatus or signInInitiate).
    copilot_auth_rx: Option<oneshot::Receiver<serde_json::Value>>,

    /// When true the status message persists across keypresses until explicitly
    /// cleared (used for Copilot device-auth URLs which the user needs to read).
    status_sticky: bool,

    // ── Agent / Copilot Chat panel ────────────────────────────────────────────
    agent_panel: AgentPanel,
    /// Timestamp of the last frame triggered exclusively by agent streaming.
    /// Used to cap agent-only renders to ≤10 Hz (100 ms between frames) so a
    /// long-running janitor does not spin the render loop at the full 20 Hz
    /// event-poll rate.
    last_agent_render: Option<std::time::Instant>,

    // ── Clipboard (yank register) ─────────────────────────────────────────────
    /// Last yanked / deleted text + whether it is linewise or charwise.
    clipboard: Option<(String, ClipboardType)>,

    // ── Syntax highlighter ────────────────────────────────────────────────────
    /// Loaded once at startup; highlight_line() is called per visible line each frame.
    highlighter: Highlighter,

    /// Per-viewport highlight cache — invalidated on content change or scroll.
    highlight_cache: Option<HighlightCache>,

    // ── Visual mode text object state ─────────────────────────────────────────
    /// Pending `i`/`a` prefix for tree-sitter text object selection in Visual mode.
    /// Set when `i` or `a` is pressed in Visual mode; consumed on the next key.
    visual_text_obj_prefix: Option<char>,

    // ── Surround operations (ADR 0110) ────────────────────────────────────────
    /// The `from` char stored between `cs{from}` and `{to}` keypresses.
    surround_change_from: Option<char>,

    // ── Inline assistant (ADR 0111) ───────────────────────────────────────────
    /// Active only while `mode == Mode::InlineAssist`.
    inline_assist: Option<InlineAssistState>,

    // ── Multi-file review / change set view (ADR 0113) ───────────────────────
    /// Active only while `mode == Mode::ReviewChanges`.
    pub review_changes: Option<ReviewChangesState>,

    // ── Agent hooks (ADR 0114) ────────────────────────────────────────────────
    /// Per-hook cooldown tracking: `hook_index → last_fired`.
    /// Prevents the same hook from firing more than once per 5 seconds.
    hook_cooldowns: std::collections::HashMap<usize, std::time::Instant>,
    /// Result of the most recent test run: `true` = passing, `false` = failing.
    /// `None` until the first test run completes.  Used by `on_test_fail` hooks
    /// to detect pass→fail transitions (repeated failures do not re-fire the hook).
    last_test_passed: Option<bool>,
    /// Set to `true` while an agent hook is running to prevent re-entrant test
    /// runs that would loop (agent fixes → save → tests → agent fires again).
    hooks_firing: bool,

    // ── Tree-sitter AST cache ─────────────────────────────────────────────────
    /// Wraps the Tree-sitter `Parser`; shared across all buffers (language is
    /// reset before each parse).
    ts_engine: crate::treesitter::TsEngine,
    /// Most recent parse result per buffer index.  Keyed by `buffer_idx`.
    ts_cache: std::collections::HashMap<usize, crate::treesitter::TsSnapshot>,
    /// `lsp_version` at the time each cached tree was last parsed.
    /// When `buffer.lsp_version != ts_versions[idx]` the tree is stale.
    ts_versions: std::collections::HashMap<usize, i32>,

    // ── Code folding (ADR 0106) ───────────────────────────────────────────────
    /// Per-buffer set of fold start rows that are currently closed.
    /// Keyed by buffer index; the value is the set of fold-region start rows
    /// for which the fold is collapsed.
    fold_closed: std::collections::HashMap<usize, std::collections::HashSet<usize>>,

    // ── File explorer ─────────────────────────────────────────────────────────
    file_explorer: FileExplorer,

    // ── Markdown preview ──────────────────────────────────────────────────────
    /// Scroll offset (in rendered lines) for Mode::MarkdownPreview.
    preview_scroll: usize,
    /// Cached rendered markdown lines — avoids re-parsing on every render frame.
    markdown_cache: Option<MarkdownCache>,

    /// Cached sticky-scroll header — avoids walking the tree-sitter CST every frame.
    sticky_scroll_cache: Option<StickyScrollCache>,

    // ── Project-wide text search ──────────────────────────────────────────────
    /// State for the search overlay (Mode::Search).
    search_state: SearchState,
    /// In-flight ripgrep task receiver; `Some` while a search is running.
    search_rx: Option<oneshot::Receiver<anyhow::Result<Vec<crate::search::SearchResult>>>>,
    /// Timestamp of the last query/glob change — drives the 300 ms debounce.
    last_search_instant: Option<Instant>,

    // ── In-file search ────────────────────────────────────────────────────────
    /// Text typed so far while in Mode::InFileSearch (the `/` prompt).
    in_file_search_buffer: String,

    // ── Explorer rename popup ─────────────────────────────────────────────────
    /// Filename being edited while in Mode::RenameFile.
    rename_buffer: String,
    /// Absolute path of the entry being renamed.
    rename_source: Option<std::path::PathBuf>,

    // ── Explorer delete confirmation popup ────────────────────────────────────
    /// Path of the entry pending deletion (Mode::DeleteFile).
    delete_confirm_path: Option<std::path::PathBuf>,

    // ── Binary / unsupported file popup ───────────────────────────────────────
    /// Path of a binary file that cannot be opened as text (Mode::BinaryFile).
    pub binary_file_path: Option<std::path::PathBuf>,

    // ── Explorer new folder popup ─────────────────────────────────────────────
    /// Folder name being typed while in Mode::NewFolder.
    new_folder_buffer: String,
    /// Parent directory in which the new folder will be created.
    new_folder_parent: Option<std::path::PathBuf>,

    // ── Explorer file-info overlay ────────────────────────────────────────────
    /// When `true` a file-info popup is shown for the currently selected entry.
    /// Toggled by `i` in Mode::Explorer; cleared when focus leaves the explorer.
    show_file_info: bool,

    // ── Vertical split ────────────────────────────────────────────────────────
    split: SplitState,

    // ── Commit message generation (Mode::CommitMsg) ───────────────────────────
    commit_msg: CommitMsgState,

    // ── Release notes generation (Mode::ReleaseNotes) ─────────────────────────
    release_notes: ReleaseNotesState,

    // ── MCP servers ───────────────────────────────────────────────────────────
    /// Manages connected MCP servers and their tool registries.
    /// Set once the background connection task completes (see `mcp_rx`).
    mcp_manager: Option<std::sync::Arc<McpManager>>,
    /// Receives the completed `McpManager` from the background startup task.
    /// Polled each tick; cleared and wired into `agent_panel` on first `Ok`.
    mcp_rx: Option<oneshot::Receiver<McpManager>>,

    // ── LSP navigation ────────────────────────────────────────────────────────
    /// In-flight goto-definition request; polled non-blocking each frame.
    pending_goto_definition: Option<oneshot::Receiver<serde_json::Value>>,
    /// In-flight find-references request; polled non-blocking each frame.
    pending_references: Option<oneshot::Receiver<serde_json::Value>>,
    /// In-flight document-symbols request; polled non-blocking each frame.
    pending_symbols: Option<oneshot::Receiver<serde_json::Value>>,
    /// State for the location list overlay (Mode::LocationList).
    pub location_list: Option<LocationListState>,
    /// In-flight hover request; polled non-blocking each frame.
    pending_hover: Option<oneshot::Receiver<serde_json::Value>>,
    /// Hover popup state (Mode::LspHover).
    pub hover_popup: Option<HoverPopupState>,
    /// Text typed into the LSP rename input popup (Mode::LspRename).
    pub lsp_rename_buffer: String,
    /// URI + position of the symbol being renamed; set when entering Mode::LspRename.
    lsp_rename_origin: Option<(lsp_types::Uri, lsp_types::Position)>,
    /// In-flight rename request; polled non-blocking each frame.
    pending_rename: Option<oneshot::Receiver<serde_json::Value>>,

    // ── Filesystem watcher ────────────────────────────────────────────────────
    /// Watches paths of all open buffers; detects external changes.
    file_watcher: Option<RecommendedWatcher>,
    /// Receives raw notify events; polled each tick.
    watcher_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>,
    /// Paths written by the editor itself, with the save timestamp.
    /// Watcher events for these paths are suppressed for 500 ms to avoid
    /// treating our own saves as external changes.
    self_saved: std::collections::HashMap<std::path::PathBuf, std::time::Instant>,

    // ── In-memory log ring buffer ─────────────────────────────────────────────
    /// Recent WARN/ERROR log entries captured from the tracing subscriber.
    /// Shared with the tracing layer via Arc<Mutex<...>>.
    pub log_buffer: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<(String, String)>>>,

    // ── Startup timing ────────────────────────────────────────────────────────
    /// Time from process start to the editor being fully ready (LSP + MCP set up).
    /// Set by main() after setup completes; displayed on the welcome screen.
    pub startup_elapsed: Option<std::time::Duration>,

    // ── Configuration ─────────────────────────────────────────────────────────
    /// Editor configuration (LSP servers, tab width, Copilot defaults, etc.)
    config: Config,
}

impl Editor {
    pub fn new(config: Config) -> Result<Self> {
        // Set up terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut editor = Self {
            buffers: Vec::new(),
            current_buffer_idx: 0,
            mode: Mode::Normal,
            command_buffer: String::new(),
            key_handler: KeyHandler::new(),
            terminal,
            should_quit: false,
            status_message: None,
            buffer_picker_idx: 0,
            file_picker_idx: 0,
            file_all: Vec::new(),
            file_query: String::new(),
            file_list: Vec::new(),
            recent_files: Self::load_recents(),
            lsp_manager: LspManager::new(),
            current_diagnostics: Vec::new(),
            ghost_text: None,
            pending_completion: None,
            last_edit_instant: None,
            copilot_auth_rx: None,
            status_sticky: false,
            agent_panel: {
                let mut panel = AgentPanel::new();
                panel.spec_framework =
                    spec_framework::load_from_config(&config.agent.spec_framework);
                panel.provider = crate::agent::ProviderKind::from_str(&config.provider.active);
                panel.ollama_base_url = config.provider.ollama.base_url.clone();
                panel.ollama_context_length = config.provider.ollama.context_length;
                panel.ollama_tool_calls = config.provider.ollama.tool_calls;
                panel.ollama_planning_tools = config.provider.ollama.planning_tools;
                // Resolve API keys for direct-API providers ($VAR expansion).
                panel.api_key = match panel.provider {
                    crate::agent::ProviderKind::Anthropic => {
                        crate::agent::provider::resolve_api_key(&config.provider.anthropic.api_key)
                    },
                    crate::agent::ProviderKind::OpenAi => {
                        crate::agent::provider::resolve_api_key(&config.provider.openai.api_key)
                    },
                    crate::agent::ProviderKind::Gemini => {
                        crate::agent::provider::resolve_api_key(&config.provider.gemini.api_key)
                    },
                    crate::agent::ProviderKind::OpenRouter => {
                        crate::agent::provider::resolve_api_key(&config.provider.openrouter.api_key)
                    },
                    _ => String::new(),
                };
                if let Some(ref base) = config.provider.openai.base_url {
                    panel.openai_base_url = base.clone();
                }
                panel.openrouter_site_url = config.provider.openrouter.site_url.clone();
                panel.openrouter_app_name = config.provider.openrouter.app_name.clone();
                panel
            },
            clipboard: None::<(String, ClipboardType)>,
            highlighter: Highlighter::new(),
            highlight_cache: None,
            visual_text_obj_prefix: None,
            surround_change_from: None,
            inline_assist: None,
            review_changes: None,
            hook_cooldowns: std::collections::HashMap::new(),
            last_test_passed: None,
            hooks_firing: false,
            ts_engine: crate::treesitter::TsEngine::new(),
            ts_cache: std::collections::HashMap::new(),
            ts_versions: std::collections::HashMap::new(),
            fold_closed: std::collections::HashMap::new(),
            file_explorer: FileExplorer::new(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            ),
            preview_scroll: 0,
            markdown_cache: None,
            sticky_scroll_cache: None,
            last_agent_render: None,
            search_state: SearchState::new(),
            search_rx: None,
            last_search_instant: None,
            in_file_search_buffer: String::new(),
            rename_buffer: String::new(),
            rename_source: None,
            delete_confirm_path: None,
            binary_file_path: None,
            new_folder_buffer: String::new(),
            new_folder_parent: None,
            show_file_info: false,
            split: SplitState::default(),
            commit_msg: CommitMsgState { from_staged: true, ..Default::default() },
            release_notes: ReleaseNotesState {
                count_input: String::from("10"),
                ..Default::default()
            },
            mcp_manager: None,
            mcp_rx: None,
            pending_goto_definition: None,
            pending_references: None,
            pending_symbols: None,
            location_list: None,
            pending_hover: None,
            hover_popup: None,
            lsp_rename_buffer: String::new(),
            lsp_rename_origin: None,
            pending_rename: None,
            file_watcher: None,
            watcher_rx: None,
            self_saved: std::collections::HashMap::new(),
            log_buffer: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            startup_elapsed: None,
            config,
        };

        // Spin up the filesystem watcher (best-effort; degrades gracefully).
        let (tx, rx) = std::sync::mpsc::channel();
        match notify::recommended_watcher(tx) {
            Ok(w) => {
                editor.file_watcher = Some(w);
                editor.watcher_rx = Some(rx);
            },
            Err(e) => {
                tracing::warn!("Filesystem watcher unavailable: {e}");
            },
        }

        Ok(editor)
    }

    /// Render a loading frame while async setup (LSP / MCP) is in progress.
    /// The terminal is already in alternate-screen mode at this point.
    pub fn render_loading(&mut self, msg: &str) -> Result<()> {
        use ratatui::{
            style::{Color, Modifier, Style},
            text::{Line, Span},
            widgets::Paragraph,
        };
        #[rustfmt::skip]
        const CROSS: &[&str] = &[
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "           ━━━━━━━━━━━━━━━━━━━━╋╋╋━━━━━━━━━━━━━━━━━━━━",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
            "                               ┃┃┃",
        ];
        #[rustfmt::skip]
        const WORDMARK: &[&str] = &[
            "███████╗ ██████╗ ██████╗  ██████╗ ██╗██╗   ██╗███████╗███╗   ██╗",
            "██╔════╝██╔═══██╗██╔══██╗██╔════╝ ██║██║   ██║██╔════╝████╗  ██║",
            "█████╗  ██║   ██║██████╔╝██║  ███╗██║██║   ██║█████╗  ██╔██╗ ██║",
            "██╔══╝  ██║   ██║██╔══██╗██║   ██║██║╚██╗ ██╔╝██╔══╝  ██║╚██╗██║",
            "██║     ╚██████╔╝██║  ██║╚██████╔╝██║ ╚████╔╝ ███████╗██║ ╚████║",
            "╚═╝      ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝  ╚══════╝╚═╝  ╚═══╝",
        ];
        const LOGO_W: usize = 64;

        let msg = msg.to_owned();
        self.terminal.draw(|frame| {
            let area = frame.area();
            let area_h = area.height as usize;
            let area_w = area.width as usize;

            // cross + blank + wordmark + blank + msg
            let logo_h = CROSS.len() + 1 + WORDMARK.len() + 1 + 1;
            let top_pad = area_h.saturating_sub(logo_h) / 2;
            let left_pad = area_w.saturating_sub(LOGO_W) / 2;

            let cross_style = Style::default().fg(Color::Yellow);
            let word_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
            let loading_style = Style::default().fg(Color::DarkGray);

            let mut lines: Vec<Line> = (0..top_pad).map(|_| Line::from("")).collect();
            for s in CROSS {
                lines.push(Line::from(Span::styled(
                    format!("{}{}", " ".repeat(left_pad), *s),
                    cross_style,
                )));
            }
            lines.push(Line::from(""));
            for s in WORDMARK {
                lines.push(Line::from(Span::styled(
                    format!("{}{}", " ".repeat(left_pad), *s),
                    word_style,
                )));
            }
            lines.push(Line::from(""));
            let msg_pad = area_w.saturating_sub(msg.len()) / 2;
            lines.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(msg_pad), msg),
                loading_style,
            )));

            frame.render_widget(Paragraph::new(lines), area);
        })?;
        Ok(())
    }

    /// Open a file into a new buffer.
    /// Creates an empty buffer for non-existent files (new file workflow).
    /// Returns Ok(()) for unsupported binary files, displaying a status message instead of crashing.
    pub fn open_file(&mut self, path: &std::path::Path) -> Result<()> {
        // Binary-file guard — probe first 8 KB for null bytes.
        if path.exists() {
            use std::io::Read as _;
            let mut probe = [0u8; 8192];
            if let Ok(mut f) = std::fs::File::open(path) {
                let n = f.read(&mut probe).unwrap_or(0);
                if probe[..n].contains(&0u8) {
                    self.binary_file_path = Some(path.to_path_buf());
                    self.mode = Mode::BinaryFile;
                    return Ok(());
                }
            }
        }

        let buffer = if path.exists() {
            match Buffer::from_file(path.to_path_buf()) {
                Ok(buf) => buf,
                Err(e) => {
                    self.set_status(format!(
                        "Cannot open '{}': {}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        e
                    ));
                    return Ok(());
                },
            }
        } else {
            // New file — create an empty named buffer
            let mut buf = Buffer::new(path.to_string_lossy().as_ref());
            buf.file_path = Some(path.to_path_buf());
            buf
        };
        self.buffers.push(buffer);
        self.current_buffer_idx = self.buffers.len() - 1;
        self.set_status(format!("Opened {}", path.display()));

        // Track in recents using the canonical absolute path for deduplication.
        if let Ok(abs) = path.canonicalize() {
            self.recent_files.retain(|p| *p != abs);
            self.recent_files.insert(0, abs);
            self.recent_files.truncate(5);
            let _ = self.save_recents();
        }

        // Notify LSP about opened document if a server is running for this language.
        let language = LspManager::language_from_path(path);
        let text = self.current_buffer().map(|b| b.lines().join("\n")).unwrap_or_default();

        if let Ok(uri) = LspManager::path_to_uri(path) {
            if let Some(client) = self.lsp_manager.get_client(&language) {
                let _ = client.did_open(uri, language.clone(), text);
            }
        }

        // Register with the filesystem watcher so external changes are detected.
        if let Some(ref mut watcher) = self.file_watcher {
            if let Some(ref buf_path) = self.buffers.last().and_then(|b| b.file_path.clone()) {
                let _ = watcher.watch(buf_path, RecursiveMode::NonRecursive);
            }
        }

        Ok(())
    }

    /// Start all LSP servers and MCP servers concurrently, then apply the results.
    ///
    /// LSP startup blocks the loading screen (the editor needs completions and
    /// diagnostics to be useful).  MCP startup is fire-and-forget: a background
    /// task is spawned immediately and the result is wired in via `mcp_rx` once
    /// the connections complete — the editor opens without waiting for MCP.
    pub async fn setup_services(&mut self) {
        let workspace_root =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let lsp_servers = self.config.lsp.servers.clone();
        let mcp_servers = self.config.mcp.servers.clone();
        let notif_tx = self.lsp_manager.notification_tx();

        // ── LSP — filter to workspace-relevant servers, then await ────────────
        let lsp_servers = crate::lsp::filter_servers_for_workspace(&lsp_servers, &workspace_root);
        tracing::info!("Starting {} LSP server(s) for this workspace", lsp_servers.len());
        let lsp_results =
            crate::lsp::init_servers_parallel(&lsp_servers, workspace_root, notif_tx).await;

        for (language, result) in lsp_results {
            match result {
                Err(e) => {
                    let msg = format!("LSP '{}': {e}", language);
                    tracing::warn!("{}", msg);
                    self.set_status(msg);
                },
                Ok(client) => {
                    self.lsp_manager.insert_client(language.clone(), client);
                    if language == "copilot" {
                        if let Some(c) = self.lsp_manager.get_client("copilot") {
                            match c.copilot_check_status() {
                                Ok(rx) => self.copilot_auth_rx = Some(rx),
                                Err(e) => tracing::warn!("copilot checkStatus failed: {e}"),
                            }
                        }
                    }
                },
            }
        }

        // Send did_open for any files that were opened before LSP was ready.
        let notifications: Vec<_> = self
            .buffers
            .iter()
            .filter_map(|buf| {
                let path = buf.file_path.as_ref()?;
                let language = LspManager::language_from_path(path);
                let uri = LspManager::path_to_uri(path).ok()?;
                let text = buf.lines().join("\n");
                Some((language, uri, text))
            })
            .collect();
        for (language, uri, text) in notifications {
            if let Some(client) = self.lsp_manager.get_client(&language) {
                let _ = client.did_open(uri, language, text);
            }
        }

        // ── MCP — fire-and-forget background task ─────────────────────────────
        // The editor opens immediately; MCP tools become available once the
        // background handshakes complete.  Progress is visible in the agent
        // panel bottom bar (ADR 0048) and the diagnostics overlay (SPC d).
        if !mcp_servers.is_empty() {
            tracing::info!("Spawning {} MCP server(s) in background", mcp_servers.len());
            let (tx, rx) = oneshot::channel();
            tokio::spawn(async move {
                let manager = McpManager::from_config(&mcp_servers).await;
                let _ = tx.send(manager);
            });
            self.mcp_rx = Some(rx);
        }
    }

    /// Get the currently active buffer
    pub fn current_buffer(&self) -> Option<&Buffer> {
        self.buffers.get(self.current_buffer_idx)
    }

    /// Get mutable reference to current buffer
    pub fn current_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffers.get_mut(self.current_buffer_idx)
    }

    /// Return the Tree-sitter parse snapshot for the current buffer, parsing or
    /// re-parsing lazily if the cached version is stale.
    ///
    /// Returns `None` when no buffer is open, the file has an unknown extension,
    /// or Tree-sitter parsing fails (grammar ABI mismatch). All callers must
    /// handle `None` — Tree-sitter features degrade gracefully for unsupported files.
    pub(crate) fn ts_tree_for_current_buffer(&mut self) -> Option<&crate::treesitter::TsSnapshot> {
        let idx = self.current_buffer_idx;
        let buf = self.buffers.get(idx)?;
        let path = buf.file_path.as_deref()?;
        let lang = crate::treesitter::TsEngine::detect(path)?;
        let current_version = buf.lsp_version;

        // Cache hit: the stored version matches the buffer's current version.
        if self.ts_versions.get(&idx) == Some(&current_version) {
            return self.ts_cache.get(&idx);
        }

        // Cache miss: re-parse from the buffer's current content.
        let source = buf.lines().join("\n");
        let snap = self.ts_engine.parse(&source, lang)?;
        self.ts_cache.insert(idx, snap);
        self.ts_versions.insert(idx, current_version);
        self.ts_cache.get(&idx)
    }

    // ── Code folding helpers (ADR 0106) ──────────────────────────────────────

    /// Toggle the fold at the cursor position.
    ///
    /// Finds the innermost foldable region (function or class node) that
    /// contains the cursor row and toggles its collapsed state.  When the
    /// cursor is inside a fold body (not on the start row), it is moved to the
    /// fold start row so it remains visible.
    pub(crate) fn fold_toggle(&mut self) {
        let buf_idx = self.current_buffer_idx;
        // Ensure tree is parsed.
        let _ = self.ts_tree_for_current_buffer();

        let cursor_row = match self.current_buffer() {
            Some(b) => b.cursor.row,
            None => return,
        };

        let fold_ranges = self
            .ts_cache
            .get(&buf_idx)
            .map(crate::treesitter::query::fold_ranges)
            .unwrap_or_default();

        if fold_ranges.is_empty() {
            self.set_status(
                "No foldable region (tree-sitter not available for this file type)".to_string(),
            );
            return;
        }

        // Find the innermost range whose [start, end] spans the cursor row.
        let target = fold_ranges
            .iter()
            .filter(|&&(s, e)| cursor_row >= s && cursor_row <= e)
            .min_by_key(|&&(s, e)| e - s); // innermost = smallest span

        if let Some(&(start, _)) = target {
            let closed = self.fold_closed.entry(buf_idx).or_default();
            if closed.contains(&start) {
                closed.remove(&start);
            } else {
                closed.insert(start);
                // Move cursor to fold start if it was inside the fold body.
                if cursor_row != start {
                    if let Some(buf) = self.current_buffer_mut() {
                        buf.cursor.row = start;
                        let col = buf.cursor.col;
                        buf.move_to_col(col);
                    }
                }
            }
        } else {
            self.set_status("No foldable region at cursor".to_string());
        }
    }

    /// Close all folds in the current buffer.
    pub(crate) fn fold_close_all(&mut self) {
        let buf_idx = self.current_buffer_idx;
        let _ = self.ts_tree_for_current_buffer();

        let fold_ranges = self
            .ts_cache
            .get(&buf_idx)
            .map(crate::treesitter::query::fold_ranges)
            .unwrap_or_default();

        if fold_ranges.is_empty() {
            self.set_status("No foldable regions found".to_string());
            return;
        }

        let count = fold_ranges.len();
        let closed = self.fold_closed.entry(buf_idx).or_default();
        for (start, _) in fold_ranges {
            closed.insert(start);
        }
        // Move cursor to the start of its fold if it ended up hidden.
        let cursor_row = self.current_buffer().map(|b| b.cursor.row).unwrap_or(0);
        let buf_idx = self.current_buffer_idx;
        let hidden = {
            let closed = self.fold_closed.get(&buf_idx).cloned().unwrap_or_default();
            let ranges = self
                .ts_cache
                .get(&buf_idx)
                .map(crate::treesitter::query::fold_ranges)
                .unwrap_or_default();
            let mut h = std::collections::HashSet::new();
            for (s, e) in &ranges {
                if closed.contains(s) {
                    for r in (s + 1)..=*e {
                        h.insert(r);
                    }
                }
            }
            h
        };
        if hidden.contains(&cursor_row) {
            // Find the enclosing fold start and move cursor there.
            let closed = self.fold_closed.get(&buf_idx).cloned().unwrap_or_default();
            let ranges = self
                .ts_cache
                .get(&buf_idx)
                .map(crate::treesitter::query::fold_ranges)
                .unwrap_or_default();
            if let Some(&(start, _)) = ranges
                .iter()
                .filter(|&&(s, e)| closed.contains(&s) && cursor_row > s && cursor_row <= e)
                .min_by_key(|&&(s, e)| e - s)
            {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.cursor.row = start;
                    let col = buf.cursor.col;
                    buf.move_to_col(col);
                }
            }
        }
        self.set_status(format!("{count} fold{} closed", if count == 1 { "" } else { "s" }));
    }

    /// Open all folds in the current buffer.
    pub(crate) fn fold_open_all(&mut self) {
        let buf_idx = self.current_buffer_idx;
        let count = self.fold_closed.get(&buf_idx).map(|s| s.len()).unwrap_or(0);
        self.fold_closed.remove(&buf_idx);
        if count > 0 {
            self.set_status(format!("{count} fold{} opened", if count == 1 { "" } else { "s" }));
        }
    }

    // ── Inline assistant (ADR 0111) ───────────────────────────────────────────

    /// Poll the inline assist stream for new tokens.
    /// Called once per frame from the run loop, alongside `agent_panel.poll_stream()`.
    /// Returns `true` when the frame should be re-rendered.
    pub(super) fn poll_inline_assist(&mut self) -> bool {
        use crate::agent::StreamEvent;
        const MAX_TOKENS_PER_FRAME: usize = 64;

        // Only active during the Generating phase.
        if !matches!(
            self.inline_assist.as_ref().map(|s| s.phase),
            Some(InlineAssistPhase::Generating)
        ) {
            return false;
        }

        let mut active = false;
        let mut token_count = 0usize;
        let mut error: Option<String> = None;

        if let Some(state) = self.inline_assist.as_mut() {
            if let Some(rx) = state.stream_rx.as_mut() {
                loop {
                    match rx.try_recv() {
                        Ok(StreamEvent::Token(t)) => {
                            active = true;
                            state.response.push_str(&t);
                            token_count += 1;
                            if token_count >= MAX_TOKENS_PER_FRAME {
                                break;
                            }
                        },
                        Ok(StreamEvent::Done) => {
                            active = true;
                            // Strip any wrapping code fence the LLM may have added.
                            state.response = strip_assist_fence(&state.response);
                            state.phase = InlineAssistPhase::Preview;
                            break;
                        },
                        Ok(StreamEvent::Error(e)) => {
                            active = true;
                            error = Some(e);
                            break;
                        },
                        // Ignore tool / file / task events — inline assist has no tools.
                        Ok(_) => {
                            active = true;
                        },
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            // Stream ended without explicit Done.
                            active = true;
                            state.response = strip_assist_fence(&state.response);
                            state.phase = InlineAssistPhase::Preview;
                            break;
                        },
                    }
                }
            }
        }

        if let Some(e) = error {
            self.set_status(format!("Inline assist error: {e}"));
            self.inline_assist = None;
            self.mode = Mode::Normal;
        }

        active
    }

    /// Apply a mutating closure to the current buffer, returning `Some(T)` on
    /// success or `None` when no buffer is open. Prefer this over the raw
    /// `if let Some(buf) = self.current_buffer_mut()` pattern so that future
    /// additions stay uniform and the nesting depth stays flat.
    #[inline]
    fn with_buffer<T, F: FnOnce(&mut Buffer) -> T>(&mut self, f: F) -> Option<T> {
        self.current_buffer_mut().map(f)
    }

    /// Main event loop
    pub async fn run(&mut self) -> Result<()> {
        const COMPLETION_DEBOUNCE_MS: u128 = 300;

        // Render on the very first frame regardless of activity.
        let mut needs_render = true;
        // Set to true whenever the terminal cell grid may be stale (resize, SIGCONT, Ctrl+L).
        // A full terminal clear is issued before the next render to force a repaint.
        let mut force_clear = false;

        // ── SIGCONT: laptop-lid-open / process-resume repaint ─────────────────
        // When the OS suspends and resumes a process it sends SIGCONT.  The
        // terminal has already forgotten our screen contents, so we must clear
        // and repaint everything.  Tokio's signal module is already available
        // (tokio full feature); no extra dependency is needed.
        #[cfg(unix)]
        let (sigcont_tx, mut sigcont_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        #[cfg(unix)]
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            // SIGCONT = 18 on Linux and macOS.
            if let Ok(mut sig) = signal(SignalKind::from_raw(18)) {
                loop {
                    sig.recv().await;
                    if sigcont_tx.send(()).is_err() {
                        break;
                    }
                }
            }
        });

        loop {
            // ── LSP: process incoming notifications / responses ────────────────
            let lsp_changed = self.lsp_manager.process_messages().unwrap_or(false);
            if lsp_changed {
                needs_render = true;
            }

            // Surface any human-readable LSP messages (e.g. Copilot auth instructions).
            // These are sticky so they persist until the user presses Esc.
            for msg in self.lsp_manager.drain_messages() {
                self.set_sticky(msg);
                needs_render = true;
            }

            // Update diagnostics for current buffer — only when LSP sent something new
            // to avoid cloning the full diagnostic Vec on every frame.
            if lsp_changed {
                if let Some(buf) = self.current_buffer() {
                    if let Some(path) = &buf.file_path {
                        if let Ok(uri) = LspManager::path_to_uri(path) {
                            self.current_diagnostics = self.lsp_manager.get_diagnostics(&uri);
                        }
                    }
                }
            }

            // ── Copilot auth polling ───────────────────────────────────────────
            let auth_done = if let Some(rx) = self.copilot_auth_rx.as_mut() {
                match rx.try_recv() {
                    Ok(val) => Some(val),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(serde_json::Value::Null),
                }
            } else {
                None
            };
            if let Some(val) = auth_done {
                self.copilot_auth_rx = None;
                needs_render = true;
                let status = val.get("status").and_then(|s| s.as_str()).unwrap_or("");
                tracing::info!("Copilot auth response: {:?}", val);
                match status {
                    "OK" | "AlreadySignedIn" => {
                        let user = val.get("user").and_then(|u| u.as_str()).unwrap_or("unknown");
                        self.set_sticky(format!("Copilot: signed in as {}", user));
                    },
                    "NotSignedIn" => {
                        // Auto-escalate: start the device auth flow
                        if let Some(client) = self.lsp_manager.get_client("copilot") {
                            match client.copilot_sign_in_initiate() {
                                Ok(rx) => {
                                    self.copilot_auth_rx = Some(rx);
                                },
                                Err(e) => {
                                    self.set_sticky(format!("Copilot sign-in failed: {}", e));
                                },
                            }
                        }
                    },
                    "PromptUserDeviceFlow" => {
                        let uri =
                            val.get("verificationUri").and_then(|u| u.as_str()).unwrap_or("?");
                        let code = val.get("userCode").and_then(|c| c.as_str()).unwrap_or("?");
                        self.set_sticky(format!(
                            "Copilot auth: go to {}  and enter code: {}  (Esc to dismiss)",
                            uri, code
                        ));
                    },
                    _ => {
                        self.set_sticky(format!("Copilot: {}", val));
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Agent panel stream polling ─────────────────────────────────────
            let agent_active =
                self.agent_panel.poll_stream(self.config.agent.janitor_threshold_tokens);
            if let Some(err) = self.agent_panel.last_error.take() {
                self.set_status(format!("Agent error: {err}"));
            }
            // Clear the hook re-entry guard once the agent goes idle.
            if self.hooks_firing && self.agent_panel.status == crate::agent::AgentStatus::Idle {
                self.hooks_firing = false;
            }
            if agent_active {
                // Rate-limit agent-only renders to ≤10 Hz (100 ms between frames).
                // If another source (keyboard, watcher) already set `needs_render`
                // we render immediately; the cap only kicks in when streaming is
                // the sole reason to repaint.
                const AGENT_RENDER_INTERVAL: std::time::Duration =
                    std::time::Duration::from_millis(100);
                if needs_render {
                    // Another source is already dirty — update stamp and render now.
                    self.last_agent_render = Some(std::time::Instant::now());
                } else {
                    let due = self
                        .last_agent_render
                        .map(|t| t.elapsed() >= AGENT_RENDER_INTERVAL)
                        .unwrap_or(true);
                    if due {
                        self.last_agent_render = Some(std::time::Instant::now());
                        needs_render = true;
                    }
                }
            }

            // ── Inline assist stream polling (ADR 0111) ───────────────────────
            if self.poll_inline_assist() {
                needs_render = true;
            }
            // ── Auto-Janitor: deferred resubmit after compression ─────────────
            // pending_janitor is now consumed inside submit() when the user sends
            // their next message, so no immediate tick-loop trigger is needed here.
            // After the janitor round completes, if the user had already typed a
            // message, pending_resubmit_after_janitor is set and we fire submit
            // automatically so the user's message is sent without a second Enter.
            if self.agent_panel.pending_resubmit_after_janitor {
                self.agent_panel.pending_resubmit_after_janitor = false;
                let _ = self.execute_action(Action::AgentSubmitPending);
            }

            // Reload any buffers the agent modified on disk this tick.
            let reloads: Vec<String> = std::mem::take(&mut self.agent_panel.pending_reloads);
            for rel_path in reloads {
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let abs_path = cwd.join(&rel_path);
                // Canonicalize once — resolves symlinks, cleans ".." etc.
                // Falls back to the plain joined path if the file somehow can't be stat'd.
                let canonical = abs_path.canonicalize().unwrap_or_else(|_| abs_path.clone());

                let mut reloaded = false;
                for buf in &mut self.buffers {
                    let matches = buf
                        .file_path
                        .as_ref()
                        .map(|fp| {
                            // Case 1: buffer stored an absolute path (opened from explorer)
                            // — compare both canonicalized so symlinks don't fool us.
                            let fp_canon = fp.canonicalize().unwrap_or_else(|_| fp.clone());
                            if fp_canon == canonical {
                                return true;
                            }
                            // Case 2: buffer stored a relative path (opened from CLI)
                            // — compare component-wise suffix of the file_path against rel_path.
                            fp.ends_with(std::path::Path::new(&rel_path))
                        })
                        .unwrap_or(false);

                    if matches {
                        if let Err(e) = buf.reload_from_disk() {
                            tracing::warn!("Failed to reload {rel_path}: {e}");
                        } else {
                            reloaded = true;
                        }
                    }
                }
                if reloaded {
                    self.set_status(format!("↺ reloaded {rel_path}"));
                    needs_render = true;
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Filesystem watcher: reload buffers changed externally ──────────
            // Prune self_saved entries older than 500 ms.
            let suppress_window = std::time::Duration::from_millis(500);
            self.self_saved.retain(|_, t| t.elapsed() < suppress_window);

            let fs_changed_paths: Vec<std::path::PathBuf> = if let Some(ref rx) = self.watcher_rx {
                let mut paths = Vec::new();
                while let Ok(Ok(event)) = rx.try_recv() {
                    use notify::EventKind;
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        for p in event.paths {
                            let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
                            // Skip events caused by our own saves.
                            let self_saved = self.self_saved.keys().any(|saved| {
                                saved.canonicalize().unwrap_or_else(|_| saved.clone()) == canonical
                            });
                            if !self_saved {
                                paths.push(p);
                            }
                        }
                    }
                }
                paths
            } else {
                Vec::new()
            };

            for changed_path in fs_changed_paths {
                let canonical =
                    changed_path.canonicalize().unwrap_or_else(|_| changed_path.clone());
                let mut status_msg: Option<String> = None;
                for buf in &mut self.buffers {
                    let matches = buf
                        .file_path
                        .as_ref()
                        .map(|fp| fp.canonicalize().unwrap_or_else(|_| fp.clone()) == canonical)
                        .unwrap_or(false);
                    if !matches {
                        continue;
                    }
                    if buf.is_modified {
                        status_msg = Some(format!(
                            "⚠ external change to '{}' (unsaved — :e! to reload)",
                            buf.name
                        ));
                    } else if buf.reload_from_disk().is_ok() {
                        status_msg = Some(format!("↺ {} reloaded", buf.name));
                    }
                    needs_render = true;
                }
                if let Some(msg) = status_msg {
                    self.set_status(msg);
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Inline completion debounce + poll ──────────────────────────────
            // Fire a new request once the debounce delay has elapsed in Insert mode.
            if self.pending_completion.is_none() && self.ghost_text.is_none() {
                if let Some(instant) = self.last_edit_instant {
                    if instant.elapsed().as_millis() >= COMPLETION_DEBOUNCE_MS
                        && self.mode == Mode::Insert
                    {
                        self.last_edit_instant = None; // consume
                        self.request_inline_completion();
                    }
                }
            }

            // Poll for a response from an in-flight request.
            let completed = if let Some(rx) = self.pending_completion.as_mut() {
                match rx.try_recv() {
                    Ok(value) => Some(value),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => {
                        // channel closed without a response
                        Some(serde_json::Value::Null)
                    },
                }
            } else {
                None
            };
            if let Some(value) = completed {
                self.pending_completion = None;
                needs_render = true;
                if let Some(text) = parse_first_inline_completion(value) {
                    if let Some(buf) = self.current_buffer() {
                        let row = buf.cursor.row;
                        let col = buf.cursor.col;
                        self.ghost_text = Some((text, row, col));
                    }
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Project-wide search: debounce + poll ──────────────────────────
            const SEARCH_DEBOUNCE_MS: u128 = 300;
            if self.search_rx.is_none() {
                if let Some(instant) = self.last_search_instant {
                    if instant.elapsed().as_millis() >= SEARCH_DEBOUNCE_MS
                        && self.mode == Mode::Search
                    {
                        self.last_search_instant = None;
                        self.fire_search();
                    }
                }
            }

            let search_done = if let Some(rx) = self.search_rx.as_mut() {
                match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(Err(anyhow::anyhow!("search channel closed"))),
                }
            } else {
                None
            };
            if let Some(result) = search_done {
                self.search_rx = None;
                needs_render = true;
                match result {
                    Ok(results) => {
                        self.search_state.set_results(results);
                    },
                    Err(e) => {
                        self.search_state.status = SearchStatus::Error(e.to_string());
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── LSP goto-definition / references / symbols polls ──────────────
            macro_rules! poll_lsp_rx {
                ($field:expr) => {{
                    if let Some(rx) = $field.as_mut() {
                        match rx.try_recv() {
                            Ok(v) => {
                                $field = None;
                                needs_render = true;
                                Some(v)
                            },
                            Err(oneshot::error::TryRecvError::Empty) => None,
                            Err(_) => {
                                $field = None;
                                Some(serde_json::Value::Null)
                            },
                        }
                    } else {
                        None
                    }
                }};
            }
            if let Some(v) = poll_lsp_rx!(self.pending_goto_definition) {
                self.handle_goto_definition_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_references) {
                self.handle_references_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_symbols) {
                self.handle_symbols_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_hover) {
                self.handle_hover_response(v);
            }
            if let Some(v) = poll_lsp_rx!(self.pending_rename) {
                self.handle_rename_response(v);
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Commit-message AI response poll ───────────────────────────────
            let commit_done = if let Some(rx) = self.commit_msg.rx.as_mut() {
                match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(Err(anyhow::anyhow!("commit msg channel closed"))),
                }
            } else {
                None
            };
            if let Some(result) = commit_done {
                self.commit_msg.rx = None;
                needs_render = true;
                match result {
                    Ok(msg) => {
                        self.commit_msg.buffer = msg;
                        self.set_status(
                            "Commit message ready — edit then Enter to commit, Esc to discard"
                                .to_string(),
                        );
                    },
                    Err(e) => {
                        self.mode = Mode::Normal;
                        self.set_status(format!("Failed to generate commit message: {e}"));
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Release-notes AI response poll ────────────────────────────────
            let release_notes_done = if let Some(rx) = self.release_notes.rx.as_mut() {
                match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(_) => Some(Err(anyhow::anyhow!("release notes channel closed"))),
                }
            } else {
                None
            };
            if let Some(result) = release_notes_done {
                self.release_notes.rx = None;
                needs_render = true;
                match result {
                    Ok(notes) => {
                        self.release_notes.buffer = strip_markdown_fence(&notes);
                        self.set_status(
                            "Release notes ready — y=copy to clipboard, j/k=scroll, Esc=close"
                                .to_string(),
                        );
                    },
                    Err(e) => {
                        self.mode = Mode::Normal;
                        self.set_status(format!("Failed to generate release notes: {e}"));
                    },
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── MCP background connection poll ────────────────────────────────
            if let Some(rx) = self.mcp_rx.as_mut() {
                if let Ok(manager) = rx.try_recv() {
                    tracing::info!("MCP ready: {}", manager.summary());
                    let arc = std::sync::Arc::new(manager);
                    self.mcp_manager = Some(std::sync::Arc::clone(&arc));
                    self.agent_panel.mcp_manager = Some(arc);
                    self.mcp_rx = None;
                    needs_render = true;
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // Force a render whenever background work is in-flight OR the
            // which-key timer is pending (so the popup appears after 500 ms
            // even when no key event arrives to trigger a normal render).
            if self.copilot_auth_rx.is_some()
                || self.pending_completion.is_some()
                || self.key_handler.which_key_pending()
                || self.search_rx.is_some()
                || self.commit_msg.rx.is_some()
                || self.release_notes.rx.is_some()
                || self.mcp_rx.is_some()
            {
                needs_render = true;
            }

            // ── SIGCONT: drain any pending resume notifications ────────────────
            #[cfg(unix)]
            while sigcont_rx.try_recv().is_ok() {
                force_clear = true;
                needs_render = true;
            }

            // ── Render (only when something changed) ───────────────────────────
            if needs_render {
                if force_clear {
                    self.terminal.clear()?;
                    force_clear = false;
                }
                self.render()?;
                needs_render = false;
            }

            // ── Input (blocks up to 50 ms) ─────────────────────────────────────
            if event::poll(std::time::Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) => {
                        // Ctrl+L: force a full redraw (universal terminal convention).
                        // Intercepted before handle_key so it works in every mode.
                        if key.code == KeyCode::Char('l') && key.modifiers == KeyModifiers::CONTROL
                        {
                            force_clear = true;
                        } else {
                            self.handle_key(key)?;
                        }
                        needs_render = true;
                    },
                    // Bracketed paste: the terminal wraps pasted text in escape sequences
                    // so it arrives as a single Event::Paste(String) instead of a stream
                    // of KeyCode::Char / KeyCode::Enter events.
                    Event::Paste(text) => {
                        self.handle_paste(text)?;
                        needs_render = true;
                    },
                    // Terminal resize: the cell grid has been invalidated — clear and
                    // repaint so ratatui lays out to the new dimensions correctly.
                    Event::Resize(_, _) => {
                        force_clear = true;
                        needs_render = true;
                    },
                    _ => {},
                }
            }

            if self.should_quit {
                break;
            }
        }

        // Clean up terminal
        self.cleanup()?;
        Ok(())
    }

    /// Render the UI
    fn render(&mut self) -> Result<()> {
        let mode = self.mode;
        // Clone into owned Strings so these don't hold borrows on `self`
        // while we need a mutable borrow below to call scroll_to_cursor().
        let status_owned = self.status_message.clone();
        let status = status_owned.as_deref();
        let command_buffer_owned =
            if self.mode == Mode::Command { Some(self.command_buffer.clone()) } else { None };
        let command_buffer = command_buffer_owned.as_deref();

        // Check if we should show which-key
        let show_which_key = self.key_handler.should_show_which_key();
        let which_key_options =
            if show_which_key { Some(self.key_handler.which_key_options()) } else { None };

        // Get key sequence for display
        let key_sequence = self.key_handler.sequence();

        // ── Scroll to keep cursor in view ─────────────────────────────────────
        // Must happen before the buffer snapshot so scroll_row/col are current.
        //
        // viewport_height: subtract status line (1) and which-key popup (dynamic) when shown.
        //
        // viewport_width: we must match the three-panel layout produced by UI::render()
        // so that horizontal scrolling kicks in at the right column.  The layout is:
        //   explorer+agent → [Length(25), Min(1), Percentage(35)]
        //   explorer only  → [Length(25), Min(1)]
        //   agent only     → [Percentage(60), Percentage(40)]
        //   neither        → [Min(1)]
        // Then subtract 2 for the diagnostic gutter that is always prepended.
        let size = self.terminal.size().unwrap_or_default();
        // which-key popup: 2 (borders) + 1 (header) + number of options
        let wk_height = which_key_options.as_ref().map_or(0, |opts| opts.len() + 3);
        let viewport_height =
            (size.height as usize).saturating_sub(if show_which_key { wk_height + 1 } else { 1 });

        const GUTTER: usize = 2;
        let total_w = size.width as usize;
        let editor_area_w = match (self.file_explorer.visible, self.agent_panel.visible) {
            (true, true) => total_w.saturating_sub(25).saturating_sub(total_w * 35 / 100),
            (true, false) => total_w.saturating_sub(25),
            (false, true) => total_w * 60 / 100,
            (false, false) => total_w,
        };
        let viewport_width = editor_area_w.saturating_sub(GUTTER);

        if self.config.soft_wrap {
            // Text width = viewport_width (gutter already subtracted above).
            self.with_buffer(|buf| buf.scroll_to_cursor_wrapped(viewport_height, viewport_width));
        } else {
            self.with_buffer(|buf| buf.scroll_to_cursor(viewport_height, viewport_width));
        }

        // ── Fold data (ADR 0106) ──────────────────────────────────────────────
        // Populate the tree-sitter cache for the current buffer (no-op if already
        // current), then compute which rows are hidden and which are fold stubs.
        let buf_idx = self.current_buffer_idx;
        let _ = self.ts_tree_for_current_buffer(); // ensures ts_cache[buf_idx] is fresh

        let fold_ranges: Vec<(usize, usize)> = self
            .ts_cache
            .get(&buf_idx)
            .map(crate::treesitter::query::fold_ranges)
            .unwrap_or_default();

        let fold_closed_set = self.fold_closed.get(&buf_idx).cloned().unwrap_or_default();

        let mut fold_hidden_rows: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let mut fold_stub_map: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for &(start, end) in &fold_ranges {
            if fold_closed_set.contains(&start) {
                for row in (start + 1)..=end {
                    fold_hidden_rows.insert(row);
                }
                fold_stub_map.insert(start, end);
            }
        }

        // The fold_data passed to the renderer (None when no folds are active).
        let fold_data_owned: Option<crate::ui::FoldData> = if !fold_stub_map.is_empty() {
            Some(crate::ui::FoldData {
                hidden_rows: fold_hidden_rows.clone(),
                fold_starts: fold_stub_map,
            })
        } else {
            None
        };
        let fold_data_ref: Option<&crate::ui::FoldData> = fold_data_owned.as_ref();

        // ── Sticky scroll header (ADR 0107) ───────────────────────────────────
        let scroll_row_for_sticky = self.current_buffer().map(|b| b.scroll_row).unwrap_or(0);
        let lsp_ver_for_sticky = self.current_buffer().map(|b| b.lsp_version).unwrap_or(0);
        let cache_hit = self.sticky_scroll_cache.as_ref().is_some_and(|c| {
            c.buffer_idx == buf_idx
                && c.scroll_row == scroll_row_for_sticky
                && c.lsp_version == lsp_ver_for_sticky
        });
        if !cache_hit {
            let header = self.ts_cache.get(&buf_idx).and_then(|s| {
                crate::treesitter::query::sticky_scroll_header(s, scroll_row_for_sticky)
            });
            self.sticky_scroll_cache = Some(StickyScrollCache {
                buffer_idx: buf_idx,
                scroll_row: scroll_row_for_sticky,
                lsp_version: lsp_ver_for_sticky,
                header,
            });
        }
        let sticky_header_owned: Option<String> =
            self.sticky_scroll_cache.as_ref().and_then(|c| c.header.clone());
        let sticky_header_ref: Option<&str> = sticky_header_owned.as_deref();

        // ── Account for sticky header in viewport height ───────────────────────
        // When a sticky header is present it occupies 1 row; reduce the content
        // height used for scroll calculations and line-range clipping accordingly.
        let sticky_height: usize = if sticky_header_owned.is_some() { 1 } else { 0 };
        let content_viewport_height = viewport_height.saturating_sub(sticky_height);

        // Get buffer data before drawing to avoid borrow issues.
        // Extend the visible range by the number of hidden rows so that after
        // fold-skipping the renderer still has enough lines to fill the viewport.
        let buffer_data = self.current_buffer().map(|buf| {
            let extra = fold_hidden_rows.len();
            let vis_end = (buf.scroll_row + content_viewport_height + extra).min(buf.lines().len());

            // Adjust cursor.row to the visual row: subtract the number of hidden
            // rows that fall between scroll_row and the cursor row so that the
            // renderer positions the terminal cursor at the correct screen row.
            let hidden_before_cursor =
                (buf.scroll_row..buf.cursor.row).filter(|r| fold_hidden_rows.contains(r)).count();
            let mut visual_cursor = buf.cursor.clone();
            visual_cursor.row = buf.cursor.row.saturating_sub(hidden_before_cursor);

            (
                buf.name.clone(),
                buf.is_modified,
                visual_cursor,
                buf.scroll_row,
                buf.scroll_col,
                buf.lines()[buf.scroll_row..vis_end].to_vec(),
                buf.selection.clone(),
            )
        });

        // ── Syntax-highlight cache ─────────────────────────────────────────────
        // Re-use spans from the previous frame when the buffer content (lsp_version),
        // scroll position, and active buffer are all unchanged.  This eliminates the
        // ~3–8 ms syntect cost on every frame where the user is just moving the cursor.
        let term_height = viewport_height;

        // Collect the cache key from an immutable borrow (borrow ends before mut access).
        let cache_key = self.current_buffer().map(|buf| {
            let ext = buf.file_path.as_deref().map(Highlighter::extension_for).unwrap_or_default();
            let name = buf.file_path.as_deref().map(Highlighter::filename_for).unwrap_or_default();
            (buf.scroll_row, buf.lsp_version, ext, name)
        });

        let highlighted_lines: Option<Arc<Vec<Vec<Span<'static>>>>> =
            if let Some((scroll_row, lsp_ver, ext, name)) = cache_key {
                // Cache hit only when: same buffer, same scroll position, same content version,
                // AND the cached range covers at least as many rows as we now need
                // (the range grows when folds are active to cover hidden rows).
                let hl_extra = fold_hidden_rows.len();
                let required_end = (scroll_row + term_height + hl_extra).min(usize::MAX);
                let cache_hit = self.highlight_cache.as_ref().is_some_and(|c| {
                    c.buffer_idx == buf_idx
                        && c.scroll_row == scroll_row
                        && c.lsp_version == lsp_ver
                        && c.spans.len() >= required_end.saturating_sub(scroll_row)
                });

                if cache_hit {
                    // Cache hit: Arc::clone is a single atomic increment — zero allocation.
                    self.highlight_cache.as_ref().map(|c| Arc::clone(&c.spans))
                } else {
                    // Cache miss: run syntect for the visible window and store result.
                    // Extend the range by the number of hidden (fold-skipped) rows so
                    // that the renderer can look up highlights for all visible buffer rows
                    // using `line_idx = buf_row - scroll_row` even when folds are active.
                    let spans = if let Some(buf) = self.current_buffer() {
                        let hl_extra = fold_hidden_rows.len();
                        let end = (scroll_row + term_height + hl_extra).min(buf.lines().len());
                        buf.lines()[scroll_row..end]
                            .iter()
                            .map(|line| self.highlighter.highlight_line(line, &ext, &name))
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    let arc = Arc::new(spans);
                    self.highlight_cache = Some(HighlightCache {
                        buffer_idx: buf_idx,
                        scroll_row,
                        lsp_version: lsp_ver,
                        spans: Arc::clone(&arc),
                    });
                    Some(arc)
                }
            } else {
                None
            };

        // Buffer list for PickBuffer mode
        let buffer_list = if self.mode == Mode::PickBuffer {
            Some((
                self.buffers.iter().map(|b| (b.name.clone(), b.is_modified)).collect::<Vec<_>>(),
                self.buffer_picker_idx,
            ))
        } else {
            None
        };

        // File list for PickFile mode
        let file_list = if self.mode == Mode::PickFile {
            Some((self.file_list.clone(), self.file_picker_idx, self.file_query.clone()))
        } else {
            None
        };

        let ghost = self.ghost_text.as_ref().map(|(text, row, col)| (text.as_str(), *row, *col));

        // ── Markdown preview lines ─────────────────────────────────────────────
        // Computed when in MarkdownPreview mode; cached by (lsp_version, viewport_width)
        // so markdown re-parsing is skipped on frames where nothing changed.
        let preview_lines_owned: Option<Vec<ratatui::text::Line<'static>>> = if mode
            == Mode::MarkdownPreview
        {
            let all_lines = {
                let buf_idx = self.current_buffer_idx;
                let key = self.current_buffer().map(|buf| buf.lsp_version);
                let cache_hit = self.markdown_cache.as_ref().is_some_and(|c| {
                    c.buffer_idx == buf_idx
                        && Some(c.lsp_version) == key
                        && c.viewport_width == viewport_width
                });
                if cache_hit {
                    self.markdown_cache.as_ref().unwrap().lines.clone()
                } else {
                    let ver = key.unwrap_or(0);
                    let content =
                        self.current_buffer().map(|buf| buf.lines().join("\n")).unwrap_or_default();
                    let rendered = crate::markdown::render(&content, viewport_width);
                    self.markdown_cache = Some(MarkdownCache {
                        buffer_idx: buf_idx,
                        lsp_version: ver,
                        viewport_width,
                        lines: rendered.clone(),
                    });
                    rendered
                }
            };
            // Cap scroll so we can't scroll past the end.
            let max_scroll = all_lines.len().saturating_sub(1);
            let scroll = self.preview_scroll.min(max_scroll);
            Some(all_lines.into_iter().skip(scroll).collect())
        } else {
            None
        };

        // ── Split pane data ────────────────────────────────────────────────────
        // Same viewport-clipped approach as the primary buffer: only the visible
        // rows are cloned.
        let split_buffer_data = self.split.other_idx.and_then(|idx| {
            self.buffers.get(idx).map(|buf| {
                let vis_end = (buf.scroll_row + viewport_height).min(buf.lines().len());
                (
                    buf.name.clone(),
                    buf.is_modified,
                    buf.cursor.clone(),
                    buf.scroll_row,
                    buf.scroll_col,
                    buf.lines()[buf.scroll_row..vis_end].to_vec(),
                    buf.selection.clone(),
                )
            })
        });

        // ── Split highlight cache ──────────────────────────────────────────────
        let split_highlighted_lines: Option<Arc<Vec<Vec<ratatui::text::Span<'static>>>>> =
            if let Some(split_idx) = self.split.other_idx {
                if let Some(split_buf) = self.buffers.get(split_idx) {
                    let split_scroll = split_buf.scroll_row;
                    let split_ver = split_buf.lsp_version;
                    let split_ext = split_buf
                        .file_path
                        .as_deref()
                        .map(Highlighter::extension_for)
                        .unwrap_or_default();
                    let split_name = split_buf
                        .file_path
                        .as_deref()
                        .map(Highlighter::filename_for)
                        .unwrap_or_default();
                    let cache_hit = self.split.highlight_cache.as_ref().is_some_and(|c| {
                        c.buffer_idx == split_idx
                            && c.scroll_row == split_scroll
                            && c.lsp_version == split_ver
                    });
                    if cache_hit {
                        // Cache hit: Arc::clone is a single atomic increment — zero allocation.
                        self.split.highlight_cache.as_ref().map(|c| Arc::clone(&c.spans))
                    } else {
                        let end = (split_scroll + term_height).min(split_buf.lines().len());
                        let spans: Vec<Vec<ratatui::text::Span<'static>>> = split_buf.lines()
                            [split_scroll..end]
                            .iter()
                            .map(|line| {
                                self.highlighter.highlight_line(line, &split_ext, &split_name)
                            })
                            .collect();
                        let arc = Arc::new(spans);
                        self.split.highlight_cache = Some(HighlightCache {
                            buffer_idx: split_idx,
                            scroll_row: split_scroll,
                            lsp_version: split_ver,
                            spans: Arc::clone(&arc),
                        });
                        Some(arc)
                    }
                } else {
                    None
                }
            } else {
                None
            };

        let split_right_focused = self.split.right_focused;

        let agent_ref = if self.agent_panel.visible { Some(&self.agent_panel) } else { None };
        let explorer_ref =
            if self.file_explorer.visible { Some(&self.file_explorer) } else { None };
        let hl_ref: Option<&[Vec<Span<'static>>]> = highlighted_lines.as_deref().map(Vec::as_slice);
        let split_hl_ref: Option<&[Vec<Span<'static>>]> =
            split_highlighted_lines.as_deref().map(Vec::as_slice);
        let preview_ref = preview_lines_owned.as_deref();
        let search_ref = if mode == Mode::Search { Some(&self.search_state) } else { None };
        let rename_buf_owned =
            if mode == Mode::RenameFile { Some(self.rename_buffer.clone()) } else { None };
        let rename_buf = rename_buf_owned.as_deref();

        let new_folder_buf_owned =
            if mode == Mode::NewFolder { Some(self.new_folder_buffer.clone()) } else { None };
        let new_folder_buf = new_folder_buf_owned.as_deref();

        let delete_path_owned = if mode == Mode::DeleteFile {
            self.delete_confirm_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        } else {
            None
        };
        let delete_name = delete_path_owned.as_deref();

        let commit_msg_buf =
            if mode == Mode::CommitMsg { Some(self.commit_msg.buffer.as_str()) } else { None };

        let release_notes_view = if mode == Mode::ReleaseNotes {
            Some(crate::ui::ReleaseNotesView {
                count_input: self.release_notes.count_input.as_str(),
                generating: self.release_notes.rx.is_some(),
                notes: self.release_notes.buffer.as_str(),
                scroll: self.release_notes.scroll,
            })
        } else {
            None
        };

        let mcp_failed_empty: Vec<(String, String)> = Vec::new();
        let recent_logs_owned: Vec<(String, String)> =
            self.log_buffer.lock().map(|g| g.iter().cloned().collect()).unwrap_or_default();
        let diag_overlay = if mode == Mode::Diagnostics {
            let mcp_connected =
                self.mcp_manager.as_ref().map(|m| m.connected_servers()).unwrap_or_default();
            let mcp_failed: &[(String, String)] = self
                .mcp_manager
                .as_ref()
                .map(|m| m.failed_servers.as_slice())
                .unwrap_or(mcp_failed_empty.as_slice());
            let lsp_servers =
                self.config.lsp.servers.iter().map(|s| s.language.as_str()).collect::<Vec<_>>();
            let agent_session_tokens = if self.agent_panel.session_rounds > 0 {
                Some((
                    self.agent_panel.total_session_prompt_tokens,
                    self.agent_panel.total_session_completion_tokens,
                    self.agent_panel.context_window_size(),
                    self.agent_panel.session_rounds,
                ))
            } else {
                None
            };
            Some(crate::ui::DiagnosticsData {
                version: env!("CARGO_PKG_VERSION"),
                mcp_connected,
                mcp_failed,
                lsp_servers,
                log_path: "~/.local/share/forgiven/forgiven.log",
                recent_logs: recent_logs_owned.as_slice(),
                agent_session_tokens,
                agent_ctx_breakdown: self.agent_panel.last_breakdown,
                mcp_call_log: self
                    .mcp_manager
                    .as_ref()
                    .map(|m| m.recent_calls())
                    .unwrap_or_default(),
            })
        } else {
            None
        };

        // File-info popup: stat the selected explorer entry once per frame while active.
        // `fs::metadata` on a local filesystem is effectively instantaneous (~1 µs).
        let file_info_data: Option<crate::ui::FileInfoData> = if self.show_file_info {
            self.file_explorer.selected_path().and_then(|path| {
                std::fs::metadata(&path).ok().map(|meta| {
                    #[cfg(unix)]
                    let permissions = {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = meta.permissions().mode();
                        let bits: &[(u32, char)] = &[
                            (0o400, 'r'),
                            (0o200, 'w'),
                            (0o100, 'x'),
                            (0o040, 'r'),
                            (0o020, 'w'),
                            (0o010, 'x'),
                            (0o004, 'r'),
                            (0o002, 'w'),
                            (0o001, 'x'),
                        ];
                        Some(
                            bits.iter()
                                .map(|(mask, ch)| if mode & mask != 0 { *ch } else { '-' })
                                .collect::<String>(),
                        )
                    };
                    #[cfg(not(unix))]
                    let permissions: Option<String> = None;
                    crate::ui::FileInfoData {
                        is_dir: meta.is_dir(),
                        size_bytes: if meta.is_file() { Some(meta.len()) } else { None },
                        modified: meta.modified().ok(),
                        created: meta.created().ok(),
                        permissions,
                        path,
                    }
                })
            })
        } else {
            None
        };

        self.terminal.draw(|frame| {
            let ctx = RenderContext {
                mode,
                buffer_data: buffer_data.as_ref(),
                status_message: status,
                command_buffer,
                which_key_options: which_key_options.as_deref(),
                key_sequence: key_sequence.as_str(),
                buffer_list: buffer_list.as_ref(),
                file_list: file_list.as_ref(),
                diagnostics: &self.current_diagnostics,
                ghost_text: ghost,
                agent_panel: agent_ref,
                highlighted_lines: hl_ref,
                file_explorer: explorer_ref,
                preview_lines: preview_ref,
                search_state: search_ref,
                rename_buffer: rename_buf,
                delete_name,
                new_folder_buffer: new_folder_buf,
                split_buffer_data: split_buffer_data.as_ref(),
                split_highlighted_lines: split_hl_ref,
                split_right_focused,
                commit_msg: commit_msg_buf,
                release_notes: release_notes_view.as_ref(),
                diag_overlay: diag_overlay.as_ref(),
                binary_file_path: self.binary_file_path.as_deref(),
                startup_elapsed: self.startup_elapsed,
                file_info: file_info_data.as_ref(),
                location_list: if mode == Mode::LocationList {
                    self.location_list.as_ref()
                } else {
                    None
                },
                in_file_search_query: if mode == Mode::InFileSearch {
                    Some(self.in_file_search_buffer.as_str())
                } else {
                    None
                },
                hover_popup: if mode == Mode::LspHover { self.hover_popup.as_ref() } else { None },
                lsp_rename_buffer: if mode == Mode::LspRename {
                    Some(self.lsp_rename_buffer.as_str())
                } else {
                    None
                },
                fold_data: fold_data_ref,
                sticky_header: sticky_header_ref,
                inline_assist: self.inline_assist.as_ref().map(|s| crate::ui::InlineAssistView {
                    prompt: &s.prompt,
                    response: &s.response,
                    phase: s.phase,
                }),
                review_changes: if mode == Mode::ReviewChanges {
                    self.review_changes.as_ref()
                } else {
                    None
                },
                soft_wrap: self.config.soft_wrap,
            };
            UI::render(frame, &ctx);
        })?;

        Ok(())
    }

    /// Cycle focus left-to-right through visible panels: Explorer → Editor → Agent → (wrap).
    /// Panels that are not currently visible  Visual mode
    fn check_quit(&mut self) -> Result<()> {
        for buf in &self.buffers {
            if buf.is_modified {
                self.set_status(format!(
                    "'{}' has unsaved changes. :w to save, :q! to force quit.",
                    buf.name
                ));
                return Ok(());
            }
        }
        self.should_quit = true;
        Ok(())
    }

    /// Set a transient status message (cleared on next keypress).
    fn set_status(&mut self, msg: String) {
        self.status_sticky = false;
        self.status_message = Some(msg);
    }

    /// Set a sticky status message that persists until the user presses Esc.
    /// Use for important notifications the user must read (e.g. Copilot auth URL).
    fn set_sticky(&mut self, msg: String) {
        self.status_sticky = true;
        self.status_message = Some(msg);
    }

    /// Write `text` to the OS system clipboard.
    /// Errors are silently swallowed — the internal register is always primary.
    fn sync_system_clipboard(&self, text: &str) {
        match arboard::Clipboard::new() {
            Ok(mut cb) => {
                if let Err(e) = cb.set_text(text.to_string()) {
                    tracing::debug!("system clipboard write failed: {e}");
                }
            },
            Err(e) => tracing::debug!("system clipboard unavailable: {e}"),
        }
    }

    /// Clean up terminal state before exit
    fn cleanup(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Strip a wrapping code fence from an inline assist response.
///
/// Many models wrap their output in ` ```lang\n…\n``` ` despite being told not
/// to.  This strips the opening fence (including any language tag) and the
/// closing fence, returning only the code body.
fn strip_assist_fence(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Skip optional language tag up to the first newline.
        let body = if let Some(nl) = rest.find('\n') { &rest[nl + 1..] } else { rest };
        // Strip closing fence.
        let body = body.strip_suffix("```").unwrap_or(body).trim_end();
        return body.to_string();
    }
    trimmed.to_string()
}
