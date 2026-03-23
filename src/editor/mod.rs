use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::fs;
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
use crate::search::{run_search, SearchState, SearchStatus};
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
            Some(crate::ui::DiagnosticsData {
                version: env!("CARGO_PKG_VERSION"),
                mcp_connected,
                mcp_failed,
                lsp_servers,
                log_path: "/tmp/forgiven.log",
                recent_logs: recent_logs_owned.as_slice(),
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
            };
            UI::render(frame, &ctx);
        })?;

        Ok(())
    }

    /// Cycle focus left-to-right through visible panels: Explorer → Editor → Agent → (wrap).
    /// Panels that are not currently visible are skipped.
    fn cycle_panel_focus(&mut self) {
        let current: u8 = match self.mode {
            Mode::Explorer => 0,
            Mode::Agent => 2,
            _ => 1,
        };

        // Build ordered list of visible panel indices (explorer=0, editor=1, agent=2).
        let mut visible: Vec<u8> = vec![1]; // editor is always present
        if self.file_explorer.visible {
            visible.insert(0, 0);
        }
        if self.agent_panel.visible {
            visible.push(2);
        }

        if visible.len() < 2 {
            return;
        }

        let pos = visible.iter().position(|&p| p == current).unwrap_or(0);
        let next = visible[(pos + 1) % visible.len()];

        // Blur the panel losing focus.
        match current {
            0 => self.file_explorer.blur(),
            2 => self.agent_panel.blur(),
            _ => {},
        }

        // Focus the panel gaining focus.
        match next {
            0 => {
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            2 => {
                self.agent_panel.focus();
                self.mode = Mode::Agent;
            },
            _ => {
                self.mode = Mode::Normal;
            },
        }
    }

    /// Handle a key press
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Esc always clears sticky notifications (user explicitly dismissing).
        if key.code == KeyCode::Esc {
            self.status_sticky = false;
        }

        // Clear transient status message on any new input (except sticky messages and picker modes).
        if self.mode != Mode::PickBuffer
            && self.mode != Mode::PickFile
            && self.mode != Mode::Search
            && !self.status_sticky
        {
            self.status_message = None;
        }

        // Global: Ctrl+W cycles visible panels (Explorer → Editor → Agent → wrap).
        // Skip in modes that capture text input or show modal overlays.
        if key.code == KeyCode::Char('w')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !matches!(
                self.mode,
                Mode::Command
                    | Mode::PickBuffer
                    | Mode::PickFile
                    | Mode::InFileSearch
                    | Mode::RenameFile
                    | Mode::DeleteFile
                    | Mode::NewFolder
                    | Mode::ApplyDiff
                    | Mode::CommitMsg
                    | Mode::Diagnostics
            )
        {
            self.cycle_panel_focus();
            return Ok(());
        }

        match self.mode {
            Mode::Normal => self.handle_normal_mode(key)?,
            Mode::Insert => self.handle_insert_mode(key)?,
            Mode::Command => self.handle_command_mode(key)?,
            Mode::Visual => self.handle_visual_mode(key)?,
            Mode::VisualLine => self.handle_visual_line_mode(key)?,
            Mode::PickBuffer => self.handle_pick_buffer_mode(key)?,
            Mode::PickFile => self.handle_pick_file_mode(key)?,
            Mode::Agent => self.handle_agent_mode(key)?,
            Mode::Explorer => self.handle_explorer_mode(key)?,
            Mode::MarkdownPreview => self.handle_preview_mode(key)?,
            Mode::Search => self.handle_search_mode(key)?,
            Mode::InFileSearch => self.handle_in_file_search_mode(key)?,
            Mode::RenameFile => self.handle_rename_mode(key)?,
            Mode::DeleteFile => self.handle_delete_mode(key)?,
            Mode::NewFolder => self.handle_new_folder_mode(key)?,
            Mode::ApplyDiff => self.handle_apply_diff_mode(key)?,
            Mode::CommitMsg => self.handle_commit_msg_mode(key)?,
            Mode::ReleaseNotes => self.handle_release_notes_mode(key)?,
            Mode::Diagnostics => {
                // Any key closes the overlay.
                self.mode = Mode::Normal;
            },
            Mode::BinaryFile => self.handle_binary_file_mode(key)?,
            Mode::LocationList => self.handle_location_list_mode(key)?,
        }

        Ok(())
    }

    /// Handle keys in Normal mode
    fn handle_normal_mode(&mut self, key: KeyEvent) -> Result<()> {
        let action = self.key_handler.handle_normal(key);
        self.execute_action(action)?;
        Ok(())
    }

    /// Execute an action
    fn execute_action(&mut self, action: Action) -> Result<()> {
        // Don't consume the count for Noop — the user may still be building a
        // count prefix (e.g. typing "3" before "d") and we must not lose it.
        if matches!(action, Action::Noop) {
            return Ok(());
        }

        // Consume the accumulated count (defaults to 1 if none).
        let count = self.key_handler.take_count();

        // ── Undo snapshot ─────────────────────────────────────────────────────
        // Save buffer state BEFORE any action that mutates content.
        // Insert-mode entry actions save once here — all subsequent keystrokes
        // in Insert mode are NOT snapshotted individually, so the whole Insert
        // session forms a single undo step (vim behaviour).
        let needs_snapshot = matches!(
            action,
            // Enter Insert mode (one snapshot per Insert session)
            Action::Insert
            | Action::InsertAppend
            | Action::InsertLineStart
            | Action::InsertLineEnd
            | Action::InsertNewlineBelow
            | Action::InsertNewlineAbove
            // Normal-mode destructive operations
            | Action::DeleteChar
            | Action::DeleteLine
            | Action::DeleteToLineEnd
            | Action::DeleteWord
            | Action::DeleteToChar { .. }
            | Action::YankToChar { .. }
            | Action::ChangeToChar { .. }
            | Action::DeleteSelection
            | Action::ChangeLine
            | Action::ChangeWord
            // Paste (alters content)
            | Action::PasteAfter
            | Action::PasteBefore
        );
        if needs_snapshot {
            self.with_buffer(|buf| buf.save_undo_snapshot());
        }

        match action {
            Action::Noop => unreachable!(),
            Action::Insert => self.mode = Mode::Insert,
            Action::InsertAppend => {
                self.with_buffer(|buf| buf.move_cursor_right());
                self.mode = Mode::Insert;
            },
            Action::InsertLineStart => {
                self.with_buffer(|buf| buf.move_cursor_line_start());
                self.mode = Mode::Insert;
            },
            Action::InsertLineEnd => {
                self.with_buffer(|buf| buf.move_cursor_line_end());
                self.mode = Mode::Insert;
            },
            Action::InsertNewlineBelow => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_end();
                    buf.insert_newline();
                });
                self.mode = Mode::Insert;
            },
            Action::InsertNewlineAbove => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_start();
                    buf.insert_newline();
                    buf.move_cursor_up();
                });
                self.mode = Mode::Insert;
            },
            Action::MoveLeft => {
                // h — clamped, no line wrap; repeats `count` times
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_left_clamp();
                    }
                });
            },
            Action::MoveRight => {
                // l — clamped, no line wrap; repeats `count` times
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_right_clamp();
                    }
                });
            },
            Action::MoveUp => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_up();
                    }
                });
            },
            Action::MoveDown => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_down();
                    }
                });
            },
            Action::MoveLineStart => {
                self.with_buffer(|buf| buf.move_cursor_line_start());
            },
            Action::MoveFirstNonBlank => {
                self.with_buffer(|buf| buf.move_cursor_first_nonblank());
            },
            Action::MoveLineEnd => {
                // Used by A / InsertLineEnd (cursor goes past last char)
                self.with_buffer(|buf| buf.move_cursor_line_end());
            },
            Action::MoveLineEndNormal => {
                // Used by $ in Normal mode (cursor lands ON last char)
                self.with_buffer(|buf| buf.move_cursor_line_end_normal());
            },
            Action::GotoFileTop => {
                self.with_buffer(|buf| {
                    // `5gg` → jump to line 5 (1-based); bare `gg` → first line
                    if count > 1 {
                        buf.goto_line(count);
                    } else {
                        buf.goto_first_line();
                    }
                });
            },
            Action::GotoFileBottom => {
                self.with_buffer(|buf| {
                    // `5G` → jump to line 5 (1-based); bare `G` → last line
                    if count > 1 {
                        buf.goto_line(count);
                    } else {
                        buf.goto_last_line();
                    }
                });
            },
            Action::MoveWordForward => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_word_forward();
                    }
                });
            },
            Action::MoveWordBackward => {
                self.with_buffer(|buf| {
                    for _ in 0..count {
                        buf.move_cursor_word_backward();
                    }
                });
            },
            Action::Command => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
            },
            Action::Visual => {
                self.with_buffer(|buf| buf.start_selection());
                self.mode = Mode::Visual;
            },
            Action::BufferList => {
                if self.buffers.is_empty() {
                    self.set_status("No buffers open".to_string());
                } else {
                    self.buffer_picker_idx = self.current_buffer_idx;
                    self.mode = Mode::PickBuffer;
                }
            },
            Action::BufferNext => {
                if !self.buffers.is_empty() {
                    self.current_buffer_idx = (self.current_buffer_idx + 1) % self.buffers.len();
                    self.set_status(format!(
                        "Switched to buffer: {}",
                        self.buffers[self.current_buffer_idx].name
                    ));
                }
            },
            Action::BufferPrevious => {
                if !self.buffers.is_empty() {
                    self.current_buffer_idx = if self.current_buffer_idx == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.current_buffer_idx - 1
                    };
                    self.set_status(format!(
                        "Switched to buffer: {}",
                        self.buffers[self.current_buffer_idx].name
                    ));
                }
            },
            Action::BufferClose => {
                if !self.buffers.is_empty() {
                    let buf = &self.buffers[self.current_buffer_idx];
                    if buf.is_modified {
                        self.set_status(
                            "Unsaved changes. Use :bd! to discard and close, or :w to save."
                                .to_string(),
                        );
                    } else {
                        let closing_idx = self.current_buffer_idx;
                        // If closing the focused pane while a split is active, bring the
                        // other pane to focus first so we never close the split's buffer
                        // out from under the cursor.
                        if let Some(other) = self.split.other_idx {
                            if closing_idx == other {
                                // Closing the background pane's buffer — just clear the split.
                                self.split.other_idx = None;
                                self.split.right_focused = false;
                                self.split.highlight_cache = None;
                            } else {
                                // Closing the focused buffer while split is open: swap focus
                                // so the other pane becomes active, then close.
                                self.current_buffer_idx = other;
                                self.split.other_idx = None;
                                self.split.right_focused = false;
                                self.split.highlight_cache = None;
                            }
                        }
                        let closing_buf = &self.buffers[closing_idx];
                        let name = closing_buf.name.clone();
                        let closed_path = closing_buf.file_path.clone();
                        self.buffers.remove(closing_idx);
                        if !self.buffers.is_empty() {
                            self.current_buffer_idx =
                                self.current_buffer_idx.min(self.buffers.len() - 1);
                        }
                        // Stop watching the closed file.
                        if let (Some(ref mut watcher), Some(ref p)) =
                            (&mut self.file_watcher, &closed_path)
                        {
                            let _ = watcher.unwatch(p);
                        }
                        self.set_status(format!("Closed buffer: {}", name));
                    }
                }
            },
            Action::BufferForceClose => {
                if !self.buffers.is_empty() {
                    let closing_idx = self.current_buffer_idx;
                    if let Some(other) = self.split.other_idx {
                        if closing_idx == other {
                            self.split.other_idx = None;
                            self.split.right_focused = false;
                            self.split.highlight_cache = None;
                        } else {
                            self.current_buffer_idx = other;
                            self.split.other_idx = None;
                            self.split.right_focused = false;
                            self.split.highlight_cache = None;
                        }
                    }
                    let force_closing_buf = &self.buffers[closing_idx];
                    let name = force_closing_buf.name.clone();
                    let closed_path = force_closing_buf.file_path.clone();
                    self.buffers.remove(closing_idx);
                    if !self.buffers.is_empty() {
                        self.current_buffer_idx =
                            self.current_buffer_idx.min(self.buffers.len() - 1);
                    }
                    if let (Some(ref mut watcher), Some(ref p)) =
                        (&mut self.file_watcher, &closed_path)
                    {
                        let _ = watcher.unwatch(p);
                    }
                    self.set_status(format!("Closed buffer (discarded): {}", name));
                }
            },
            Action::WindowSplit => {
                if self.buffers.len() < 2 {
                    self.set_status("Need another buffer open — use SPC f f".into());
                } else if self.split.other_idx.is_some() {
                    self.set_status("Split already open — SPC w c to close".into());
                } else {
                    let other = if self.current_buffer_idx == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.current_buffer_idx - 1
                    };
                    self.split.other_idx = Some(other);
                    self.split.right_focused = false;
                }
            },
            Action::WindowFocusNext => {
                if let Some(ref mut other) = self.split.other_idx {
                    std::mem::swap(&mut self.current_buffer_idx, other);
                    self.split.right_focused = !self.split.right_focused;
                }
            },
            Action::WindowClose => {
                self.split.other_idx = None;
                self.split.right_focused = false;
                self.split.highlight_cache = None;
            },
            Action::FileFind => {
                self.scan_files(); // fills file_all
                self.file_query.clear();
                self.refilter_files(); // fills file_list from file_all
                if self.file_list.is_empty() {
                    self.set_status("No files found".to_string());
                } else {
                    self.file_picker_idx = 0;
                    self.mode = Mode::PickFile;
                }
            },
            Action::FileNew => {
                // Enter command mode pre-filled with "e " — user types the path.
                self.command_buffer = "e ".to_string();
                self.mode = Mode::Command;
            },
            Action::FileEditConfig => match crate::config::Config::config_path() {
                Some(path) => self.open_file(&path)?,
                None => self.set_status("Cannot locate config file ($HOME not set)".to_string()),
            },
            Action::FileSave => {
                // Get file path and text before doing LSP operations
                let (file_path, text) = if let Some(buf) = self.current_buffer_mut() {
                    match buf.save() {
                        Ok(()) => (buf.file_path.clone(), buf.lines().join("\n")),
                        Err(e) => {
                            self.set_status(format!("Error: {e}"));
                            return Ok(());
                        },
                    }
                } else {
                    (None, String::new())
                };
                if let Some(ref p) = file_path {
                    self.self_saved.insert(p.clone(), std::time::Instant::now());
                }

                self.set_status("File saved".to_string());

                // Notify LSP about saved document
                if let Some(path) = file_path {
                    let language = LspManager::language_from_path(&path);
                    if let Ok(uri) = LspManager::path_to_uri(&path) {
                        if let Some(client) = self.lsp_manager.get_client(&language) {
                            let _ = client.did_save(uri, Some(text));
                        }
                    }
                }
            },
            Action::Quit => {
                self.check_quit()?;
            },
            Action::LspHover => {
                self.request_hover();
            },
            Action::LspGoToDefinition => {
                self.request_goto_definition();
            },
            Action::LspReferences => {
                self.request_references();
            },
            Action::LspRename => {
                self.set_status("Rename not yet implemented".to_string());
                // TODO: Implement rename workflow
            },
            Action::LspDocumentSymbols => {
                self.request_document_symbols();
            },
            Action::LspNextDiagnostic => {
                self.goto_next_diagnostic();
            },
            Action::LspPrevDiagnostic => {
                self.goto_prev_diagnostic();
            },
            Action::AgentToggle => {
                self.agent_panel.toggle_visible();
                if self.agent_panel.visible {
                    self.mode = Mode::Agent;
                    // Eagerly load models on first show
                    if self.agent_panel.available_models.is_empty() {
                        let preferred = self.config.default_copilot_model.clone();
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                if let Err(e) = self.agent_panel.ensure_models(&preferred).await {
                                    tracing::warn!("Could not fetch model list: {e}");
                                }
                            });
                        });
                    }
                } else {
                    self.mode = Mode::Normal;
                }
            },
            Action::AgentFocus => {
                if !self.agent_panel.visible {
                    self.agent_panel.visible = true;
                }
                self.agent_panel.focus();
                self.mode = Mode::Agent;
                // Eagerly load models on first show
                if self.agent_panel.available_models.is_empty() {
                    let preferred = self.config.default_copilot_model.clone();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            if let Err(e) = self.agent_panel.ensure_models(&preferred).await {
                                tracing::warn!("Could not fetch model list: {e}");
                            }
                        });
                    });
                }
            },
            Action::AgentNewConversation => {
                let model_name = self.agent_panel.selected_model_display().to_string();
                self.agent_panel.new_conversation(&model_name);
                self.set_status(format!("New conversation started · {model_name}"));
            },
            Action::ExplorerToggle => {
                self.file_explorer.toggle_visible();
                if self.file_explorer.visible {
                    self.mode = Mode::Explorer;
                } else {
                    self.mode = Mode::Normal;
                }
            },
            Action::ExplorerFocus => {
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            Action::ExplorerToggleHidden => {
                self.file_explorer.toggle_hidden();
                let status = if self.file_explorer.show_hidden {
                    "Explorer: showing hidden files"
                } else {
                    "Explorer: hiding hidden files"
                };
                self.set_status(status.to_string());
            },
            // ── Git ───────────────────────────────────────────────────────────
            Action::GitOpen => {
                self.open_lazygit()?;
            },
            Action::GitCommitStaged => self.start_commit_msg(true),
            Action::GitCommitLast => self.start_commit_msg(false),
            Action::GitReleaseNotes => self.start_release_notes(),
            // ── Markdown preview ──────────────────────────────────────────────
            Action::MarkdownPreviewToggle => {
                if self.mode == Mode::MarkdownPreview {
                    self.mode = Mode::Normal;
                    self.set_status("Preview closed".to_string());
                } else {
                    self.preview_scroll = 0;
                    self.mode = Mode::MarkdownPreview;
                    self.set_status(
                        "Markdown preview  (Esc/q=back, j/k=scroll, Ctrl+D/U=page)".to_string(),
                    );
                }
            },
            Action::MarkdownOpenBrowser => {
                self.open_markdown_in_browser();
            },
            // ── Diagnostics overlay ───────────────────────────────────────────
            Action::DiagnosticsOpen => {
                self.mode = Mode::Diagnostics;
            },
            Action::DiagnosticsOpenLog => {
                self.open_file(std::path::Path::new("/tmp/forgiven.log"))?;
            },
            // ── Project-wide text search ──────────────────────────────────────
            Action::SearchOpen => {
                self.search_state = SearchState::new();
                self.search_rx = None;
                self.last_search_instant = None;
                self.mode = Mode::Search;
            },
            // ── Edit operations ───────────────────────────────────────────────
            Action::DeleteChar => {
                self.with_buffer(|buf| buf.delete_char_at_cursor());
                self.notify_lsp_change();
            },
            // ── Linewise deletes/yanks (paste creates new rows) ───────────────
            Action::DeleteLine => {
                // `count` lines deleted, e.g. `3dd` removes 3 lines
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_lines(count));
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                }
                self.notify_lsp_change();
            },
            Action::YankLine => {
                // `count` lines yanked, e.g. `3yy` copies 3 lines
                let yanked = self.current_buffer().map(|buf| buf.yank_lines(count));
                if let Some(text) = yanked {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.set_status(format!(
                        "{count} line{} yanked",
                        if count == 1 { "" } else { "s" }
                    ));
                }
            },
            Action::ChangeLine => {
                // `count` lines deleted then enter Insert, e.g. `3cc`
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_lines(count));
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },
            // ── Visual Line mode ─────────────────────────────────────────────
            Action::VisualLine => {
                self.with_buffer(|buf| buf.start_selection_line());
                self.mode = Mode::VisualLine;
            },
            // ── Charwise deletes/yanks (paste inserts inline) ─────────────────
            Action::DeleteToLineEnd => {
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_to_line_end());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
                self.notify_lsp_change();
            },
            Action::DeleteWord => {
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_word());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
                self.notify_lsp_change();
            },
            Action::DeleteToChar { ch, inclusive } => {
                let deleted = self.current_buffer_mut().and_then(|buf| {
                    let target = buf.find_char_forward(ch)?;
                    let end_col = if inclusive { target + 1 } else { target };
                    Some(buf.delete_to_col(end_col))
                });
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
                self.notify_lsp_change();
            },
            Action::YankToChar { ch, inclusive } => {
                let yanked = self.current_buffer().and_then(|buf| {
                    let target = buf.find_char_forward(ch)?;
                    let end_col = if inclusive { target + 1 } else { target };
                    Some(buf.yank_to_col(end_col))
                });
                if let Some(text) = yanked {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                }
            },
            Action::ChangeToChar { ch, inclusive } => {
                let deleted = self.current_buffer_mut().and_then(|buf| {
                    let target = buf.find_char_forward(ch)?;
                    let end_col = if inclusive { target + 1 } else { target };
                    Some(buf.delete_to_col(end_col))
                });
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },
            Action::FindCharForward { ch, inclusive } => {
                self.with_buffer(|buf| {
                    if let Some(target) = buf.find_char_forward(ch) {
                        let col = if inclusive { target } else { target.saturating_sub(1) };
                        buf.move_to_col(col);
                    }
                });
            },
            Action::FindCharBackward { ch, inclusive } => {
                self.with_buffer(|buf| {
                    if let Some(target) = buf.find_char_backward(ch) {
                        let col = if inclusive { target } else { target + 1 };
                        buf.move_to_col(col);
                    }
                });
            },
            Action::YankWord => {
                let yanked = self.current_buffer().map(|buf| buf.yank_word());
                if let Some(text) = yanked {
                    let n = text.chars().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.set_status(format!("{n} chars yanked"));
                }
            },
            Action::YankToLineEnd => {
                let yanked = self.current_buffer().map(|buf| buf.yank_to_line_end());
                if let Some(text) = yanked {
                    let n = text.chars().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.set_status(format!("{n} chars yanked"));
                }
            },
            Action::YankSelection => {
                let yanked = self.current_buffer().and_then(|buf| buf.yank_selection());
                if let Some(text) = yanked {
                    let n = text.chars().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.set_status(format!("{n} chars yanked"));
                }
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },
            Action::DeleteSelection => {
                let deleted = self.current_buffer_mut().and_then(|buf| buf.delete_selection());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Normal;
            },
            Action::ChangeWord => {
                let deleted = self.current_buffer_mut().map(|buf| buf.delete_word());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },
            // ── Paste — dispatch on clipboard type ────────────────────────────
            Action::PasteAfter => {
                if let Some((text, clip_type)) = self.clipboard.clone() {
                    self.with_buffer(|buf| match clip_type {
                        ClipboardType::Linewise => buf.paste_linewise_after(&text),
                        ClipboardType::Charwise => buf.paste_charwise_after(&text),
                    });
                    self.notify_lsp_change();
                }
            },
            Action::PasteBefore => {
                if let Some((text, clip_type)) = self.clipboard.clone() {
                    self.with_buffer(|buf| match clip_type {
                        ClipboardType::Linewise => buf.paste_linewise_before(&text),
                        ClipboardType::Charwise => buf.paste_charwise_before(&text),
                    });
                    self.notify_lsp_change();
                }
            },
            Action::Undo => {
                let did_undo = self.current_buffer_mut().map(|buf| buf.undo()).unwrap_or(false);
                if did_undo {
                    self.notify_lsp_change();
                } else {
                    self.set_status("Already at oldest change".to_string());
                }
            },
            Action::Redo => {
                let did_redo = self.current_buffer_mut().map(|buf| buf.redo()).unwrap_or(false);
                if did_redo {
                    self.notify_lsp_change();
                } else {
                    self.set_status("Already at newest change".to_string());
                }
            },
            Action::InFileSearchStart => {
                self.in_file_search_buffer.clear();
                self.mode = Mode::InFileSearch;
            },
            Action::InFileSearchNext => {
                self.with_buffer(|buf| buf.search_next());
            },
            Action::InFileSearchPrev => {
                self.with_buffer(|buf| buf.search_prev());
            },
        }
        Ok(())
    }

    /// Handle keys in Visual mode
    fn handle_visual_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // ── Exit / cancel ─────────────────────────────────────────────────
            KeyCode::Esc => {
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },

            // ── Yank / delete / change operators ──────────────────────────────
            // y — copy selection to register + system clipboard, back to Normal
            KeyCode::Char('y') => {
                self.execute_action(Action::YankSelection)?;
            },
            // d / x — delete selection into register, back to Normal
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.execute_action(Action::DeleteSelection)?;
            },
            // c — delete selection + enter Insert mode
            KeyCode::Char('c') => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                let deleted = self.current_buffer_mut().and_then(|buf| buf.delete_selection());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Charwise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },

            // ── Motion keys (extend the selection) ────────────────────────────
            KeyCode::Char('h') | KeyCode::Left => {
                self.with_buffer(|buf| {
                    buf.move_cursor_left();
                    buf.update_selection();
                });
            },
            KeyCode::Char('l') | KeyCode::Right => {
                self.with_buffer(|buf| {
                    buf.move_cursor_right();
                    buf.update_selection();
                });
            },
            KeyCode::Char('k') | KeyCode::Up => {
                self.with_buffer(|buf| {
                    buf.move_cursor_up();
                    buf.update_selection();
                });
            },
            KeyCode::Char('j') | KeyCode::Down => {
                self.with_buffer(|buf| {
                    buf.move_cursor_down();
                    buf.update_selection();
                });
            },
            KeyCode::Char('w') => {
                self.with_buffer(|buf| {
                    buf.move_cursor_word_forward();
                    buf.update_selection();
                });
            },
            KeyCode::Char('b') => {
                self.with_buffer(|buf| {
                    buf.move_cursor_word_backward();
                    buf.update_selection();
                });
            },
            KeyCode::Char('0') | KeyCode::Home => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_start();
                    buf.update_selection();
                });
            },
            KeyCode::Char('^') => {
                self.with_buffer(|buf| {
                    buf.move_cursor_first_nonblank();
                    buf.update_selection();
                });
            },
            KeyCode::Char('$') | KeyCode::End => {
                self.with_buffer(|buf| {
                    buf.move_cursor_line_end_normal();
                    buf.update_selection();
                });
            },
            KeyCode::Char('G') => {
                self.with_buffer(|buf| {
                    buf.goto_last_line();
                    buf.update_selection();
                });
            },
            _ => {},
        }
        Ok(())
    }

    /// Handle keys in Visual Line mode (`V`)
    ///
    /// The selection always covers whole lines. `j`/`k` move the cursor and
    /// re-anchor the selection; `y`/`d`/`x` operate on the selected line span.
    fn handle_visual_line_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // ── Exit ──────────────────────────────────────────────────────────
            KeyCode::Esc | KeyCode::Char('V') => {
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },

            // ── Yank selection (linewise) ─────────────────────────────────────
            // `y` — copy selected lines into register + system clipboard, Normal
            KeyCode::Char('y') => {
                let yanked = self.current_buffer().and_then(|buf| buf.yank_selection_lines());
                if let Some(text) = yanked {
                    let n = text.lines().count();
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.set_status(format!("{n} line{} yanked", if n == 1 { "" } else { "s" }));
                }
                self.with_buffer(|buf| buf.clear_selection());
                self.mode = Mode::Normal;
            },

            // ── Delete / change selection (linewise) ─────────────────────────
            // `d` / `x` — remove selected lines, store in register, Normal
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                let deleted =
                    self.current_buffer_mut().and_then(|buf| buf.delete_selection_lines());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Normal;
            },

            // `c` — remove selected lines + enter Insert
            KeyCode::Char('c') => {
                self.with_buffer(|buf| buf.save_undo_snapshot());
                let deleted =
                    self.current_buffer_mut().and_then(|buf| buf.delete_selection_lines());
                if let Some(text) = deleted {
                    self.sync_system_clipboard(&text);
                    self.clipboard = Some((text, ClipboardType::Linewise));
                    self.notify_lsp_change();
                }
                self.mode = Mode::Insert;
            },

            // ── Motion keys (extend the line selection) ───────────────────────
            KeyCode::Char('j') | KeyCode::Down => {
                self.with_buffer(|buf| {
                    buf.move_cursor_down();
                    buf.update_selection_line();
                });
            },
            KeyCode::Char('k') | KeyCode::Up => {
                self.with_buffer(|buf| {
                    buf.move_cursor_up();
                    buf.update_selection_line();
                });
            },
            KeyCode::Char('G') => {
                self.with_buffer(|buf| {
                    buf.goto_last_line();
                    buf.update_selection_line();
                });
            },
            KeyCode::Char('g') => {
                // gg — go to first line (we can't use pending_key here easily,
                // so a single `g` press jumps to the top — matches common muscle
                // memory for `Vgg` select-to-top).
                self.with_buffer(|buf| {
                    buf.goto_first_line();
                    buf.update_selection_line();
                });
            },

            _ => {},
        }
        Ok(())
    }

    /// Handle keys in PickBuffer mode
    fn handle_pick_buffer_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status_message = None;
            },
            KeyCode::Enter => {
                self.current_buffer_idx = self.buffer_picker_idx;
                self.mode = Mode::Normal;
                self.status_message = None;
            },
            KeyCode::Up | KeyCode::Char('k') => {
                if self.buffer_picker_idx > 0 {
                    self.buffer_picker_idx -= 1;
                }
            },
            KeyCode::Down | KeyCode::Char('j') => {
                if self.buffer_picker_idx + 1 < self.buffers.len() {
                    self.buffer_picker_idx += 1;
                }
            },
            _ => {},
        }
        Ok(())
    }

    /// Handle keys in PickFile mode (fuzzy search).
    fn handle_pick_file_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.file_query.clear();
                self.status_message = None;
            },
            KeyCode::Enter => {
                if let Some((path, _)) = self.file_list.get(self.file_picker_idx) {
                    if !Self::is_picker_sentinel(path) {
                        let path_clone = path.clone();
                        self.file_query.clear();
                        self.mode = Mode::Normal;
                        self.open_file(&path_clone)?;
                    }
                }
            },
            KeyCode::Up => {
                let mut idx = self.file_picker_idx;
                while idx > 0 {
                    idx -= 1;
                    if !Self::is_picker_sentinel(&self.file_list[idx].0) {
                        self.file_picker_idx = idx;
                        break;
                    }
                }
            },
            KeyCode::Down => {
                let mut idx = self.file_picker_idx;
                while idx + 1 < self.file_list.len() {
                    idx += 1;
                    if !Self::is_picker_sentinel(&self.file_list[idx].0) {
                        self.file_picker_idx = idx;
                        break;
                    }
                }
            },
            KeyCode::Backspace => {
                self.file_query.pop();
                self.refilter_files();
            },
            KeyCode::Char(c) => {
                self.file_query.push(c);
                self.refilter_files();
            },
            _ => {},
        }
        Ok(())
    }

    /// Handle keys while the agent panel is focused.
    fn handle_agent_mode(&mut self, key: KeyEvent) -> Result<()> {
        // If the agent is waiting for a question answer, intercept all keys for the dialog.
        if self.agent_panel.asking_user.is_some() {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.agent_panel.move_question_selection(-1);
                },
                KeyCode::Down | KeyCode::Char('j') => {
                    self.agent_panel.move_question_selection(1);
                },
                KeyCode::Enter => {
                    self.agent_panel.confirm_user_question();
                },
                KeyCode::Esc => {
                    self.agent_panel.cancel_user_question();
                },
                _ => {},
            }
            return Ok(());
        }

        // Ctrl+P file-context picker: intercept all keys while the overlay is open.
        if self.agent_panel.at_picker.is_some() {
            return self.handle_at_picker_key(key);
        }

        // Slash-command autocomplete: intercept navigation keys when the menu is visible.
        if self.agent_panel.slash_menu.is_some() {
            match key.code {
                KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                    self.agent_panel.move_slash_selection(1);
                    return Ok(());
                },
                KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
                    self.agent_panel.move_slash_selection(-1);
                    return Ok(());
                },
                KeyCode::Enter => {
                    self.agent_panel.complete_slash_selection();
                    return Ok(());
                },
                KeyCode::Esc => {
                    self.agent_panel.slash_menu = None;
                    return Ok(());
                },
                _ => {}, // fall through to normal input handling
            }
        }

        match key.code {
            // Esc — blur panel, return focus to editor.
            KeyCode::Esc => {
                self.agent_panel.blur();
                self.mode = Mode::Normal;
            },
            // Tab — toggle focus back to editor without closing.
            KeyCode::Tab => {
                self.agent_panel.blur();
                self.mode = Mode::Normal;
            },
            // Alt+Enter — insert a newline into the multi-line input.
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                self.agent_panel.input_newline();
                self.agent_panel.update_slash_menu();
            },
            // Enter — submit the input.
            KeyCode::Enter => {
                // Snapshot current buffer content as context, including its path
                // so the model knows which file is open and can reference it directly.
                let context = self.current_buffer().map(|buf| {
                    let path_header =
                        buf.file_path.as_deref().and_then(|p| p.to_str()).unwrap_or(&buf.name);
                    format!("File: {path_header}\n\n{}", buf.lines().join("\n"))
                });
                // Project root for tool sandboxing.
                let project_root =
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                // Submit is async; spawn a task and let the stream_rx handle tokens.
                let panel = &mut self.agent_panel;
                let max_rounds = self.config.max_agent_rounds;
                let warning_threshold = self.config.agent_warning_threshold;
                let preferred_model = self.config.default_copilot_model.clone();
                // We need a blocking submit here.  Use a one-shot channel via block_in_place
                // or simply call submit synchronously via tokio::task::block_in_place.
                // Since we are inside an async context, we use a local async block.
                let fut = panel.submit(
                    context,
                    project_root,
                    max_rounds,
                    warning_threshold,
                    &preferred_model,
                );
                // We can't .await inside handle_key (sync fn), so we use try_join on
                // the runtime directly.  The cleanest way: push to a queue and process
                // in the async run() loop.  For now use tokio::task::block_in_place.
                let submit_err = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        match fut.await {
                            Ok(()) => None,
                            Err(e) => {
                                tracing::warn!("Agent submit error: {}", e);
                                Some(e.to_string())
                            },
                        }
                    })
                });
                if let Some(e) = submit_err {
                    self.set_status(format!("Agent error: {e}"));
                }
                self.agent_panel.update_slash_menu();
            },
            // Backspace — delete last input character.
            KeyCode::Backspace => {
                self.agent_panel.input_backspace();
                self.agent_panel.update_slash_menu();
            },
            // Scroll history.
            KeyCode::Up => self.agent_panel.scroll_up(),
            KeyCode::Down => self.agent_panel.scroll_down(),
            // Ctrl+T — cycle through available models.
            // Note: Ctrl+M = Enter (0x0D) in all terminals and cannot be used here.
            // Ctrl+T (0x14) is safe in raw mode and not used by this editor.
            // On first press, fetches the live model list from the Copilot API.
            // Ctrl+C — abort the running agentic loop (stream + tool calls).
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.agent_panel.stream_rx.is_some() {
                    self.agent_panel.cancel_stream();
                    self.set_status("Agent stopped".to_string());
                }
            },
            // Ctrl+K — copy next code block from the last reply (cycles through all blocks).
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(reply) = self.agent_panel.last_assistant_reply() {
                    let blocks = crate::agent::AgentPanel::extract_code_blocks(&reply);
                    if blocks.is_empty() {
                        self.set_status("No code blocks in last reply".to_string());
                    } else {
                        let idx = self.agent_panel.code_block_idx % blocks.len();
                        self.sync_system_clipboard(&blocks[idx]);
                        self.set_status(format!(
                            "Code block {}/{} copied  (Ctrl+K for next)",
                            idx + 1,
                            blocks.len()
                        ));
                        self.agent_panel.code_block_idx =
                            (self.agent_panel.code_block_idx + 1) % blocks.len();
                    }
                } else {
                    self.set_status("No reply to copy".to_string());
                }
            },
            // Ctrl+M — open the next mermaid diagram from the last reply in the browser.
            // Auto-fixes unquoted parentheses in node labels (common AI generation bug).
            // Cycles through multiple diagrams; resets on new reply.
            KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_mermaid_in_browser();
            },
            // Ctrl+Y — yank the full last reply to the system clipboard.
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = self.agent_panel.last_assistant_reply() {
                    let len = text.lines().count();
                    self.sync_system_clipboard(&text);
                    self.set_status(format!("Copied {} lines to clipboard", len));
                } else {
                    self.set_status("No reply to copy".to_string());
                }
            },
            // Ctrl+A — open apply-diff overlay for the last code block in the reply.
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some((path_hint, proposed_code)) = self.agent_panel.get_apply_candidate() {
                    let cwd =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    let (resolved_path, current_content) = if let Some(hint) = path_hint {
                        let abs = cwd.join(&hint);
                        let content = self
                            .buffers
                            .iter()
                            .find(|b| {
                                b.file_path
                                    .as_ref()
                                    .map(|fp| {
                                        fp.canonicalize().unwrap_or_else(|_| fp.clone())
                                            == abs.canonicalize().unwrap_or_else(|_| abs.clone())
                                    })
                                    .unwrap_or(false)
                            })
                            .map(|b| b.lines().join("\n"))
                            .or_else(|| {
                                if abs.exists() {
                                    std::fs::read_to_string(&abs).ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_default();
                        (Some(abs), content)
                    } else {
                        let (path, content) = self
                            .current_buffer()
                            .map(|b| (b.file_path.clone(), b.lines().join("\n")))
                            .unwrap_or_default();
                        (path, content)
                    };
                    let old: Vec<String> = current_content.lines().map(str::to_string).collect();
                    let new: Vec<String> = proposed_code.lines().map(str::to_string).collect();
                    self.apply_diff.lines = lcs_diff(&old, &new);
                    self.apply_diff.path = resolved_path;
                    self.apply_diff.content = Some(proposed_code);
                    self.apply_diff.scroll = 0;
                    self.mode = Mode::ApplyDiff;
                } else {
                    self.set_status("No code block in latest reply to apply".to_string());
                }
            },
            // Ctrl+P — open the file-context picker (attach a file to agent message).
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_at_picker();
            },
            // Ctrl+V — paste from clipboard (image-first, then text fallback).
            // On macOS Cmd+V triggers bracketed paste (text only via Event::Paste);
            // Ctrl+V is passed to the app and allows us to read images via arboard.
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                use crate::agent::AgentPanel;
                match AgentPanel::try_paste_image() {
                    Ok(Some(img)) => {
                        let w = img.width;
                        let h = img.height;
                        self.agent_panel.image_blocks.push(img);
                        self.set_status(format!("Image pasted ({w}x{h})"));
                    },
                    Ok(None) => {
                        // No image — try text from clipboard.
                        match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
                            Ok(text) if !text.is_empty() => {
                                self.handle_paste(text)?;
                            },
                            _ => {
                                self.set_status("Clipboard empty".to_string());
                            },
                        }
                    },
                    Err(e) => {
                        self.set_status(format!("Image paste failed: {e}"));
                    },
                }
            },
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Eagerly load models if not yet fetched (brief one-time network call).
                let was_empty = self.agent_panel.available_models.is_empty();
                if was_empty {
                    self.set_status("Loading model list…".to_string());
                    let preferred = self.config.default_copilot_model.clone();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            if let Err(e) = self.agent_panel.ensure_models(&preferred).await {
                                tracing::warn!("Could not fetch model list: {e}");
                            }
                        });
                    });
                    // First press: just confirm the config-preferred model; don't advance past it.
                } else {
                    // Subsequent presses: cycle to next model and persist the choice.
                    self.agent_panel.cycle_model();
                    let model_id = self.agent_panel.selected_model_id().to_string();
                    let model_name = self.agent_panel.selected_model_display().to_string();
                    self.config.default_copilot_model = model_id.clone();
                    if let Err(e) = self.config.save() {
                        tracing::warn!("Failed to save config: {e}");
                    }
                    // Clear conversation history so the new model gets a clean context.
                    self.agent_panel.new_conversation(&model_name);
                }
                let model_name = self.agent_panel.selected_model_display().to_string();
                let n = self.agent_panel.available_models.len();
                let idx = self.agent_panel.selected_model + 1;

                self.set_status(format!(
                    "Agent model → {model_name}  [{idx}/{n}]  (Ctrl+T to cycle)"
                ));
            },
            // Ctrl+Shift+T — refresh model list from API (picks up new releases).
            KeyCode::Char('T') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_status("Refreshing model list from API…".to_string());
                let preferred = self.config.default_copilot_model.clone();
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        if let Err(e) = self.agent_panel.refresh_models(&preferred).await {
                            tracing::warn!("Could not refresh model list: {e}");
                            self.set_status(format!("Failed to refresh models: {e}"));
                        } else {
                            let model_name = self.agent_panel.selected_model_display().to_string();
                            let n = self.agent_panel.available_models.len();
                            self.set_status(format!(
                                "Refreshed {n} models, selected: {model_name}"
                            ));
                        }
                    });
                });
            },
            // Regular characters — handle special agent commands before appending to input.
            KeyCode::Char(ch) => {
                // If awaiting continuation, 'y' approves and 'n' denies.
                if self.agent_panel.awaiting_continuation {
                    match ch {
                        'y' | 'Y' => {
                            self.agent_panel.approve_continuation();
                            self.set_status("Continuing agent work...".to_string());
                        },
                        'n' | 'N' => {
                            self.agent_panel.deny_continuation();
                            self.set_status("Agent stopped by user".to_string());
                        },
                        _ => {
                            // Ignore other keys when awaiting continuation
                        },
                    }
                    return Ok(());
                }

                // All other characters type into the input box.
                // (Apply-diff, copy code block, and yank-reply moved to Ctrl+A / Ctrl+K / Ctrl+Y
                // so single letters never intercept the first character of a message.)
                self.agent_panel.input_char(ch);
                self.agent_panel.update_slash_menu();
            },
            _ => {},
        }
        Ok(())
    }

    // ── Paste handling ─────────────────────────────────────────────────────────

    /// Handle a bracketed-paste event.
    ///
    /// In Agent mode newlines are preserved so multi-line pastes work correctly.
    /// The user still presses Enter to send.
    fn handle_paste(&mut self, text: String) -> Result<()> {
        if self.mode == Mode::Agent {
            // Store the paste as a block; the UI shows a compact summary line
            // ("⎘ Pasted N lines") and the full content is sent with the message.
            let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
            let line_count = normalised.lines().count();
            self.agent_panel.pasted_blocks.push((normalised, line_count));
        } else if self.mode == Mode::Insert {
            // In insert mode, paste the text as-is into the current buffer.
            let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
            self.with_buffer(|buf| buf.insert_text_block(&normalised));
        }
        Ok(())
    }

    // ── Explorer mode key handling ─────────────────────────────────────────────

    fn handle_explorer_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => {
                // Blur explorer, return to editor (keep panel visible)
                self.show_file_info = false;
                self.file_explorer.blur();
                self.mode = Mode::Normal;
            },
            KeyCode::Up | KeyCode::Char('k') => {
                self.file_explorer.move_up();
            },
            KeyCode::Down | KeyCode::Char('j') => {
                self.file_explorer.move_down();
            },
            KeyCode::Enter | KeyCode::Char('l') => {
                let idx = self.file_explorer.cursor_idx;
                let selected = self.file_explorer.selected_path();
                if let Some(path) = selected {
                    if path.is_dir() {
                        self.file_explorer.toggle_node_at(idx);
                    } else {
                        // Open the file and return focus to editor
                        self.file_explorer.blur();
                        self.mode = Mode::Normal;
                        self.open_file(&path)?;
                    }
                }
            },
            // h — toggle hidden files visibility
            KeyCode::Char('h') => {
                self.file_explorer.toggle_hidden();
                let status = if self.file_explorer.show_hidden {
                    "Explorer: showing hidden files"
                } else {
                    "Explorer: hiding hidden files"
                };
                self.set_status(status.to_string());
            },
            // n — new file: pre-fill command mode with "e <dir>/" so the user
            //     only needs to type the filename and press Enter.
            KeyCode::Char('n') => {
                // Resolve the target directory: selected dir, or parent of selected file,
                // or fall back to the explorer root.
                let target_dir = self
                    .file_explorer
                    .selected_path()
                    .map(|p| {
                        if p.is_dir() {
                            p
                        } else {
                            p.parent()
                                .map(|x| x.to_path_buf())
                                .unwrap_or(self.file_explorer.root_path.clone())
                        }
                    })
                    .unwrap_or_else(|| self.file_explorer.root_path.clone());

                // Build a project-relative prefix for readability.
                let rel = target_dir
                    .strip_prefix(&self.file_explorer.root_path)
                    .unwrap_or(&target_dir)
                    .to_string_lossy()
                    .to_string();

                let prefill = if rel.is_empty() { "e ".to_string() } else { format!("e {}/", rel) };

                self.file_explorer.blur();
                self.command_buffer = prefill;
                self.mode = Mode::Command;
            },
            // r — rename selected entry (falls back to reload when nothing is selected).
            // R — reload / refresh the file tree from disk.
            KeyCode::Char('r') => {
                if let Some(path) = self.file_explorer.selected_path() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    self.rename_source = Some(path);
                    self.rename_buffer = name;
                    self.file_explorer.blur();
                    self.mode = Mode::RenameFile;
                } else {
                    self.file_explorer.reload();
                    self.set_status("Explorer refreshed".to_string());
                }
            },
            KeyCode::Char('R') => {
                self.file_explorer.reload();
                self.set_status("Explorer refreshed".to_string());
            },
            // d — delete selected entry (with confirmation popup).
            KeyCode::Char('d') => {
                if let Some(path) = self.file_explorer.selected_path() {
                    self.delete_confirm_path = Some(path);
                    self.file_explorer.blur();
                    self.mode = Mode::DeleteFile;
                }
            },
            // m — create a new folder inside the selected directory (or file's parent).
            KeyCode::Char('m') => {
                let target_dir = self
                    .file_explorer
                    .selected_path()
                    .map(|p| {
                        if p.is_dir() {
                            p
                        } else {
                            p.parent()
                                .map(|x| x.to_path_buf())
                                .unwrap_or(self.file_explorer.root_path.clone())
                        }
                    })
                    .unwrap_or_else(|| self.file_explorer.root_path.clone());
                self.new_folder_parent = Some(target_dir);
                self.new_folder_buffer.clear();
                self.file_explorer.blur();
                self.mode = Mode::NewFolder;
            },
            // i — toggle file-info popup for the selected entry.
            // Navigation (j/k) automatically refreshes the popup by re-computing
            // FileInfoData from the new cursor position on the next frame.
            KeyCode::Char('i') => {
                self.show_file_info = !self.show_file_info;
            },
            _ => {},
        }
        Ok(())
    }

    // ── Rename popup mode key handling ───────────────────────────────────────

    fn handle_rename_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.rename_source = None;
                self.rename_buffer.clear();
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            KeyCode::Enter => {
                self.do_rename()?;
            },
            KeyCode::Backspace => {
                self.rename_buffer.pop();
            },
            KeyCode::Char(c) if c != '/' && c != '\\' => {
                self.rename_buffer.push(c);
            },
            _ => {},
        }
        Ok(())
    }

    fn do_rename(&mut self) -> Result<()> {
        let new_name = self.rename_buffer.trim().to_string();
        if new_name.is_empty() {
            self.set_status("Rename cancelled: empty name".into());
            self.rename_source = None;
            self.rename_buffer.clear();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            return Ok(());
        }

        if let Some(src) = self.rename_source.take() {
            let dst = src
                .parent()
                .map(|p| p.join(&new_name))
                .ok_or_else(|| anyhow::anyhow!("No parent directory"))?;

            if dst.exists() {
                self.set_status(format!("Rename failed: '{}' already exists", new_name));
                self.rename_source = Some(src); // keep popup open so user can retry
                return Ok(());
            }

            std::fs::rename(&src, &dst)?;

            // Update any open buffer whose path matches the old path
            for buf in &mut self.buffers {
                if buf.file_path.as_deref() == Some(&src) {
                    buf.file_path = Some(dst.clone());
                }
            }

            // Refresh the explorer tree
            self.file_explorer.reload();

            let old_name = src.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            self.rename_buffer.clear();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            self.set_status(format!("Renamed '{}' → '{}'", old_name, new_name));
        }
        Ok(())
    }

    // ── Delete confirmation popup mode key handling ───────────────────────────

    fn handle_delete_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.do_delete()?;
            },
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.delete_confirm_path = None;
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            _ => {},
        }
        Ok(())
    }

    // ── Binary file popup mode key handling ───────────────────────────────────

    fn handle_binary_file_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('o') => {
                if let Some(ref path) = self.binary_file_path {
                    #[cfg(target_os = "macos")]
                    {
                        std::process::Command::new("open").arg(path).spawn().ok();
                    }
                    #[cfg(target_os = "linux")]
                    {
                        std::process::Command::new("xdg-open").arg(path).spawn().ok();
                    }
                    self.set_status("Opened in default app".to_string());
                }
                self.binary_file_path = None;
                self.mode = Mode::Normal;
            },
            KeyCode::Esc | KeyCode::Char('q') => {
                self.binary_file_path = None;
                self.mode = Mode::Normal;
            },
            _ => {},
        }
        Ok(())
    }

    fn do_delete(&mut self) -> Result<()> {
        if let Some(path) = self.delete_confirm_path.take() {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }

            // Close any open buffers under the deleted path (handles dirs too)
            self.buffers.retain(|buf| buf.file_path.as_ref().is_none_or(|p| !p.starts_with(&path)));
            if self.current_buffer_idx >= self.buffers.len() {
                self.current_buffer_idx = self.buffers.len().saturating_sub(1);
            }

            self.file_explorer.reload();

            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            self.set_status(format!("Deleted '{}'", name));
        }
        Ok(())
    }

    // ── New folder popup mode key handling ───────────────────────────────────

    fn handle_new_folder_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.new_folder_buffer.clear();
                self.new_folder_parent = None;
                self.file_explorer.focus();
                self.mode = Mode::Explorer;
            },
            KeyCode::Enter => {
                self.do_create_folder()?;
            },
            KeyCode::Backspace => {
                self.new_folder_buffer.pop();
            },
            KeyCode::Char(c) if c != '/' && c != '\\' => {
                self.new_folder_buffer.push(c);
            },
            _ => {},
        }
        Ok(())
    }

    fn do_create_folder(&mut self) -> Result<()> {
        let name = self.new_folder_buffer.trim().to_string();
        if name.is_empty() {
            self.set_status("Create folder cancelled: empty name".into());
            self.new_folder_buffer.clear();
            self.new_folder_parent = None;
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            return Ok(());
        }

        if let Some(parent) = self.new_folder_parent.take() {
            let new_dir = parent.join(&name);
            if new_dir.exists() {
                self.set_status(format!("Create folder failed: '{}' already exists", name));
                self.new_folder_parent = Some(parent); // keep popup open for retry
                return Ok(());
            }

            std::fs::create_dir_all(&new_dir)?;
            self.file_explorer.reload();

            self.new_folder_buffer.clear();
            self.file_explorer.focus();
            self.mode = Mode::Explorer;
            self.set_status(format!("Created folder '{}'", name));
        }
        Ok(())
    }

    // ── Apply-diff mode ───────────────────────────────────────────────────────

    fn clear_apply_diff(&mut self) {
        self.apply_diff.path = None;
        self.apply_diff.content = None;
        self.apply_diff.lines.clear();
        self.apply_diff.scroll = 0;
    }

    fn handle_apply_diff_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => self.do_apply_diff()?,
            KeyCode::Char('n') | KeyCode::Esc => {
                self.clear_apply_diff();
                self.agent_panel.focus();
                self.mode = Mode::Agent;
                self.set_status("Apply discarded".to_string());
            },
            KeyCode::Char('j') | KeyCode::Down => {
                self.apply_diff.scroll = self.apply_diff.scroll.saturating_add(1);
            },
            KeyCode::Char('k') | KeyCode::Up => {
                self.apply_diff.scroll = self.apply_diff.scroll.saturating_sub(1);
            },
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.apply_diff.scroll = self.apply_diff.scroll.saturating_add(20);
            },
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.apply_diff.scroll = self.apply_diff.scroll.saturating_sub(20);
            },
            _ => {},
        }
        Ok(())
    }

    fn do_apply_diff(&mut self) -> Result<()> {
        let content = match self.apply_diff.content.take() {
            Some(c) => c,
            None => {
                self.clear_apply_diff();
                self.mode = Mode::Normal;
                return Ok(());
            },
        };
        let path = self.apply_diff.path.take();
        self.apply_diff.lines.clear();
        self.apply_diff.scroll = 0;
        match &path {
            Some(p) => {
                if let Some(parent) = p.parent() {
                    if !parent.exists() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                let to_write =
                    if content.ends_with('\n') { content.clone() } else { format!("{content}\n") };
                std::fs::write(p, &to_write)?;
                self.self_saved.insert(p.clone(), std::time::Instant::now());
                let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                for buf in &mut self.buffers {
                    let matches = buf
                        .file_path
                        .as_ref()
                        .map(|fp| fp.canonicalize().unwrap_or_else(|_| fp.clone()) == canon)
                        .unwrap_or(false);
                    if matches {
                        let _ = buf.reload_from_disk();
                    }
                }
                self.mode = Mode::Normal;
                self.set_status(format!("Applied to {}", p.display()));
            },
            None => {
                let new_lines: Vec<String> = content.lines().map(str::to_string).collect();
                self.with_buffer(|buf| buf.replace_all_lines(new_lines));
                self.mode = Mode::Normal;
                self.set_status("Applied to unsaved buffer".to_string());
            },
        }
        Ok(())
    }

    // ── In-file search mode key handling ─────────────────────────────────────

    fn handle_in_file_search_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.in_file_search_buffer.clear();
            },
            KeyCode::Enter => {
                let pattern = self.in_file_search_buffer.clone();
                self.in_file_search_buffer.clear();
                self.mode = Mode::Normal;
                let count = self.with_buffer(|buf| buf.set_search_pattern(pattern)).unwrap_or(0);
                if count == 0 {
                    self.set_status("Pattern not found".to_string());
                } else {
                    self.set_status(format!("{} match(es) found", count));
                }
            },
            KeyCode::Char(c) => {
                self.in_file_search_buffer.push(c);
            },
            KeyCode::Backspace => {
                self.in_file_search_buffer.pop();
            },
            _ => {},
        }
        Ok(())
    }

    // ── Markdown preview mode key handling ────────────────────────────────────

    fn handle_preview_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // Esc / q — exit preview, return to Normal
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
            },

            // j / Down — scroll down one line
            KeyCode::Char('j') | KeyCode::Down => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
            },

            // k / Up — scroll up one line
            KeyCode::Char('k') | KeyCode::Up => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
            },

            // Ctrl+D — scroll down half-page (10 lines)
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.preview_scroll = self.preview_scroll.saturating_add(10);
            },

            // Ctrl+U — scroll up half-page (10 lines)
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.preview_scroll = self.preview_scroll.saturating_sub(10);
            },

            // g — jump to top
            KeyCode::Char('g') => {
                self.preview_scroll = 0;
            },

            // G — jump to bottom (approximate — capped in render())
            KeyCode::Char('G') => {
                self.preview_scroll = usize::MAX / 2; // capped by render()
            },

            _ => {},
        }
        Ok(())
    }

    // ── Search mode key handling ───────────────────────────────────────────────

    fn handle_search_mode(&mut self, key: KeyEvent) -> Result<()> {
        use crate::search::SearchFocus;
        match key.code {
            // Esc — close the search overlay, return to Normal.
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.search_rx = None;
                self.last_search_instant = None;
            },

            // Enter — open the selected result at the matched line.
            KeyCode::Enter => {
                if let Some(result) = self.search_state.selected_result() {
                    let path = result.path.clone();
                    let line = result.line;
                    self.mode = Mode::Normal;
                    self.search_rx = None;
                    self.last_search_instant = None;
                    self.open_file(&path)?;
                    self.with_buffer(|buf| buf.goto_line(line + 1)); // goto_line expects 1-based
                }
            },

            // Tab — switch focus between query and glob fields.
            KeyCode::Tab => {
                self.search_state.focus = match self.search_state.focus {
                    SearchFocus::Query => SearchFocus::Glob,
                    SearchFocus::Glob => SearchFocus::Query,
                };
            },

            // Navigation within results list.
            KeyCode::Up | KeyCode::Char('k') => {
                self.search_state.select_up();
            },
            KeyCode::Down | KeyCode::Char('j') => {
                self.search_state.select_down();
            },

            // Text editing in the focused field.
            KeyCode::Backspace => {
                match self.search_state.focus {
                    SearchFocus::Query => {
                        self.search_state.query.pop();
                    },
                    SearchFocus::Glob => {
                        self.search_state.glob.pop();
                    },
                }
                self.on_search_input_changed();
            },
            KeyCode::Char(c) => {
                match self.search_state.focus {
                    SearchFocus::Query => {
                        self.search_state.query.push(c);
                    },
                    SearchFocus::Glob => {
                        self.search_state.glob.push(c);
                    },
                }
                self.on_search_input_changed();
            },

            _ => {},
        }
        Ok(())
    }

    /// Called whenever the query or glob field changes — resets the debounce timer
    /// and cancels any in-flight search so only the settled value is searched.
    fn on_search_input_changed(&mut self) {
        self.last_search_instant = Some(Instant::now());
        self.search_state.status = SearchStatus::Running;
        self.search_rx = None; // cancel previous in-flight request
    }

    /// Spawn a tokio task that runs ripgrep and delivers results via oneshot channel.
    fn fire_search(&mut self) {
        let query = self.search_state.query.clone();
        let glob = self.search_state.glob.clone();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let (tx, rx) = oneshot::channel();
        self.search_rx = Some(rx);
        self.search_state.status = SearchStatus::Running;
        tokio::spawn(async move {
            let result = run_search(&query, &glob, &cwd).await;
            let _ = tx.send(result);
        });
    }

    // ── Fuzzy file search ──────────────────────────────────────────────────────

    /// Score `query` against `candidate` using a subsequence-match algorithm.
    /// Returns `None` if not all query chars appear in order in the candidate.
    /// Returns `Some((score, match_indices))` otherwise; higher score = better match.
    fn fuzzy_score(query: &str, candidate: &str) -> Option<(i64, Vec<usize>)> {
        if query.is_empty() {
            return Some((0, vec![]));
        }
        let q_chars: Vec<char> = query.to_lowercase().chars().collect();
        let c_chars: Vec<char> = candidate.to_lowercase().chars().collect();

        // Subsequence scan — find the first left-to-right match
        let mut indices = Vec::with_capacity(q_chars.len());
        let mut qi = 0;
        for (ci, &cc) in c_chars.iter().enumerate() {
            if qi < q_chars.len() && cc == q_chars[qi] {
                indices.push(ci);
                qi += 1;
            }
        }
        if qi < q_chars.len() {
            return None; // not all query chars appeared
        }

        let mut score: i64 = 0;

        // Bonus: consecutive matched characters (runs feel like exact substrings)
        for i in 1..indices.len() {
            if indices[i] == indices[i - 1] + 1 {
                score += 10;
            }
        }

        // Bonus: match starts right after a path separator or word boundary
        for &idx in &indices {
            let prev = if idx == 0 { '/' } else { c_chars[idx - 1] };
            if matches!(prev, '/' | '\\' | '_' | '-' | '.') {
                score += 8;
            }
        }

        // Bonus: first matched char appears late in the path (filename > directory)
        if let Some(&first) = indices.first() {
            // Reward matches that are in the filename portion (after the last /)
            let last_sep =
                c_chars.iter().rposition(|&c| c == '/' || c == '\\').map(|p| p + 1).unwrap_or(0);
            if first >= last_sep {
                score += 15;
            }
        }

        // Penalty: longer paths score slightly lower (prefer direct matches)
        score -= candidate.len() as i64 / 6;

        Some((score, indices))
    }

    /// Rebuild `file_list` from `file_all` applying the current `file_query`.
    /// Results are sorted by fuzzy score descending; `file_picker_idx` is clamped.
    fn refilter_files(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        if self.file_query.is_empty() {
            // No query → show recent files (scoped to cwd) first, then all project files.
            let recents: Vec<PathBuf> = self
                .recent_files
                .iter()
                .filter(|p| p.exists() && p.starts_with(&cwd))
                .cloned()
                .collect();

            let mut result: Vec<(PathBuf, Vec<usize>)> = Vec::new();

            if !recents.is_empty() {
                // PathBuf::new() (empty)  → "─── Recent ───" header sentinel.
                // PathBuf::from("\x01")   → closing divider sentinel.
                result.push((PathBuf::new(), vec![]));
                for p in &recents {
                    result.push((p.clone(), vec![]));
                }
                result.push((PathBuf::from("\x01"), vec![]));
            }

            for p in &self.file_all {
                if !recents.contains(p) {
                    result.push((p.clone(), vec![]));
                }
            }

            self.file_list = result;
            // Place cursor on the first recent file (index 1, skipping the sentinel header).
            self.file_picker_idx = if recents.is_empty() { 0 } else { 1 };
            return;
        } else {
            let mut scored: Vec<(i64, PathBuf, Vec<usize>)> = self
                .file_all
                .iter()
                .filter_map(|p| {
                    let display = p.strip_prefix(&cwd).unwrap_or(p).to_string_lossy().to_string();
                    Self::fuzzy_score(&self.file_query, &display)
                        .map(|(score, idxs)| (score, p.clone(), idxs))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            self.file_list = scored.into_iter().map(|(_, p, idxs)| (p, idxs)).collect();
        }

        // Clamp selection index
        if self.file_list.is_empty() {
            self.file_picker_idx = 0;
        } else {
            self.file_picker_idx = self.file_picker_idx.min(self.file_list.len() - 1);
        }
    }

    // ── File-picker helpers ────────────────────────────────────────────────────

    /// Returns true for the synthetic sentinel entries injected into `file_list`:
    /// - `PathBuf::new()`       → "─── Recent ───" section header
    /// - `PathBuf::from("\x01")` → closing divider after the recent section
    #[inline]
    fn is_picker_sentinel(path: &std::path::Path) -> bool {
        path.as_os_str().is_empty() || path.to_str() == Some("\x01")
    }

    // ── Recent files persistence ───────────────────────────────────────────────

    fn recents_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
        PathBuf::from(home).join(".local/share/forgiven/recent_files.txt")
    }

    fn load_recents() -> Vec<PathBuf> {
        let Ok(content) = std::fs::read_to_string(Self::recents_path()) else {
            return vec![];
        };
        content
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .take(5)
            .collect()
    }

    fn save_recents(&self) -> Result<()> {
        let path = Self::recents_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = self
            .recent_files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(path, text)?;
        Ok(())
    }

    fn scan_files(&mut self) {
        self.file_all.clear();
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.scan_directory(&current_dir, 0);
        self.file_all.sort();
    }

    /// Recursively scan a directory for files
    fn scan_directory(&mut self, dir: &PathBuf, depth: usize) {
        // Limit recursion depth to avoid scanning too deep
        if depth > 5 {
            return;
        }

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Skip hidden files, common build dirs, and IDE folders
            if file_name.starts_with('.')
                || file_name == "target"
                || file_name == "node_modules"
                || file_name == "dist"
                || file_name == "build"
            {
                continue;
            }

            if path.is_file() {
                // Skip binary and lock files
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_str().unwrap_or("");
                    if ext_str == "lock" || ext_str == "exe" || ext_str == "dll" || ext_str == "so"
                    {
                        continue;
                    }
                }
                self.file_all.push(path);
            } else if path.is_dir() {
                self.scan_directory(&path, depth + 1);
            }
        }
    }

    // ── Ctrl+P agent file-context picker ─────────────────────────────────────

    /// Read a file for use as agent context.
    ///
    /// Returns `(display_name, content, line_count)` where `display_name` is the
    /// cwd-relative path, `content` is the (possibly truncated) file text, and
    /// `line_count` is the number of lines in the returned content.
    /// Files exceeding `AT_PICKER_MAX_LINES` are truncated and a notice is appended.
    fn read_file_for_context(
        path: &std::path::Path,
        project_root: &std::path::Path,
    ) -> std::io::Result<(String, String, usize)> {
        use crate::agent::AT_PICKER_MAX_LINES;

        let display_name =
            path.strip_prefix(project_root).unwrap_or(path).to_string_lossy().into_owned();

        let raw = std::fs::read_to_string(path)?;
        let all_lines: Vec<&str> = raw.lines().collect();
        let total = all_lines.len();

        let (content, line_count) = if total > AT_PICKER_MAX_LINES {
            let truncated = all_lines[..AT_PICKER_MAX_LINES].join("\n");
            let warned =
                format!("{truncated}\n\n[Truncated: showing {AT_PICKER_MAX_LINES}/{total} lines]");
            (warned, AT_PICKER_MAX_LINES)
        } else {
            (raw, total)
        };

        Ok((display_name, content, line_count))
    }

    /// Open the Ctrl+P file-context picker in the agent panel.
    ///
    /// Rescans the project files (always fresh) and initialises `at_picker` with
    /// an unfiltered list of all files.
    fn open_at_picker(&mut self) {
        self.scan_files();
        let results: Vec<(PathBuf, Vec<usize>)> =
            self.file_all.iter().map(|p| (p.clone(), vec![])).collect();
        let total = results.len();
        self.agent_panel.at_picker =
            Some(crate::agent::AtPickerState { query: String::new(), results, selected: 0 });
        self.set_status(format!("Attach file ({total} files) — type to filter"));
    }

    /// Recompute `at_picker.results` from `file_all` using the current query.
    fn refilter_at_picker(&mut self) {
        let query = match self.agent_panel.at_picker.as_ref() {
            Some(p) => p.query.clone(),
            None => return,
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let results: Vec<(PathBuf, Vec<usize>)> = if query.is_empty() {
            self.file_all.iter().map(|p| (p.clone(), vec![])).collect()
        } else {
            let mut scored: Vec<(i64, PathBuf, Vec<usize>)> = self
                .file_all
                .iter()
                .filter_map(|p| {
                    let display = p.strip_prefix(&cwd).unwrap_or(p).to_string_lossy().to_string();
                    Self::fuzzy_score(&query, &display).map(|(sc, idxs)| (sc, p.clone(), idxs))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored.into_iter().map(|(_, p, idxs)| (p, idxs)).collect()
        };

        if let Some(ref mut picker) = self.agent_panel.at_picker {
            let max = results.len().saturating_sub(1);
            picker.selected = picker.selected.min(max);
            picker.results = results;
        }
    }

    /// Handle a key event while the Ctrl+P file-context picker is open.
    fn handle_at_picker_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.agent_panel.at_picker = None;
                self.set_status(String::new());
            },

            KeyCode::Up | KeyCode::BackTab => {
                if let Some(ref mut picker) = self.agent_panel.at_picker {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                    }
                }
            },

            KeyCode::Down | KeyCode::Tab => {
                if let Some(ref mut picker) = self.agent_panel.at_picker {
                    let max = picker.results.len().saturating_sub(1);
                    if picker.selected < max {
                        picker.selected += 1;
                    }
                }
            },

            KeyCode::Enter => {
                // Toggle: if already attached remove it, otherwise add it.
                // Picker stays open so the user can attach/detach multiple files.
                let path_opt = self
                    .agent_panel
                    .at_picker
                    .as_ref()
                    .and_then(|p| p.results.get(p.selected))
                    .map(|(path, _)| path.clone());

                if let Some(path) = path_opt {
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    let display_name =
                        path.strip_prefix(&cwd).unwrap_or(&path).to_string_lossy().into_owned();

                    if let Some(pos) = self
                        .agent_panel
                        .file_blocks
                        .iter()
                        .position(|(name, _, _)| name == &display_name)
                    {
                        // Already attached — remove it.
                        self.agent_panel.file_blocks.remove(pos);
                        self.set_status(format!("Removed: {display_name}"));
                    } else {
                        // Not yet attached — read and add it.
                        match Self::read_file_for_context(&path, &cwd) {
                            Ok((display_name, content, line_count)) => {
                                let msg = format!(
                                    "Attached: {display_name} ({line_count} line{})",
                                    if line_count == 1 { "" } else { "s" }
                                );
                                self.agent_panel.file_blocks.push((
                                    display_name,
                                    content,
                                    line_count,
                                ));
                                self.set_status(msg);
                            },
                            Err(e) => {
                                self.set_status(format!("Cannot read file: {e}"));
                            },
                        }
                    }
                    // Picker stays open; Esc closes it.
                }
            },

            KeyCode::Backspace => {
                if let Some(ref mut picker) = self.agent_panel.at_picker {
                    picker.query.pop();
                }
                self.refilter_at_picker();
            },

            KeyCode::Char(ch) => {
                if let Some(ref mut picker) = self.agent_panel.at_picker {
                    picker.query.push(ch);
                }
                self.refilter_at_picker();
            },

            _ => {},
        }
        Ok(())
    }

    /// Handle keys in Insert mode
    fn handle_insert_mode(&mut self, key: KeyEvent) -> Result<()> {
        let should_notify_lsp = match key.code {
            // Tab: accept ghost text suggestion if one is displayed at the cursor.
            KeyCode::Tab => {
                if let Some((text, row, col)) = self.ghost_text.take() {
                    let cursor_matches = self
                        .current_buffer()
                        .map(|b| b.cursor.row == row && b.cursor.col == col)
                        .unwrap_or(false);
                    if cursor_matches {
                        for ch in text.chars() {
                            if ch == '\n' {
                                if let Some(buf) = self.current_buffer_mut() {
                                    buf.insert_newline();
                                }
                            } else if let Some(buf) = self.current_buffer_mut() {
                                buf.insert_char(ch);
                            }
                        }
                        self.pending_completion = None;
                        // Notify LSP of the accepted text.
                        self.notify_lsp_change();
                        // Immediately clear the debounce so we don't re-request right away.
                        self.last_edit_instant = None;
                        return Ok(());
                    }
                }
                // No ghost text — insert indent (spaces or tab based on config).
                let use_spaces = self.config.use_spaces;
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| {
                    if use_spaces {
                        for _ in 0..tab_width {
                            buf.insert_char(' ');
                        }
                    } else {
                        buf.insert_char('\t');
                    }
                });
                true
            },
            KeyCode::BackTab => {
                // Shift+Tab — remove one indent level from the start of the line.
                let use_spaces = self.config.use_spaces;
                let tab_width = self.config.tab_width;
                self.with_buffer(|buf| buf.dedent_line(use_spaces, tab_width));
                true
            },
            KeyCode::Esc => {
                // Clear ghost text when leaving Insert mode.
                self.ghost_text = None;
                self.pending_completion = None;
                self.last_edit_instant = None;
                self.mode = Mode::Normal;
                false
            },
            KeyCode::Char(c) => {
                self.with_buffer(|buf| buf.insert_char(c));
                true
            },
            KeyCode::Enter => {
                self.with_buffer(|buf| buf.insert_newline());
                true
            },
            KeyCode::Backspace => {
                self.with_buffer(|buf| buf.delete_char_before());
                true
            },
            KeyCode::Delete => {
                self.with_buffer(|buf| buf.delete_char_at());
                true
            },
            KeyCode::Left => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_left());
                false
            },
            KeyCode::Right => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_right());
                false
            },
            KeyCode::Up => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_up());
                false
            },
            KeyCode::Down => {
                self.ghost_text = None;
                self.with_buffer(|buf| buf.move_cursor_down());
                false
            },
            _ => false,
        };

        // Notify LSP about content changes
        if should_notify_lsp {
            self.notify_lsp_change();
        }

        Ok(())
    }

    /// Handle keys in Command mode
    fn handle_command_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            },
            KeyCode::Enter => {
                self.execute_command()?;
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            },
            KeyCode::Char(c) => {
                self.command_buffer.push(c);
            },
            KeyCode::Backspace => {
                self.command_buffer.pop();
            },
            _ => {},
        }

        Ok(())
    }

    /// Execute a command entered in command mode
    fn execute_command(&mut self) -> Result<()> {
        let cmd = self.command_buffer.trim();

        match cmd {
            "q" | "quit" => {
                self.check_quit()?;
            },
            "q!" | "quit!" => {
                self.should_quit = true;
            },
            "w" | "write" => {
                if let Some(buf) = self.current_buffer_mut() {
                    match buf.save() {
                        Ok(()) => {
                            if let Some(ref p) = buf.file_path.clone() {
                                self.self_saved.insert(p.clone(), std::time::Instant::now());
                            }
                            self.set_status("File saved".to_string());
                        },
                        Err(e) => self.set_status(format!("Error: {e}")),
                    }
                }
            },
            "wq" => {
                if let Some(buf) = self.current_buffer_mut() {
                    match buf.save() {
                        Ok(()) => {
                            if let Some(ref p) = buf.file_path.clone() {
                                self.self_saved.insert(p.clone(), std::time::Instant::now());
                            }
                        },
                        Err(e) => {
                            self.set_status(format!("Error: {e}"));
                            return Ok(());
                        },
                    }
                }
                self.should_quit = true;
            },
            // :bd / :bdelete — close buffer, refuse if unsaved
            "bd" | "bdelete" => {
                if !self.buffers.is_empty() {
                    let is_modified = self.buffers[self.current_buffer_idx].is_modified;
                    if is_modified {
                        self.set_status(
                            "Unsaved changes. Use :bd! to discard and close, or :w to save."
                                .to_string(),
                        );
                    } else {
                        let name = self.buffers[self.current_buffer_idx].name.clone();
                        self.buffers.remove(self.current_buffer_idx);
                        if !self.buffers.is_empty() {
                            self.current_buffer_idx =
                                self.current_buffer_idx.min(self.buffers.len() - 1);
                        }
                        self.set_status(format!("Closed buffer: {name}"));
                    }
                }
            },
            // :bd! / :bdelete! — force-close buffer, discarding unsaved changes
            "bd!" | "bdelete!" => {
                if !self.buffers.is_empty() {
                    let name = self.buffers[self.current_buffer_idx].name.clone();
                    self.buffers.remove(self.current_buffer_idx);
                    if !self.buffers.is_empty() {
                        self.current_buffer_idx =
                            self.current_buffer_idx.min(self.buffers.len() - 1);
                    }
                    self.set_status(format!("Closed buffer: {name} (discarded changes)"));
                }
            },
            "copilot status" => {
                let completion_state = if self.ghost_text.is_some() {
                    "suggestion ready (Tab to accept)"
                } else if self.pending_completion.is_some() {
                    "fetching suggestion..."
                } else {
                    "idle (type in Insert mode to trigger)"
                };
                let has_server = self.lsp_manager.get_client("copilot").is_some();
                self.set_status(format!(
                    "Copilot: server={} | {}",
                    if has_server { "running" } else { "not connected" },
                    completion_state
                ));
            },
            "copilot auth" => {
                // Re-run the auth check + sign-in initiate flow manually.
                if let Some(client) = self.lsp_manager.get_client("copilot") {
                    match client.copilot_check_status() {
                        Ok(rx) => {
                            self.copilot_auth_rx = Some(rx);
                            self.set_status("Copilot: checking auth status…".to_string());
                        },
                        Err(e) => {
                            self.set_status(format!("Copilot auth error: {}", e));
                        },
                    }
                } else {
                    self.set_status(
                        "Copilot: server not connected (check config.toml)".to_string(),
                    );
                }
            },
            // :e <path> / :edit <path> — open or create a file
            _ if cmd.starts_with("e ") || cmd.starts_with("edit ") => {
                let path_str = cmd.split_once(' ').map(|(_, rest)| rest).unwrap_or("").trim();
                if path_str.is_empty() {
                    self.set_status("Usage: e <path>  (e.g.  e src/main.rs)".to_string());
                } else {
                    let path = {
                        let p = PathBuf::from(path_str);
                        if p.is_absolute() {
                            p
                        } else {
                            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
                        }
                    };
                    self.open_file(&path)?;
                    // Refresh explorer tree so newly-created buffers show up on save.
                    if self.file_explorer.visible {
                        self.file_explorer.reload();
                    }
                }
            },
            // :s/pattern/replacement or :s/pattern/replacement/g
            _ if cmd.starts_with("s/") => {
                let rest = &cmd[2..];
                let parts: Vec<&str> = rest.splitn(3, '/').collect();
                if parts.len() < 2 {
                    self.set_status("Usage: s/pattern/replacement[/g]".to_string());
                } else {
                    let pattern = parts[0].to_string();
                    let replacement = parts[1].to_string();
                    let global = parts.get(2).map(|s| *s == "g").unwrap_or(false);
                    self.with_buffer(|buf| buf.set_search_pattern(pattern));
                    if global {
                        let count = self
                            .current_buffer_mut()
                            .map(|buf| buf.replace_all(&replacement))
                            .unwrap_or(0);
                        if count == 0 {
                            self.set_status("Pattern not found".to_string());
                        } else {
                            self.notify_lsp_change();
                            self.set_status(format!("{} replacement(s) made", count));
                        }
                    } else {
                        let made = self
                            .current_buffer_mut()
                            .map(|buf| buf.replace_current(&replacement))
                            .unwrap_or(false);
                        if made {
                            self.notify_lsp_change();
                            self.set_status("1 replacement made".to_string());
                        } else {
                            self.set_status("Pattern not found".to_string());
                        }
                    }
                }
            },
            // :12 — jump to line 12 (1-based), same as vim
            _ if cmd.chars().all(|c| c.is_ascii_digit()) => {
                if let Ok(n) = cmd.parse::<usize>() {
                    self.with_buffer(|buf| buf.goto_line(n));
                }
            },
            _ => {
                self.set_status(format!("Unknown command: {}", cmd));
            },
        }

        Ok(())
    }

    /// Check if we can quit (no unsaved changes)
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

    /// Request hover information at cursor position
    fn request_hover(&mut self) {
        // Get current buffer and position
        let (_uri, _position) = match self.get_current_lsp_position() {
            Some(pos) => pos,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            },
        };

        // TODO: Actually request hover and display result
        // For now, just show that it was triggered
        self.set_status("Hover requested (not yet fully implemented)".to_string());
    }

    /// Request go-to-definition at cursor position
    fn request_goto_definition(&mut self) {
        let (uri, position) = match self.get_current_lsp_position() {
            Some(pos) => pos,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            },
        };
        let language = self
            .current_buffer()
            .and_then(|b| b.file_path.as_deref())
            .map(LspManager::language_from_path)
            .unwrap_or_default();
        if let Some(client) = self.lsp_manager.get_client(&language) {
            match client.goto_definition(uri, position) {
                Ok(rx) => {
                    self.pending_goto_definition = Some(rx);
                    self.set_status("Finding definition…".to_string());
                },
                Err(e) => self.set_status(format!("LSP error: {e}")),
            }
        } else {
            self.set_status(format!("No LSP client for '{language}'"));
        }
    }

    /// Request find references at cursor position
    fn request_references(&mut self) {
        let (uri, position) = match self.get_current_lsp_position() {
            Some(pos) => pos,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            },
        };
        let language = self
            .current_buffer()
            .and_then(|b| b.file_path.as_deref())
            .map(LspManager::language_from_path)
            .unwrap_or_default();
        if let Some(client) = self.lsp_manager.get_client(&language) {
            match client.references(uri, position) {
                Ok(rx) => {
                    self.pending_references = Some(rx);
                    self.set_status("Finding references…".to_string());
                },
                Err(e) => self.set_status(format!("LSP error: {e}")),
            }
        } else {
            self.set_status(format!("No LSP client for '{language}'"));
        }
    }

    /// Request document symbols for the current file
    fn request_document_symbols(&mut self) {
        let uri = match self.get_current_lsp_position() {
            Some((uri, _)) => uri,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            },
        };
        let language = self
            .current_buffer()
            .and_then(|b| b.file_path.as_deref())
            .map(LspManager::language_from_path)
            .unwrap_or_default();
        if let Some(client) = self.lsp_manager.get_client(&language) {
            match client.document_symbols(uri) {
                Ok(rx) => {
                    self.pending_symbols = Some(rx);
                    self.set_status("Loading symbols…".to_string());
                },
                Err(e) => self.set_status(format!("LSP error: {e}")),
            }
        } else {
            self.set_status(format!("No LSP client for '{language}'"));
        }
    }

    /// Navigate the editor to an absolute file path + 0-based line/col.
    fn navigate_to_location(&mut self, path: std::path::PathBuf, line: u32, col: u32) {
        let already_open =
            self.buffers.iter().position(|b| b.file_path.as_deref() == Some(path.as_path()));
        if let Some(idx) = already_open {
            self.current_buffer_idx = idx;
        } else {
            let _ = self.open_file(&path);
        }
        self.with_buffer(|buf| {
            buf.cursor.row = line as usize;
            buf.cursor.col = col as usize;
            buf.ensure_cursor_visible();
        });
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.set_status(format!("{}:{}", name, line + 1));
    }

    /// Handle a goto-definition LSP response value.
    fn handle_goto_definition_response(&mut self, value: serde_json::Value) {
        if value.is_null() {
            self.set_status("No definition found".to_string());
            return;
        }
        // Scalar Location: { "uri": "...", "range": { ... } }
        if value.get("uri").is_some() {
            if let Some((path, line, col)) =
                lsp_parse_location(value.get("uri"), value.get("range"))
            {
                self.navigate_to_location(path, line, col);
            }
            return;
        }
        if let Some(arr) = value.as_array() {
            if arr.is_empty() {
                self.set_status("No definition found".to_string());
                return;
            }
            if arr.len() == 1 {
                let loc = &arr[0];
                let (uri_key, range_key) = if loc.get("targetUri").is_some() {
                    ("targetUri", "targetSelectionRange")
                } else {
                    ("uri", "range")
                };
                if let Some((path, line, col)) =
                    lsp_parse_location(loc.get(uri_key), loc.get(range_key))
                {
                    self.navigate_to_location(path, line, col);
                }
            } else {
                let entries: Vec<LocationEntry> = arr
                    .iter()
                    .filter_map(|loc| {
                        let (uri_key, range_key) = if loc.get("targetUri").is_some() {
                            ("targetUri", "targetSelectionRange")
                        } else {
                            ("uri", "range")
                        };
                        let (path, line, col) =
                            lsp_parse_location(loc.get(uri_key), loc.get(range_key))?;
                        let label = format!(
                            "{}:{}",
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            line + 1
                        );
                        Some(LocationEntry { label, file_path: path, line, col })
                    })
                    .collect();
                if entries.is_empty() {
                    self.set_status("No definition found".to_string());
                } else {
                    self.location_list = Some(LocationListState {
                        title: "Definitions".to_string(),
                        entries,
                        selected: 0,
                    });
                    self.mode = Mode::LocationList;
                }
            }
            return;
        }
        self.set_status("No definition found".to_string());
    }

    /// Handle a find-references LSP response value.
    fn handle_references_response(&mut self, value: serde_json::Value) {
        let arr = match value.as_array() {
            Some(a) if !a.is_empty() => a,
            _ => {
                self.set_status("No references found".to_string());
                return;
            },
        };
        let entries: Vec<LocationEntry> = arr
            .iter()
            .filter_map(|loc| {
                let (path, line, col) = lsp_parse_location(loc.get("uri"), loc.get("range"))?;
                let label = format!(
                    "{}:{}",
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    line + 1
                );
                Some(LocationEntry { label, file_path: path, line, col })
            })
            .collect();
        if entries.is_empty() {
            self.set_status("No references found".to_string());
            return;
        }
        let count = entries.len();
        self.location_list = Some(LocationListState {
            title: format!("References ({count})"),
            entries,
            selected: 0,
        });
        self.mode = Mode::LocationList;
    }

    /// Handle a document-symbols LSP response value.
    fn handle_symbols_response(&mut self, value: serde_json::Value) {
        let arr = match value.as_array() {
            Some(a) if !a.is_empty() => a,
            _ => {
                self.set_status("No symbols found".to_string());
                return;
            },
        };
        let current_path =
            self.current_buffer().and_then(|b| b.file_path.clone()).unwrap_or_default();
        let entries: Vec<LocationEntry> =
            arr.iter().flat_map(|sym| lsp_flatten_symbol(sym, &current_path)).collect();
        if entries.is_empty() {
            self.set_status("No symbols found".to_string());
            return;
        }
        let count = entries.len();
        self.location_list =
            Some(LocationListState { title: format!("Symbols ({count})"), entries, selected: 0 });
        self.mode = Mode::LocationList;
    }

    /// Handle key events while Mode::LocationList is active.
    fn handle_location_list_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                self.location_list = None;
            },
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(list) = &mut self.location_list {
                    if list.selected + 1 < list.entries.len() {
                        list.selected += 1;
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(list) = &mut self.location_list {
                    if list.selected > 0 {
                        list.selected -= 1;
                    }
                }
            },
            KeyCode::Enter => {
                if let Some(list) = &self.location_list {
                    if let Some(entry) = list.entries.get(list.selected) {
                        let path = entry.file_path.clone();
                        let line = entry.line;
                        let col = entry.col;
                        self.mode = Mode::Normal;
                        self.location_list = None;
                        self.navigate_to_location(path, line, col);
                    }
                }
            },
            _ => {},
        }
        Ok(())
    }

    /// Go to next diagnostic in current buffer
    fn goto_next_diagnostic(&mut self) {
        if self.current_diagnostics.is_empty() {
            self.set_status("No diagnostics".to_string());
            return;
        }

        let current_line = self.current_buffer().map(|buf| buf.cursor.row).unwrap_or(0);

        // Find next diagnostic after current line and extract position
        let next_diag = self
            .current_diagnostics
            .iter()
            .find(|d| d.range.start.line as usize > current_line)
            .map(|d| {
                (d.range.start.line as usize, d.range.start.character as usize, d.message.clone())
            });

        if let Some((row, col, msg)) = next_diag {
            self.with_buffer(|buf| {
                buf.cursor.row = row;
                buf.cursor.col = col;
                buf.ensure_cursor_visible();
            });
            self.set_status(format!("Diagnostic: {}", msg));
        } else {
            // Wrap around to first diagnostic
            let first_diag = self.current_diagnostics.first().map(|d| {
                (d.range.start.line as usize, d.range.start.character as usize, d.message.clone())
            });

            if let Some((row, col, msg)) = first_diag {
                self.with_buffer(|buf| {
                    buf.cursor.row = row;
                    buf.cursor.col = col;
                    buf.ensure_cursor_visible();
                });
                self.set_status(format!("Diagnostic: {}", msg));
            }
        }
    }

    /// Go to previous diagnostic in current buffer
    fn goto_prev_diagnostic(&mut self) {
        if self.current_diagnostics.is_empty() {
            self.set_status("No diagnostics".to_string());
            return;
        }

        let current_line = self.current_buffer().map(|buf| buf.cursor.row).unwrap_or(0);

        // Find previous diagnostic before current line and extract position
        let prev_diag = self
            .current_diagnostics
            .iter()
            .rev()
            .find(|d| (d.range.start.line as usize) < current_line)
            .map(|d| {
                (d.range.start.line as usize, d.range.start.character as usize, d.message.clone())
            });

        if let Some((row, col, msg)) = prev_diag {
            self.with_buffer(|buf| {
                buf.cursor.row = row;
                buf.cursor.col = col;
                buf.ensure_cursor_visible();
            });
            self.set_status(format!("Diagnostic: {}", msg));
        } else {
            // Wrap around to last diagnostic
            let last_diag = self.current_diagnostics.last().map(|d| {
                (d.range.start.line as usize, d.range.start.character as usize, d.message.clone())
            });

            if let Some((row, col, msg)) = last_diag {
                self.with_buffer(|buf| {
                    buf.cursor.row = row;
                    buf.cursor.col = col;
                    buf.ensure_cursor_visible();
                });
                self.set_status(format!("Diagnostic: {}", msg));
            }
        }
    }

    /// Helper to get current position for LSP requests
    fn get_current_lsp_position(&self) -> Option<(lsp_types::Uri, lsp_types::Position)> {
        let buf = self.current_buffer()?;
        let path = buf.file_path.as_ref()?;
        let uri = LspManager::path_to_uri(path).ok()?;
        let position =
            lsp_types::Position { line: buf.cursor.row as u32, character: buf.cursor.col as u32 };
        Some((uri, position))
    }

    /// Notify LSP about document changes and arm the completion debounce timer.
    fn notify_lsp_change(&mut self) {
        let buf = match self.current_buffer() {
            Some(b) => b,
            None => return,
        };

        let path = match &buf.file_path {
            Some(p) => p,
            None => return,
        };

        let uri = match LspManager::path_to_uri(path) {
            Ok(u) => u,
            Err(_) => return,
        };

        let language = LspManager::language_from_path(path);
        let version = buf.lsp_version;
        let text = buf.lines().join("\n");

        if let Some(client) = self.lsp_manager.get_client(&language) {
            let _ = client.did_change(uri, version, text);
        }

        // Discard stale ghost text and reset debounce timer.
        self.ghost_text = None;
        self.pending_completion = None;
        self.last_edit_instant = Some(Instant::now());
    }

    /// Send a `textDocument/inlineCompletion` request to any available LSP client.
    /// Tries the file's language client first, then falls back to "copilot".
    fn request_inline_completion(&mut self) {
        let buf = match self.current_buffer() {
            Some(b) => b,
            None => return,
        };
        let path = match buf.file_path.clone() {
            Some(p) => p,
            None => return,
        };
        let row = buf.cursor.row as u32;
        let col = buf.cursor.col as u32;

        let uri = match LspManager::path_to_uri(&path) {
            Ok(u) => u,
            Err(_) => return,
        };

        // Always prefer the "copilot" client for inline completions — language servers
        // like rust-analyzer don't support textDocument/inlineCompletion.
        // Fall back to the file-language client only if no Copilot client is registered.
        // Two separate lookups avoid a double-mutable-borrow on lsp_manager.
        let language = LspManager::language_from_path(&path);
        let has_copilot = self.lsp_manager.get_client("copilot").is_some();
        let client = if has_copilot {
            self.lsp_manager.get_client("copilot")
        } else {
            self.lsp_manager.get_client(&language)
        };

        if let Some(client) = client {
            match client.inline_completion(&uri, row, col) {
                Ok(rx) => self.pending_completion = Some(rx),
                Err(e) => tracing::debug!("inline_completion request failed: {}", e),
            }
        }
    }

    /// Clean up terminal state before exit
    fn cleanup(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Suspend the TUI, open lazygit, then restore the TUI.
    ///
    /// The suspend/resume pattern:
    ///   1. Leave alternate screen + disable raw mode  → terminal is back to normal
    ///   2. Spawn `lazygit` as a child process with inherited stdio  → lazygit takes over
    ///   3. Wait for lazygit to exit
    ///   4. Re-enter alternate screen + re-enable raw mode  → our TUI resumes
    ///   5. Force a full redraw so no lazygit artifacts remain
    ///   6. Reload every open buffer from disk (git ops may have changed files)
    fn open_lazygit(&mut self) -> Result<()> {
        // ── Suspend TUI ──────────────────────────────────────────────────────
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;

        // ── Run lazygit ───────────────────────────────────────────────────────
        // lazygit is itself a full-screen TUI; it inherits our stdin/stdout/stderr.
        let result = std::process::Command::new("lazygit").status();

        // ── Restore TUI (always, even if lazygit errored) ─────────────────────
        enable_raw_mode()?;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        // Force ratatui to repaint every cell — clears any lazygit residue.
        self.terminal.clear()?;

        // ── Handle outcome ────────────────────────────────────────────────────
        match result {
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.set_status(
                    "lazygit not found — install it (e.g. brew install lazygit)".to_string(),
                );
            },
            Err(e) => {
                self.set_status(format!("lazygit error: {e}"));
            },
            Ok(_) => {
                // Reload every open buffer — a git pull/checkout/rebase may have
                // changed files on disk that are currently open in the editor.
                for buf in &mut self.buffers {
                    if buf.file_path.is_some() {
                        let _ = buf.reload_from_disk();
                    }
                }
                self.set_status("Returned from lazygit".to_string());
            },
        }

        Ok(())
    }

    // ── Commit-message generation ──────────────────────────────────────────────

    /// Kick off a background AI task to generate a commit message.
    /// `from_staged = true`  → use `git diff --staged`
    /// `from_staged = false` → use `git show HEAD --stat -p`
    fn start_commit_msg(&mut self, from_staged: bool) {
        // Run the git command synchronously (it's fast).
        let diff_cmd = if from_staged {
            std::process::Command::new("git").args(["diff", "--staged"]).output()
        } else {
            std::process::Command::new("git").args(["show", "HEAD", "--stat", "-p"]).output()
        };

        let diff_text = match diff_cmd {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
            Err(e) => {
                self.set_status(format!("git error: {e}"));
                return;
            },
        };

        if diff_text.trim().is_empty() {
            let msg = if from_staged {
                "No staged changes — stage files first (git add)".to_string()
            } else {
                "No commits found".to_string()
            };
            self.set_status(msg);
            return;
        }

        let model_id = self
            .agent_panel
            .selected_model_id_with_fallback(&self.config.default_copilot_model)
            .to_string();

        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let system = "You are a concise git commit message writer. \
                Given a git diff, output ONLY a single commit message: \
                a short subject line (≤72 chars, imperative mood), then a blank line, \
                then an optional bullet-point body with the key changes. \
                No preamble, no explanation, just the commit message.";
            let user = format!("Write a commit message for this diff:\n\n```\n{diff_text}\n```");
            let result = async {
                let api_token = crate::agent::acquire_copilot_token().await?;
                crate::agent::one_shot_complete(&api_token, &model_id, system, &user, 256).await
            }
            .await;
            let _ = tx.send(result);
        });

        self.commit_msg.rx = Some(rx);
        self.commit_msg.from_staged = from_staged;
        self.commit_msg.buffer = String::new();
        self.mode = Mode::CommitMsg;
        self.set_status("Generating commit message…".to_string());
    }

    /// Handle key events while in Mode::CommitMsg.
    fn handle_commit_msg_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // Esc — discard
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.commit_msg.buffer.clear();
                self.commit_msg.rx = None;
                self.set_status("Commit message discarded".to_string());
            },
            // Enter — commit with the current message
            KeyCode::Enter => {
                let msg = self.commit_msg.buffer.trim().to_string();
                if msg.is_empty() {
                    self.set_status("Commit message is empty".to_string());
                    return Ok(());
                }
                let out = std::process::Command::new("git").args(["commit", "-m", &msg]).output();
                self.mode = Mode::Normal;
                self.commit_msg.buffer.clear();
                self.commit_msg.rx = None;
                match out {
                    Ok(o) if o.status.success() => {
                        self.set_status("Committed successfully".to_string());
                    },
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr).to_string();
                        self.set_status(format!("git commit failed: {err}"));
                    },
                    Err(e) => self.set_status(format!("git error: {e}")),
                }
            },
            // Backspace — delete last character
            KeyCode::Backspace => {
                self.commit_msg.buffer.pop();
            },
            // Regular characters
            KeyCode::Char(ch) => {
                self.commit_msg.buffer.push(ch);
            },
            _ => {},
        }
        Ok(())
    }

    /// Enter `Mode::ReleaseNotes` — count-input phase.
    fn start_release_notes(&mut self) {
        self.release_notes.buffer.clear();
        self.release_notes.count_input = String::from("10");
        self.release_notes.scroll = 0;
        self.mode = Mode::ReleaseNotes;
        self.set_status(
            "Enter number of commits (default 10) then press Enter to generate release notes"
                .to_string(),
        );
    }

    /// Run git log, spawn AI task, transition to generating phase.
    fn trigger_release_notes_generation(&mut self) {
        let count: usize =
            self.release_notes.count_input.trim().parse::<usize>().unwrap_or(10).clamp(1, 200);

        let log_output = std::process::Command::new("git")
            .args(["log", "--format=%H%n%s%n%b%n---", &format!("-{count}")])
            .output();

        let log_text = match log_output {
            Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
            Err(e) => {
                self.mode = Mode::Normal;
                self.set_status(format!("git error: {e}"));
                return;
            },
        };

        if log_text.trim().is_empty() {
            self.set_status("No commits found".to_string());
            return;
        }

        let model_id = self
            .agent_panel
            .selected_model_id_with_fallback(&self.config.default_copilot_model)
            .to_string();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let system = "You are a technical writer creating release notes for a software project. \
                Given a list of git commits, produce clean, user-friendly release notes in markdown. \
                Group related changes under headings like '### Features', '### Bug Fixes', '### Improvements'. \
                Be concise but descriptive. Omit merge commits and trivial chore changes. \
                Output markdown only — no preamble, no explanation.";
            let user = format!(
                "Generate release notes from these {count} commits:\n\n```\n{log_text}\n```"
            );
            let result = async {
                let api_token = crate::agent::acquire_copilot_token().await?;
                crate::agent::one_shot_complete(&api_token, &model_id, system, &user, 1024).await
            }
            .await;
            let _ = tx.send(result);
        });

        self.release_notes.rx = Some(rx);
        self.set_status(format!("Generating release notes from {count} commits…"));
    }

    /// Handle key events while in `Mode::ReleaseNotes`.
    fn handle_release_notes_mode(&mut self, key: KeyEvent) -> Result<()> {
        // Phase 2: generating — only Esc cancels.
        if self.release_notes.rx.is_some() {
            if key.code == KeyCode::Esc {
                self.release_notes.rx = None;
                self.mode = Mode::Normal;
                self.set_status("Release notes cancelled".to_string());
            }
            return Ok(());
        }

        // Phase 3: displaying the result.
        if !self.release_notes.buffer.is_empty() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                    self.release_notes.buffer.clear();
                    self.release_notes.scroll = 0;
                    self.set_status("Release notes closed".to_string());
                },
                KeyCode::Char('y') => {
                    let text = self.release_notes.buffer.clone();
                    self.sync_system_clipboard(&text);
                    self.set_status("Release notes copied to clipboard".to_string());
                },
                KeyCode::Char('j') | KeyCode::Down => {
                    self.release_notes.scroll = self.release_notes.scroll.saturating_add(1);
                },
                KeyCode::Char('k') | KeyCode::Up => {
                    self.release_notes.scroll = self.release_notes.scroll.saturating_sub(1);
                },
                _ => {},
            }
            return Ok(());
        }

        // Phase 1: count-entry input.
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.release_notes.count_input = String::from("10");
                self.set_status("Cancelled".to_string());
            },
            KeyCode::Enter => {
                self.trigger_release_notes_generation();
            },
            KeyCode::Backspace => {
                self.release_notes.count_input.pop();
            },
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                self.release_notes.count_input.push(ch);
            },
            _ => {},
        }
        Ok(())
    }

    /// Render the current buffer as HTML and open it in the system browser.
    ///
    /// Writes a self-contained HTML file to the OS temp directory and spawns
    /// the platform opener (`open` on macOS, `xdg-open` on Linux).  The opener
    /// runs detached — the TUI stays alive and no suspend/restore is needed.
    fn open_markdown_in_browser(&mut self) {
        let content = match self.current_buffer() {
            Some(buf) => buf.lines().join("\n"),
            None => {
                self.set_status("No buffer open".to_string());
                return;
            },
        };

        let file_stem = self
            .current_buffer()
            .and_then(|b| b.file_path.as_ref())
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("preview")
            .to_string();

        // ── If the file is already HTML, open it directly ─────────────────────
        let is_html = self
            .current_buffer()
            .and_then(|b| b.file_path.as_ref())
            .and_then(|p| p.extension())
            .map(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
            .unwrap_or(false);

        if is_html {
            let path = std::env::temp_dir().join(format!("forgiven_{file_stem}.html"));
            if let Err(e) = std::fs::write(&path, &content) {
                self.set_status(format!("Failed to write temp file: {e}"));
                return;
            }
            #[cfg(target_os = "macos")]
            let opener = "open";
            #[cfg(target_os = "linux")]
            let opener = "xdg-open";
            #[cfg(target_os = "windows")]
            let opener = "explorer";
            match std::process::Command::new(opener).arg(&path).spawn() {
                Ok(_) => self.set_status(format!("Opened {file_stem}.html in browser")),
                Err(e) => self.set_status(format!("Failed to open browser: {e}")),
            }
            return;
        }

        // ── Render markdown → HTML body ───────────────────────────────────────
        let parser = pulldown_cmark::Parser::new_ext(&content, pulldown_cmark::Options::all());
        let mut body = String::new();
        pulldown_cmark::html::push_html(&mut body, parser);

        // ── Wrap in a minimal, readable HTML page ─────────────────────────────
        // The script converts pulldown-cmark's  <pre><code class="language-mermaid">
        // output into <div class="mermaid"> elements, then loads Mermaid.js to
        // render them.
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  body {{
    max-width: 720px;
    margin: 0 auto;
    padding: 64px 32px 96px;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    font-size: 16px;
    line-height: 1.75;
    color: #1a1a1a;
    background: #fff;
    -webkit-font-smoothing: antialiased;
  }}
  h1, h2, h3, h4, h5, h6 {{
    font-weight: 600;
    line-height: 1.25;
    margin: 2em 0 0.5em;
    color: #111;
  }}
  h1 {{ font-size: 2em; border-bottom: 2px solid #e8e8e8; padding-bottom: 0.3em; margin-top: 0; }}
  h2 {{ font-size: 1.4em; border-bottom: 1px solid #e8e8e8; padding-bottom: 0.25em; }}
  h3 {{ font-size: 1.15em; }}
  p  {{ margin: 0 0 1.25em; }}
  a  {{ color: #0969da; text-decoration: none; }}
  a:hover {{ text-decoration: underline; }}
  strong {{ font-weight: 600; }}
  em {{ font-style: italic; }}
  hr {{ border: none; border-top: 1px solid #e8e8e8; margin: 2.5em 0; }}
  ul, ol {{ padding-left: 1.5em; margin: 0 0 1.25em; }}
  li {{ margin: 0.3em 0; }}
  li + li {{ margin-top: 0.25em; }}
  blockquote {{
    border-left: 3px solid #d0d0d0;
    margin: 1.5em 0;
    padding: 0.25em 0 0.25em 1.25em;
    color: #555;
  }}
  blockquote p {{ margin-bottom: 0; }}
  code {{
    font-family: "SFMono-Regular", "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.875em;
    background: #f3f3f3;
    padding: 0.15em 0.35em;
    border-radius: 3px;
    color: #d63384;
  }}
  pre {{
    background: #f6f6f6;
    border: 1px solid #e8e8e8;
    border-radius: 6px;
    padding: 1em 1.25em;
    overflow-x: auto;
    margin: 0 0 1.5em;
    line-height: 1.5;
  }}
  pre code {{
    background: none;
    padding: 0;
    border-radius: 0;
    font-size: 0.85em;
    color: inherit;
  }}
  img {{ max-width: 100%; height: auto; border-radius: 4px; }}
  table {{ border-collapse: collapse; width: 100%; margin: 0 0 1.5em; }}
  th, td {{ border: 1px solid #e0e0e0; padding: 0.5em 0.75em; text-align: left; }}
  th {{ background: #f6f6f6; font-weight: 600; }}
  tr:nth-child(even) {{ background: #fafafa; }}
</style>
</head>
<body>
{body}
<script src="https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js"></script>
<script>
  document.querySelectorAll('pre > code.language-mermaid').forEach(function(code) {{
    var div = document.createElement('div');
    div.className = 'mermaid';
    div.textContent = code.textContent;
    code.parentNode.replaceWith(div);
  }});
  mermaid.initialize({{ startOnLoad: false }});
  mermaid.run();
</script>
</body>
</html>"#,
            title = file_stem,
            body = body,
        );

        // ── Write temp file ───────────────────────────────────────────────────
        let path = std::env::temp_dir().join(format!("forgiven_{file_stem}.html"));
        if let Err(e) = std::fs::write(&path, &html) {
            self.set_status(format!("Failed to write temp file: {e}"));
            return;
        }

        // ── Spawn platform opener (detached) ──────────────────────────────────
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(target_os = "linux")]
        let opener = "xdg-open";
        #[cfg(target_os = "windows")]
        let opener = "explorer";

        match std::process::Command::new(opener).arg(&path).spawn() {
            Ok(_) => self.set_status(format!("Opened in browser: {}", path.display())),
            Err(e) => self.set_status(format!("Failed to open browser: {e}")),
        }
    }

    /// Extract the current (or next) mermaid diagram from the last agent reply,
    /// auto-fix parentheses in node labels, write a self-contained HTML file to
    /// the system temp directory, and open it in the default browser.
    fn open_mermaid_in_browser(&mut self) {
        let reply = match self.agent_panel.last_assistant_reply() {
            Some(r) => r,
            None => {
                self.set_status("No agent reply to render".to_string());
                return;
            },
        };

        let blocks = crate::agent::AgentPanel::extract_mermaid_blocks(&reply);
        if blocks.is_empty() {
            self.set_status("No mermaid blocks in last reply".to_string());
            return;
        }

        let idx = self.agent_panel.mermaid_block_idx % blocks.len();
        let source = fix_mermaid_parens(&blocks[idx]);
        self.agent_panel.mermaid_block_idx =
            (self.agent_panel.mermaid_block_idx + 1) % blocks.len();

        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Mermaid diagram {num}/{total}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  body {{
    margin: 0;
    padding: 2rem;
    background: #1e1e2e;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    min-height: 100vh;
  }}
  .mermaid {{ max-width: 100%; }}
</style>
</head>
<body>
<pre class="mermaid">
{source}
</pre>
<script type="module">
  import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs';
  mermaid.initialize({{ startOnLoad: true, theme: 'dark' }});
</script>
</body>
</html>"#,
            num = idx + 1,
            total = blocks.len(),
            source = source,
        );

        let path = std::env::temp_dir().join(format!("forgiven_mermaid_{}.html", idx + 1));
        if let Err(e) = std::fs::write(&path, &html) {
            self.set_status(format!("Failed to write mermaid file: {e}"));
            return;
        }

        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(target_os = "linux")]
        let opener = "xdg-open";
        #[cfg(target_os = "windows")]
        let opener = "explorer";

        match std::process::Command::new(opener).arg(&path).spawn() {
            Ok(_) => self.set_status(format!(
                "Mermaid diagram {}/{} opened in browser  (Ctrl+M for next)",
                idx + 1,
                blocks.len()
            )),
            Err(e) => self.set_status(format!("Failed to open browser: {e}")),
        }
    }
}

/// Fix unquoted node labels containing parentheses in Mermaid source.
///
/// Mermaid breaks when a square-bracket label contains `(` or `)` without
/// surrounding quotes.  AI models frequently generate this form, e.g.:
///   `K[UseHttpMetrics (Prometheus)]`
///
/// The fix is to wrap the label in double-quotes:
///   `K["UseHttpMetrics (Prometheus)"]`
///
/// This function scans each line character-by-character and applies the
/// transformation only to `[…]` groups that (a) contain `(` or `)` and
/// (b) are not already quoted.
fn fix_mermaid_parens(source: &str) -> String {
    let mut result = String::with_capacity(source.len() + 64);
    for line in source.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '[' {
                // Find the matching `]`, tracking depth.
                let start = i;
                let mut j = i + 1;
                let mut depth = 1usize;
                while j < chars.len() && depth > 0 {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {},
                    }
                    j += 1;
                }
                if depth == 0 {
                    // chars[start+1 .. j-1] is the inner label content.
                    let inner: String = chars[start + 1..j - 1].iter().collect();
                    if (inner.contains('(') || inner.contains(')')) && !inner.starts_with('"') {
                        result.push('[');
                        result.push('"');
                        result.push_str(&inner);
                        result.push('"');
                        result.push(']');
                    } else {
                        result.extend(chars[start..j].iter());
                    }
                    i = j;
                    continue;
                }
            }
            result.push(chars[i]);
            i += 1;
        }
        result.push('\n');
    }
    // Preserve original trailing-newline behaviour.
    if !source.ends_with('\n') {
        result.pop();
    }
    result
}

/// Strip a wrapping code fence that the AI may add despite being told not to.
/// Handles ` ```markdown `, ` ```md `, and plain ` ``` ` opening fences.
fn strip_markdown_fence(s: &str) -> String {
    let trimmed = s.trim();
    let after_open = trimmed
        .strip_prefix("```markdown")
        .or_else(|| trimmed.strip_prefix("```md"))
        .or_else(|| trimmed.strip_prefix("```"));
    if let Some(rest) = after_open {
        // Strip the newline immediately after the opening fence, then the closing fence.
        let body = rest.strip_prefix('\n').unwrap_or(rest);
        if let Some(stripped) = body.strip_suffix("```") {
            return stripped.trim_end().to_string();
        }
    }
    trimmed.to_string()
}

/// Compute a line-level LCS diff between two slices of strings.
/// Falls back to a simple all-removed / all-added output for very large inputs.
fn lcs_diff(old: &[String], new: &[String]) -> Vec<DiffLine> {
    const CAP: usize = 2000;
    if old.len() > CAP || new.len() > CAP {
        let mut r = Vec::with_capacity(old.len() + new.len());
        for l in old {
            r.push(DiffLine::Removed(l.clone()));
        }
        for l in new {
            r.push(DiffLine::Added(l.clone()));
        }
        return r;
    }
    let (m, n) = (old.len(), new.len());
    let mut dp = vec![0u32; (m + 1) * (n + 1)];
    let idx = |i: usize, j: usize| i * (n + 1) + j;
    for i in 1..=m {
        for j in 1..=n {
            dp[idx(i, j)] = if old[i - 1] == new[j - 1] {
                dp[idx(i - 1, j - 1)] + 1
            } else {
                dp[idx(i - 1, j)].max(dp[idx(i, j - 1)])
            };
        }
    }
    let mut result = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            result.push(DiffLine::Context(old[i - 1].clone()));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[idx(i, j - 1)] >= dp[idx(i - 1, j)]) {
            result.push(DiffLine::Added(new[j - 1].clone()));
            j -= 1;
        } else {
            result.push(DiffLine::Removed(old[i - 1].clone()));
            i -= 1;
        }
    }
    result.reverse();
    result
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

// =============================================================================
// LSP location-list helpers (free functions)
// =============================================================================

/// Parse a `(uri, range)` JSON pair into `(PathBuf, line, col)`.
/// Handles both `Location` (`uri`/`range`) and `LocationLink` (`targetUri`/…) shapes.
fn lsp_parse_location(
    uri_val: Option<&serde_json::Value>,
    range_val: Option<&serde_json::Value>,
) -> Option<(std::path::PathBuf, u32, u32)> {
    let uri_str = uri_val?.as_str()?;
    let path = lsp_uri_to_path(uri_str)?;
    let start = range_val?.get("start")?;
    let line = start.get("line")?.as_u64()? as u32;
    let col = start.get("character")?.as_u64()? as u32;
    Some((path, line, col))
}

/// Convert a `file://` URI to a `PathBuf`.
fn lsp_uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    // Strip "file://" (two slashes) then percent-decode basic sequences.
    let raw = uri.strip_prefix("file://")?;
    // Percent-decode space and hash (the most common cases in file paths).
    let decoded = raw.replace("%20", " ").replace("%23", "#");
    Some(std::path::PathBuf::from(decoded))
}

/// Recursively flatten a DocumentSymbol (or SymbolInformation) JSON value into
/// `LocationEntry` items.  Handles both the hierarchical (`DocumentSymbol`) and
/// flat (`SymbolInformation`) response shapes.
fn lsp_flatten_symbol(
    sym: &serde_json::Value,
    current_path: &std::path::Path,
) -> Vec<LocationEntry> {
    lsp_flatten_symbol_inner(sym, current_path, 0)
}

fn lsp_flatten_symbol_inner(
    sym: &serde_json::Value,
    current_path: &std::path::Path,
    depth: u8,
) -> Vec<LocationEntry> {
    if depth > 32 {
        return Vec::new();
    }
    let name = match sym.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return Vec::new(),
    };
    let kind = lsp_symbol_kind_name(sym.get("kind").and_then(|v| v.as_u64()).unwrap_or(0));

    let mut results = Vec::new();

    // DocumentSymbol shape: has "selectionRange" directly.
    if let Some(sel) = sym.get("selectionRange") {
        if let Some(start) = sel.get("start") {
            let line = start.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let col = start.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            results.push(LocationEntry {
                label: format!("{kind}  {name}  :{}", line + 1),
                file_path: current_path.to_path_buf(),
                line,
                col,
            });
        }
        // Recurse into children.
        if let Some(children) = sym.get("children").and_then(|v| v.as_array()) {
            for child in children {
                results.extend(lsp_flatten_symbol_inner(child, current_path, depth + 1));
            }
        }
        return results;
    }

    // SymbolInformation shape: has "location".
    if let Some(loc) = sym.get("location") {
        if let Some((path, line, col)) = lsp_parse_location(loc.get("uri"), loc.get("range")) {
            results.push(LocationEntry {
                label: format!("{kind}  {name}  :{}", line + 1),
                file_path: path,
                line,
                col,
            });
        }
    }
    results
}

/// Map an LSP `SymbolKind` integer to a short display string.
fn lsp_symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "mod",
        3 => "ns",
        4 => "pkg",
        5 => "cls",
        6 => "meth",
        7 => "prop",
        8 => "field",
        9 => "ctor",
        10 => "enum",
        11 => "iface",
        12 => "fn",
        13 => "var",
        14 => "const",
        15 => "str",
        16 => "num",
        17 => "bool",
        18 => "arr",
        19 => "obj",
        20 => "key",
        21 => "null",
        22 => "mem",
        23 => "event",
        24 => "op",
        25 => "type",
        _ => "sym",
    }
}
