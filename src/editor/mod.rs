mod actions;
mod event_loop;
mod file_ops;
mod folding;
mod input;
mod lsp;
mod mode_handlers;
mod pickers;
mod render;
mod search;
mod state;
mod surround;
mod text_objects;
pub(crate) use state::{
    ClipboardType, FoldCache, HighlightCache, LspState, MarkdownCache, SplitState,
    StickyScrollCache,
};
pub use state::{HoverPopupState, LocationEntry, LocationListState};

use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;
use tokio::sync::oneshot;

use crate::buffer::Buffer;
use crate::config::Config;
use crate::explorer::FileExplorer;
use crate::highlight::Highlighter;
use crate::keymap::{KeyHandler, Mode};
use crate::lsp::LspManager;
use crate::search::SearchState;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

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

    /// When true the status message persists across keypresses until explicitly
    /// cleared (used for auth URLs which the user needs to read).
    status_sticky: bool,

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

    /// All LSP state — manager, diagnostics, in-flight RPCs, overlays (ADR 0144).
    lsp: LspState,

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
    /// Scroll offset (in rendered lines) for preview mode.
    preview_scroll: usize,
    /// Cached rendered markdown lines — avoids re-parsing on every render frame.
    markdown_cache: Option<MarkdownCache>,

    /// Cached sticky-scroll header — avoids walking the tree-sitter CST every frame.
    sticky_scroll_cache: Option<StickyScrollCache>,
    /// Cached fold hidden-row set and stub map (ADR 0138).
    fold_cache: Option<FoldCache>,

    // ── Project-wide text search ──────────────────────────────────────────────
    /// State for the search overlay (Mode::Search).
    search_state: SearchState,
    /// In-flight ripgrep task receiver; `Some` while a search is running.
    search_rx: Option<oneshot::Receiver<anyhow::Result<Vec<crate::search::SearchResult>>>>,
    /// Timestamp of the last query/glob change — drives the 300 ms debounce.
    last_search_instant: Option<std::time::Instant>,

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
    /// Time from process start to the editor being fully ready (LSP set up).
    /// Set by main() after setup completes; displayed on the welcome screen.
    pub startup_elapsed: Option<std::time::Duration>,

    // ── Configuration ─────────────────────────────────────────────────────────
    /// Editor configuration (LSP servers, tab width, etc.)
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
            status_sticky: false,
            buffer_picker_idx: 0,
            file_picker_idx: 0,
            file_all: Vec::new(),
            file_query: String::new(),
            file_list: Vec::new(),
            recent_files: Self::load_recents(),
            lsp: LspState::default(),
            clipboard: None::<(String, ClipboardType)>,
            highlighter: Highlighter::new(),
            highlight_cache: None,
            visual_text_obj_prefix: None,
            surround_change_from: None,
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
            fold_cache: None,
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

    /// Render a loading frame while async setup (LSP) is in progress.
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

        // Dedup: if this file is already open in a buffer, switch to it instead.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(idx) = self.buffers.iter().position(|b| {
            b.file_path
                .as_ref()
                .is_some_and(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == canonical)
        }) {
            self.current_buffer_idx = idx;
            return Ok(());
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
            if let Some(client) = self.lsp.manager.get_client(&language) {
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

    /// Start all LSP servers concurrently, then apply the results.
    pub async fn setup_services(&mut self) {
        let workspace_root =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let lsp_servers = self.config.lsp.servers.clone();
        let notif_tx = self.lsp.manager.notification_tx();

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
                    self.lsp.manager.insert_client(language.clone(), client);
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
            if let Some(client) = self.lsp.manager.get_client(&language) {
                let _ = client.did_open(uri, language, text);
            }
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

    /// Apply a mutating closure to the current buffer.
    #[inline]
    fn with_buffer<T, F: FnOnce(&mut Buffer) -> T>(&mut self, f: F) -> Option<T> {
        self.current_buffer_mut().map(f)
    }

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
    fn set_sticky(&mut self, msg: String) {
        self.status_sticky = true;
        self.status_message = Some(msg);
    }

    /// Write `text` to the OS system clipboard.
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
