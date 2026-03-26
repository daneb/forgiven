mod actions;
mod ai;
mod diff;
mod file_ops;
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
use crate::keymap::{KeyHandler, Mode};
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

/// A line in an LCS-based unified diff.
#[derive(Debug, Clone)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
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

/// State for the apply-diff overlay (Mode::ApplyDiff).
#[derive(Default)]
struct ApplyDiffState {
    path: Option<std::path::PathBuf>,
    content: Option<String>,
    lines: Vec<DiffLine>,
    scroll: usize,
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

    // ── Clipboard (yank register) ─────────────────────────────────────────────
    /// Last yanked / deleted text + whether it is linewise or charwise.
    clipboard: Option<(String, ClipboardType)>,

    // ── Syntax highlighter ────────────────────────────────────────────────────
    /// Loaded once at startup; highlight_line() is called per visible line each frame.
    highlighter: Highlighter,

    /// Per-viewport highlight cache — invalidated on content change or scroll.
    highlight_cache: Option<HighlightCache>,

    // ── File explorer ─────────────────────────────────────────────────────────
    file_explorer: FileExplorer,

    // ── Markdown preview ──────────────────────────────────────────────────────
    /// Scroll offset (in rendered lines) for Mode::MarkdownPreview.
    preview_scroll: usize,
    /// Cached rendered markdown lines — avoids re-parsing on every render frame.
    markdown_cache: Option<MarkdownCache>,

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

    // ── Apply-diff overlay (Mode::ApplyDiff) ──────────────────────────────────
    apply_diff: ApplyDiffState,

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
                panel
            },
            clipboard: None::<(String, ClipboardType)>,
            highlighter: Highlighter::new(),
            highlight_cache: None,
            file_explorer: FileExplorer::new(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            ),
            preview_scroll: 0,
            markdown_cache: None,
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
            apply_diff: ApplyDiffState::default(),
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
            let agent_active = self.agent_panel.poll_stream();
            if let Some(err) = self.agent_panel.last_error.take() {
                self.set_status(format!("Agent error: {err}"));
            }
            if agent_active {
                needs_render = true;
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

        self.with_buffer(|buf| buf.scroll_to_cursor(viewport_height, viewport_width));

        // Get buffer data before drawing to avoid borrow issues.
        // Only clone the lines that are actually visible in the viewport — for a
        // 10 000-line file this reduces the per-frame allocation from O(N_total)
        // to O(viewport_height) (typically ~50 lines).
        let buffer_data = self.current_buffer().map(|buf| {
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
        });

        // ── Syntax-highlight cache ─────────────────────────────────────────────
        // Re-use spans from the previous frame when the buffer content (lsp_version),
        // scroll position, and active buffer are all unchanged.  This eliminates the
        // ~3–8 ms syntect cost on every frame where the user is just moving the cursor.
        let term_height = viewport_height;
        let buf_idx = self.current_buffer_idx;

        // Collect the cache key from an immutable borrow (borrow ends before mut access).
        let cache_key = self.current_buffer().map(|buf| {
            let ext = buf.file_path.as_deref().map(Highlighter::extension_for).unwrap_or_default();
            let name = buf.file_path.as_deref().map(Highlighter::filename_for).unwrap_or_default();
            (buf.scroll_row, buf.lsp_version, ext, name)
        });

        let highlighted_lines: Option<Arc<Vec<Vec<Span<'static>>>>> =
            if let Some((scroll_row, lsp_ver, ext, name)) = cache_key {
                let cache_hit = self.highlight_cache.as_ref().is_some_and(|c| {
                    c.buffer_idx == buf_idx
                        && c.scroll_row == scroll_row
                        && c.lsp_version == lsp_ver
                });

                if cache_hit {
                    // Cache hit: Arc::clone is a single atomic increment — zero allocation.
                    self.highlight_cache.as_ref().map(|c| Arc::clone(&c.spans))
                } else {
                    // Cache miss: run syntect for the visible window and store result.
                    let spans = if let Some(buf) = self.current_buffer() {
                        let end = (scroll_row + term_height).min(buf.lines().len());
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

        let apply_diff_target_owned: Option<String> = if mode == Mode::ApplyDiff {
            Some(
                self.apply_diff
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(unsaved buffer)".to_string()),
            )
        } else {
            None
        };
        let apply_diff_view = apply_diff_target_owned.as_ref().map(|t| crate::ui::ApplyDiffView {
            target: t.as_str(),
            lines: &self.apply_diff.lines,
            scroll: self.apply_diff.scroll,
        });
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
            let agent_session_tokens = if self.agent_panel.total_session_prompt_tokens > 0 {
                Some((
                    self.agent_panel.total_session_prompt_tokens,
                    self.agent_panel.total_session_completion_tokens,
                    self.agent_panel.context_window_size(),
                ))
            } else {
                None
            };
            Some(crate::ui::DiagnosticsData {
                version: env!("CARGO_PKG_VERSION"),
                mcp_connected,
                mcp_failed,
                lsp_servers,
                log_path: "/tmp/forgiven.log",
                recent_logs: recent_logs_owned.as_slice(),
                agent_session_tokens,
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
                apply_diff: apply_diff_view.as_ref(),
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
