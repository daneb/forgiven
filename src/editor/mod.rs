mod actions;
mod ai;
mod event_loop;
mod file_ops;
mod folding;
mod hooks;
mod inline_assist;
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
    apply_hunk_verdicts, ClipboardType, CommitMsgState, FoldCache, HighlightCache, LspState,
    MarkdownCache, ReleaseNotesState, SplitState, StickyScrollCache,
};
pub use state::{
    DiffLine, HoverPopupState, InlineAssistPhase, InlineAssistState, LocationEntry,
    LocationListState, ReviewChangesState, Verdict,
};

use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::oneshot;

use crate::agent::AgentPanel;
use crate::buffer::Buffer;
use crate::config::Config;
use crate::explorer::FileExplorer;
use crate::highlight::Highlighter;
use crate::keymap::{KeyHandler, Mode};
use crate::lsp::LspManager;
use crate::mcp::McpManager;
use crate::search::SearchState;
use crate::spec_framework;
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

    // ── Insights dashboard (ADR 0129 Phase 3) ────────────────────────────────
    /// Active only while `mode == Mode::InsightsDashboard`.
    pub insights_dashboard: Option<crate::insights::panel::InsightsDashboardState>,

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

    // ── Insights narrative generation (Phase 4, ADR 0129) ─────────────────────
    /// In-flight `:insights summarize` LLM task. Polled each tick.
    insights_narrative_rx: Option<oneshot::Receiver<anyhow::Result<String>>>,

    // ── MCP servers ───────────────────────────────────────────────────────────
    /// Manages connected MCP servers and their tool registries.
    /// Set once the background connection task completes (see `mcp_rx`).
    mcp_manager: Option<std::sync::Arc<McpManager>>,
    /// Receives the completed `McpManager` from the background startup task.
    /// Polled each tick; cleared and wired into `agent_panel` on first `Ok`.
    mcp_rx: Option<oneshot::Receiver<McpManager>>,

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

    // ── Nexus sidecar IPC (Phase 3 — Hybrid Reliability) ─────────────────────
    /// UDS server broadcasting buffer/cursor/mode events to the Tauri sidecar.
    sidecar: Option<crate::sidecar::SidecarServer>,
    /// Debounce timestamp for buffer-update events (set on every edit, flushed
    /// after SIDECAR_DEBOUNCE_MS elapses without a further edit).
    last_sidecar_send: Option<std::time::Instant>,
    /// Last cursor row sent — avoids spamming cursor_move on every keystroke.
    sidecar_last_cursor_line: Option<u32>,
    /// Stringified mode from the previous tick — detects mode transitions.
    sidecar_last_mode: Option<String>,
    /// True while the companion has connected but not yet received a snapshot.
    /// Retries every tick until `current_buffer()` is non-empty and a
    /// buffer_update is successfully sent.
    sidecar_snapshot_pending: bool,
    /// Buffer index sent in the last snapshot — detects buffer switches so
    /// flush_sidecar_events() can fire an immediate update without patching
    /// every callsite that changes current_buffer_idx.
    sidecar_last_buffer_idx: Option<usize>,

    // ── Terminal graphics capability (Phase 1 — Glimpse) ─────────────────────
    /// Detected inline image protocol for this terminal session.
    /// `None` until `setup_services()` completes the detection probe.
    pub image_protocol: Option<crate::graphics::ImageProtocol>,

    // ── Companion process (Step 4.5 — Hybrid Reliability) ────────────────────
    /// Child process handle for the Tauri companion window.
    /// `None` when the companion is not running.
    companion_process: Option<std::process::Child>,
    /// True from the moment the companion connects to the Nexus socket.
    /// Reset to false when the companion process is killed.
    pub sidecar_client_connected: bool,
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
            lsp: LspState::default(),
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
                panel.intent_translator_enabled = config.agent.intent_translator.enabled;
                panel.intent_translator_provider = config.agent.intent_translator.provider.clone();
                panel.intent_translator_ollama_model =
                    config.agent.intent_translator.ollama_model.clone();
                panel.intent_translator_model = config.agent.intent_translator.model.clone();
                panel.intent_translator_min_chars =
                    config.agent.intent_translator.min_chars_to_translate;
                panel.intent_translator_timeout_ms = config.agent.intent_translator.timeout_ms;
                panel.intent_translator_skip_patterns =
                    config.agent.intent_translator.skip_patterns.clone();
                panel.codified_context_enabled = config.agent.codified_context.enabled;
                panel.codified_context_constitution_max_tokens =
                    config.agent.codified_context.constitution_max_tokens;
                panel.codified_context_max_specialists =
                    config.agent.codified_context.max_specialists_per_turn;
                panel.codified_context_knowledge_max_bytes =
                    config.agent.codified_context.knowledge_fetch_max_bytes;
                panel
            },
            clipboard: None::<(String, ClipboardType)>,
            highlighter: Highlighter::new(),
            highlight_cache: None,
            visual_text_obj_prefix: None,
            surround_change_from: None,
            inline_assist: None,
            review_changes: None,
            insights_dashboard: None,
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
            fold_cache: None,
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
            insights_narrative_rx: None,
            mcp_manager: None,
            mcp_rx: None,
            file_watcher: None,
            watcher_rx: None,
            self_saved: std::collections::HashMap::new(),
            log_buffer: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            startup_elapsed: None,
            config,
            sidecar: None,
            last_sidecar_send: None,
            sidecar_last_cursor_line: None,
            sidecar_last_mode: None,
            sidecar_snapshot_pending: false,
            sidecar_last_buffer_idx: None,
            image_protocol: None,
            companion_process: None,
            sidecar_client_connected: false,
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

        // Arm sidecar debounce so the companion sees the new file after 300 ms.
        // Works even when called before the sidecar is bound (setup_services runs later).
        self.last_sidecar_send = Some(std::time::Instant::now());

        Ok(())
    }

    /// Start all LSP servers and MCP servers concurrently, then apply the results.
    ///
    /// LSP startup blocks the loading screen (the editor needs completions and
    /// diagnostics to be useful).  MCP startup is fire-and-forget: a background
    /// task is spawned immediately and the result is wired in via `mcp_rx` once
    /// the connections complete — the editor opens without waiting for MCP.
    pub async fn setup_services(&mut self) {
        // ── Terminal graphics detection (must run first — writes escape seqs) ──
        let protocol = crate::graphics::detect_protocol().await;
        tracing::info!("Terminal image protocol: {:?}", protocol);
        self.image_protocol = Some(protocol);

        let workspace_root =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let lsp_servers = self.config.lsp.servers.clone();
        let mcp_servers = self.config.mcp.servers.clone();
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
                    if language == "copilot" {
                        if let Some(c) = self.lsp.manager.get_client("copilot") {
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
            if let Some(client) = self.lsp.manager.get_client(&language) {
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

        // ── Nexus sidecar UDS listener ────────────────────────────────────────
        let socket_path = crate::sidecar::SidecarServer::socket_path();
        match crate::sidecar::SidecarServer::bind(&socket_path).await {
            Ok(server) => {
                tracing::info!("Nexus UDS listening at {:?}", socket_path);
                self.sidecar = Some(server);
            },
            Err(e) => tracing::warn!("Nexus sidecar unavailable: {e}"),
        }

        if self.config.sidecar.auto_launch {
            self.spawn_companion();
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

    /// Apply a mutating closure to the current buffer, returning `Some(T)` on
    /// success or `None` when no buffer is open. Prefer this over the raw
    /// `if let Some(buf) = self.current_buffer_mut()` pattern so that future
    /// additions stay uniform and the nesting depth stays flat.
    #[inline]
    fn with_buffer<T, F: FnOnce(&mut Buffer) -> T>(&mut self, f: F) -> Option<T> {
        self.current_buffer_mut().map(f)
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
        // Notify the sidecar that the editor is exiting before tearing down the socket.
        if let Some(ref s) = self.sidecar {
            s.send(crate::sidecar::NexusEvent::shutdown());
        }
        // Belt-and-suspenders: kill the companion process in case it didn't
        // receive or handle the shutdown event (e.g. failed to connect).
        self.kill_companion();
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), DisableBracketedPaste, LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Spawn the Tauri companion window as a child process.
    ///
    /// The companion auto-discovers the Nexus socket via `NEXUS_SOCKET` env var
    /// so it connects immediately without polling.
    ///
    /// Binary resolution order:
    /// 1. `config.sidecar.binary_path` — explicit user override
    /// 2. Directory of the running forgiven executable — works after `make install`
    /// 3. `forgiven-companion` on `$PATH` — fallback for custom setups
    pub(crate) fn spawn_companion(&mut self) {
        let socket_path = crate::sidecar::SidecarServer::socket_path();
        let binary = self.resolve_companion_binary();
        match std::process::Command::new(&binary)
            .env("NEXUS_SOCKET", socket_path.to_string_lossy().as_ref())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.companion_process = Some(child);
                tracing::info!("Companion launched: {binary}");
            },
            Err(e) => {
                self.set_status(format!("Companion: could not launch '{binary}' — {e}"));
                tracing::warn!("Companion launch failed: {e}");
            },
        }
    }

    /// Resolve the companion binary path using the three-level lookup.
    fn resolve_companion_binary(&self) -> String {
        // 1. Explicit config override.
        if let Some(ref p) = self.config.sidecar.binary_path {
            return p.clone();
        }
        // 2. Same directory as the running forgiven binary.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join("forgiven-companion");
                if candidate.exists() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
        // 3. Fall back to PATH.
        "forgiven-companion".to_string()
    }

    /// Kill the companion child process if it is running.
    pub(crate) fn kill_companion(&mut self) {
        if let Some(mut child) = self.companion_process.take() {
            let _ = child.kill();
            self.sidecar_client_connected = false;
            tracing::info!("Companion closed");
        }
    }

    /// Flush debounced sidecar events (buffer updates, cursor moves, mode changes).
    ///
    /// Called once per event-loop tick. Debouncing mirrors the 300 ms completion
    /// debounce so rapid typing coalesces into a single buffer_update.
    pub(crate) fn flush_sidecar_events(&mut self) {
        const DEBOUNCE_MS: u128 = 300;

        // Nothing to do when no sidecar is running.
        if self.sidecar.is_none() {
            return;
        }

        // ── Debounced buffer update ───────────────────────────────────────────
        if let Some(t) = self.last_sidecar_send {
            if t.elapsed().as_millis() >= DEBOUNCE_MS {
                self.last_sidecar_send = None;
                // Collect what we need before borrowing self.sidecar.
                let event = self.current_buffer().map(|buf| {
                    let content = buf.lines().join("\n");
                    let file_path =
                        buf.file_path.as_deref().and_then(|p| p.to_str()).map(String::from);
                    let cursor_line = buf.cursor.row as u32;
                    let content_type = buf
                        .file_path
                        .as_deref()
                        .map(LspManager::language_from_path)
                        .unwrap_or_default();
                    crate::sidecar::NexusEvent::buffer_update(
                        &content,
                        &content_type,
                        file_path.as_deref(),
                        cursor_line,
                    )
                });
                if let Some(evt) = event {
                    if let Some(ref server) = self.sidecar {
                        server.send(evt);
                        self.sidecar_snapshot_pending = false;
                    }
                }
            }
        }

        // ── Cursor move (threshold: ±3 lines to filter Insert-mode jitter) ────
        let cursor_event = self.current_buffer().and_then(|buf| {
            let line = buf.cursor.row as u32;
            let should_send =
                self.sidecar_last_cursor_line.is_none_or(|prev| line.abs_diff(prev) >= 3);
            if should_send {
                let file_path = buf.file_path.as_deref().and_then(|p| p.to_str()).map(String::from);
                Some((line, file_path))
            } else {
                None
            }
        });
        if let Some((line, file_path)) = cursor_event {
            self.sidecar_last_cursor_line = Some(line);
            if let Some(ref server) = self.sidecar {
                server.send(crate::sidecar::NexusEvent::cursor_move(file_path.as_deref(), line));
            }
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
