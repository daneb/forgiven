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
use std::time::Instant;
use tokio::sync::oneshot;

use crate::agent::AgentPanel;
use crate::buffer::Buffer;
use crate::config::Config;
use crate::explorer::FileExplorer;
use crate::highlight::Highlighter;
use crate::keymap::{Action, KeyHandler, Mode};
use crate::lsp::{parse_first_inline_completion, LspManager};
use crate::search::{run_search, SearchState, SearchStatus};
use crate::ui::UI;
use lsp_types::Diagnostic;
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
    spans: Vec<Vec<ratatui::text::Span<'static>>>,
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

    // ── Apply-diff overlay (Mode::ApplyDiff) ──────────────────────────────────
    apply_diff_path: Option<std::path::PathBuf>,
    apply_diff_content: Option<String>,
    apply_diff_lines: Vec<DiffLine>,
    apply_diff_scroll: usize,

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

        Ok(Self {
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
            agent_panel: AgentPanel::new(),
            clipboard: None::<(String, ClipboardType)>,
            highlighter: Highlighter::new(),
            highlight_cache: None,
            file_explorer: FileExplorer::new(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            ),
            preview_scroll: 0,
            search_state: SearchState::new(),
            search_rx: None,
            last_search_instant: None,
            in_file_search_buffer: String::new(),
            rename_buffer: String::new(),
            rename_source: None,
            delete_confirm_path: None,
            apply_diff_path: None,
            apply_diff_content: None,
            apply_diff_lines: Vec::new(),
            apply_diff_scroll: 0,
            config,
        })
    }

    /// Open a file into a new buffer.
    /// Creates an empty buffer for non-existent files (new file workflow).
    /// Returns Ok(()) for unsupported binary files, displaying a status message instead of crashing.
    pub fn open_file(&mut self, path: &std::path::Path) -> Result<()> {
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

        Ok(())
    }

    /// Initialise LSP servers from the loaded config.
    /// Call this once after `new()`, before `run()`.
    /// Failures are non-fatal — the editor keeps working without LSP.
    pub async fn setup_lsp(&mut self) {
        let workspace_root =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        tracing::info!("LSP workspace root from current_dir: {:?}", workspace_root);

        for server in &self.config.lsp.servers.clone() {
            let args: Vec<&str> = server.args.iter().map(|s| s.as_str()).collect();
            tracing::info!(
                "Starting LSP server '{}' for language '{}'",
                server.command,
                server.language
            );
            match self
                .lsp_manager
                .add_server(server.language.clone(), &server.command, &args, workspace_root.clone())
                .await
            {
                Err(e) => {
                    let msg = format!("LSP '{}': {}", server.command, e);
                    tracing::warn!("{}", msg);
                    self.set_status(msg);
                },
                Ok(()) => {
                    // If this is the Copilot server, immediately check auth status
                    // so we can prompt the user to sign in if needed.
                    if server.language == "copilot" {
                        if let Some(client) = self.lsp_manager.get_client("copilot") {
                            match client.copilot_check_status() {
                                Ok(rx) => {
                                    self.copilot_auth_rx = Some(rx);
                                },
                                Err(e) => {
                                    tracing::warn!("copilot checkStatus failed: {}", e);
                                },
                            }
                        }
                    }
                },
            }
        }

        // Files were opened before LSP was ready — send did_open for each now.
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
    }

    /// Get the currently active buffer
    pub fn current_buffer(&self) -> Option<&Buffer> {
        self.buffers.get(self.current_buffer_idx)
    }

    /// Get mutable reference to current buffer
    pub fn current_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffers.get_mut(self.current_buffer_idx)
    }

    /// Main event loop
    pub async fn run(&mut self) -> Result<()> {
        const COMPLETION_DEBOUNCE_MS: u128 = 300;

        // Render on the very first frame regardless of activity.
        let mut needs_render = true;

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

            // Also render whenever background polling is still in-flight so the
            // user sees spinner/progress updates.
            // Force a render whenever background work is in-flight OR the
            // which-key timer is pending (so the popup appears after 500 ms
            // even when no key event arrives to trigger a normal render).
            if self.copilot_auth_rx.is_some()
                || self.pending_completion.is_some()
                || self.key_handler.which_key_pending()
                || self.search_rx.is_some()
            {
                needs_render = true;
            }

            // ── Render (only when something changed) ───────────────────────────
            if needs_render {
                self.render()?;
                needs_render = false;
            }

            // ── Input (blocks up to 50 ms) ─────────────────────────────────────
            if event::poll(std::time::Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) => {
                        self.handle_key(key)?;
                        needs_render = true;
                    },
                    // Bracketed paste: the terminal wraps pasted text in escape sequences
                    // so it arrives as a single Event::Paste(String) instead of a stream
                    // of KeyCode::Char / KeyCode::Enter events.
                    Event::Paste(text) => {
                        self.handle_paste(text)?;
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
        // viewport_height: subtract status line (1) and which-key popup (10) when shown.
        //
        // viewport_width: we must match the three-panel layout produced by UI::render()
        // so that horizontal scrolling kicks in at the right column.  The layout is:
        //   explorer+agent → [Length(25), Min(1), Percentage(35)]
        //   explorer only  → [Length(25), Min(1)]
        //   agent only     → [Percentage(60), Percentage(40)]
        //   neither        → [Min(1)]
        // Then subtract 2 for the diagnostic gutter that is always prepended.
        let size = self.terminal.size().unwrap_or_default();
        let viewport_height =
            (size.height as usize).saturating_sub(if show_which_key { 11 } else { 1 });

        const GUTTER: usize = 2;
        let total_w = size.width as usize;
        let editor_area_w = match (self.file_explorer.visible, self.agent_panel.visible) {
            (true, true) => total_w.saturating_sub(25).saturating_sub(total_w * 35 / 100),
            (true, false) => total_w.saturating_sub(25),
            (false, true) => total_w * 60 / 100,
            (false, false) => total_w,
        };
        let viewport_width = editor_area_w.saturating_sub(GUTTER);

        if let Some(buf) = self.current_buffer_mut() {
            buf.scroll_to_cursor(viewport_height, viewport_width);
        }

        // Get buffer data before drawing to avoid borrow issues
        let buffer_data = self.current_buffer().map(|buf| {
            (
                buf.name.clone(),
                buf.is_modified,
                buf.cursor.clone(),
                buf.scroll_row,
                buf.scroll_col,
                buf.lines().to_vec(),
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
            (buf.scroll_row, buf.lsp_version, ext)
        });

        let highlighted_lines: Option<Vec<Vec<Span<'static>>>> =
            if let Some((scroll_row, lsp_ver, ext)) = cache_key {
                let cache_hit = self.highlight_cache.as_ref().is_some_and(|c| {
                    c.buffer_idx == buf_idx
                        && c.scroll_row == scroll_row
                        && c.lsp_version == lsp_ver
                });

                if cache_hit {
                    // Zero allocation: clone the already-built Span vecs.
                    self.highlight_cache.as_ref().map(|c| c.spans.clone())
                } else {
                    // Cache miss: run syntect for the visible window and store result.
                    let spans = if let Some(buf) = self.current_buffer() {
                        let end = (scroll_row + term_height).min(buf.lines().len());
                        buf.lines()[scroll_row..end]
                            .iter()
                            .map(|line| self.highlighter.highlight_line(line, &ext))
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    self.highlight_cache = Some(HighlightCache {
                        buffer_idx: buf_idx,
                        scroll_row,
                        lsp_version: lsp_ver,
                        spans: spans.clone(),
                    });
                    Some(spans)
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
        // Computed when in MarkdownPreview mode; scrolled by self.preview_scroll so
        // render_buffer() only needs to take(viewport_height) from the front.
        let preview_lines_owned: Option<Vec<ratatui::text::Line<'static>>> =
            if mode == Mode::MarkdownPreview {
                let content =
                    self.current_buffer().map(|buf| buf.lines().join("\n")).unwrap_or_default();
                let all_lines = crate::markdown::render(&content, viewport_width);
                // Cap scroll so we can't scroll past the end.
                let max_scroll = all_lines.len().saturating_sub(1);
                let scroll = self.preview_scroll.min(max_scroll);
                Some(all_lines.into_iter().skip(scroll).collect())
            } else {
                None
            };

        let agent_ref = if self.agent_panel.visible { Some(&self.agent_panel) } else { None };
        let explorer_ref =
            if self.file_explorer.visible { Some(&self.file_explorer) } else { None };
        let hl_ref = highlighted_lines.as_deref();
        let preview_ref = preview_lines_owned.as_deref();
        let search_ref = if mode == Mode::Search { Some(&self.search_state) } else { None };
        let rename_buf_owned =
            if mode == Mode::RenameFile { Some(self.rename_buffer.clone()) } else { None };
        let rename_buf = rename_buf_owned.as_deref();

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
                self.apply_diff_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(unsaved buffer)".to_string()),
            )
        } else {
            None
        };
        let apply_diff_view = apply_diff_target_owned.as_ref().map(|t| crate::ui::ApplyDiffView {
            target: t.as_str(),
            lines: &self.apply_diff_lines,
            scroll: self.apply_diff_scroll,
        });

        self.terminal.draw(|frame| {
            UI::render(
                frame,
                buffer_data.as_ref(),
                mode,
                status,
                command_buffer,
                which_key_options.as_deref(),
                key_sequence.as_str(),
                buffer_list.as_ref(),
                file_list.as_ref(),
                &self.current_diagnostics,
                ghost,
                agent_ref,
                hl_ref,
                explorer_ref,
                preview_ref,
                search_ref,
                rename_buf,
                delete_name,
                apply_diff_view.as_ref(),
            );
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
                    | Mode::ApplyDiff
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
            Mode::ApplyDiff => self.handle_apply_diff_mode(key)?,
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
            | Action::DeleteSelection
            | Action::ChangeLine
            | Action::ChangeWord
            // Paste (alters content)
            | Action::PasteAfter
            | Action::PasteBefore
        );
        if needs_snapshot {
            if let Some(buf) = self.current_buffer_mut() {
                buf.save_undo_snapshot();
            }
        }

        match action {
            Action::Noop => unreachable!(),
            Action::Insert => self.mode = Mode::Insert,
            Action::InsertAppend => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
                }
                self.mode = Mode::Insert;
            },
            Action::InsertLineStart => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_start();
                }
                self.mode = Mode::Insert;
            },
            Action::InsertLineEnd => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end();
                }
                self.mode = Mode::Insert;
            },
            Action::InsertNewlineBelow => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end();
                    buf.insert_newline();
                }
                self.mode = Mode::Insert;
            },
            Action::InsertNewlineAbove => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_start();
                    buf.insert_newline();
                    buf.move_cursor_up();
                }
                self.mode = Mode::Insert;
            },
            Action::MoveLeft => {
                // h — clamped, no line wrap; repeats `count` times
                if let Some(buf) = self.current_buffer_mut() {
                    for _ in 0..count {
                        buf.move_cursor_left_clamp();
                    }
                }
            },
            Action::MoveRight => {
                // l — clamped, no line wrap; repeats `count` times
                if let Some(buf) = self.current_buffer_mut() {
                    for _ in 0..count {
                        buf.move_cursor_right_clamp();
                    }
                }
            },
            Action::MoveUp => {
                if let Some(buf) = self.current_buffer_mut() {
                    for _ in 0..count {
                        buf.move_cursor_up();
                    }
                }
            },
            Action::MoveDown => {
                if let Some(buf) = self.current_buffer_mut() {
                    for _ in 0..count {
                        buf.move_cursor_down();
                    }
                }
            },
            Action::MoveLineStart => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_start();
                }
            },
            Action::MoveFirstNonBlank => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_first_nonblank();
                }
            },
            Action::MoveLineEnd => {
                // Used by A / InsertLineEnd (cursor goes past last char)
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end();
                }
            },
            Action::MoveLineEndNormal => {
                // Used by $ in Normal mode (cursor lands ON last char)
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end_normal();
                }
            },
            Action::GotoFileTop => {
                if let Some(buf) = self.current_buffer_mut() {
                    // `5gg` → jump to line 5 (1-based); bare `gg` → first line
                    if count > 1 {
                        buf.goto_line(count);
                    } else {
                        buf.goto_first_line();
                    }
                }
            },
            Action::GotoFileBottom => {
                if let Some(buf) = self.current_buffer_mut() {
                    // `5G` → jump to line 5 (1-based); bare `G` → last line
                    if count > 1 {
                        buf.goto_line(count);
                    } else {
                        buf.goto_last_line();
                    }
                }
            },
            Action::MoveWordForward => {
                if let Some(buf) = self.current_buffer_mut() {
                    for _ in 0..count {
                        buf.move_cursor_word_forward();
                    }
                }
            },
            Action::MoveWordBackward => {
                if let Some(buf) = self.current_buffer_mut() {
                    for _ in 0..count {
                        buf.move_cursor_word_backward();
                    }
                }
            },
            Action::Command => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
            },
            Action::Visual => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.start_selection();
                }
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
                        let name = buf.name.clone();
                        self.buffers.remove(self.current_buffer_idx);
                        if !self.buffers.is_empty() {
                            self.current_buffer_idx =
                                self.current_buffer_idx.min(self.buffers.len() - 1);
                        }
                        self.set_status(format!("Closed buffer: {}", name));
                    }
                }
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
            Action::FileSave => {
                // Get file path and text before doing LSP operations
                let (file_path, text) = if let Some(buf) = self.current_buffer_mut() {
                    buf.save()?;
                    (buf.file_path.clone(), buf.lines().join("\n"))
                } else {
                    (None, String::new())
                };

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
                self.set_status("Document symbols not yet implemented".to_string());
                // TODO: Implement symbol picker
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
            // ── Project-wide text search ──────────────────────────────────────
            Action::SearchOpen => {
                self.search_state = SearchState::new();
                self.search_rx = None;
                self.last_search_instant = None;
                self.mode = Mode::Search;
            },
            // ── Edit operations ───────────────────────────────────────────────
            Action::DeleteChar => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_at_cursor();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.start_selection_line();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.clear_selection();
                }
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
                    if let Some(buf) = self.current_buffer_mut() {
                        match clip_type {
                            ClipboardType::Linewise => buf.paste_linewise_after(&text),
                            ClipboardType::Charwise => buf.paste_charwise_after(&text),
                        }
                    }
                    self.notify_lsp_change();
                }
            },
            Action::PasteBefore => {
                if let Some((text, clip_type)) = self.clipboard.clone() {
                    if let Some(buf) = self.current_buffer_mut() {
                        match clip_type {
                            ClipboardType::Linewise => buf.paste_linewise_before(&text),
                            ClipboardType::Charwise => buf.paste_charwise_before(&text),
                        }
                    }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.search_next();
                }
            },
            Action::InFileSearchPrev => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.search_prev();
                }
            },
        }
        Ok(())
    }

    /// Handle keys in Visual mode
    fn handle_visual_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // ── Exit / cancel ─────────────────────────────────────────────────
            KeyCode::Esc => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.clear_selection();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.save_undo_snapshot();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_left();
                    buf.update_selection();
                }
            },
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
                    buf.update_selection();
                }
            },
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_up();
                    buf.update_selection();
                }
            },
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_down();
                    buf.update_selection();
                }
            },
            KeyCode::Char('w') => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_word_forward();
                    buf.update_selection();
                }
            },
            KeyCode::Char('b') => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_word_backward();
                    buf.update_selection();
                }
            },
            KeyCode::Char('0') | KeyCode::Home => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_start();
                    buf.update_selection();
                }
            },
            KeyCode::Char('^') => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_first_nonblank();
                    buf.update_selection();
                }
            },
            KeyCode::Char('$') | KeyCode::End => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end_normal();
                    buf.update_selection();
                }
            },
            KeyCode::Char('G') => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.goto_last_line();
                    buf.update_selection();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.clear_selection();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.clear_selection();
                }
                self.mode = Mode::Normal;
            },

            // ── Delete / change selection (linewise) ─────────────────────────
            // `d` / `x` — remove selected lines, store in register, Normal
            KeyCode::Char('d') | KeyCode::Char('x') => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.save_undo_snapshot();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.save_undo_snapshot();
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_down();
                    buf.update_selection_line();
                }
            },
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_up();
                    buf.update_selection_line();
                }
            },
            KeyCode::Char('G') => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.goto_last_line();
                    buf.update_selection_line();
                }
            },
            KeyCode::Char('g') => {
                // gg — go to first line (we can't use pending_key here easily,
                // so a single `g` press jumps to the top — matches common muscle
                // memory for `Vgg` select-to-top).
                if let Some(buf) = self.current_buffer_mut() {
                    buf.goto_first_line();
                    buf.update_selection_line();
                }
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
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        if let Err(e) = fut.await {
                            tracing::warn!("Agent submit error: {}", e);
                        }
                    });
                });
            },
            // Backspace — delete last input character.
            KeyCode::Backspace => {
                self.agent_panel.input_backspace();
            },
            // Scroll history.
            KeyCode::Up => self.agent_panel.scroll_up(),
            KeyCode::Down => self.agent_panel.scroll_down(),
            // Ctrl+T — cycle through available models.
            // Note: Ctrl+M = Enter (0x0D) in all terminals and cannot be used here.
            // Ctrl+T (0x14) is safe in raw mode and not used by this editor.
            // On first press, fetches the live model list from the Copilot API.
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

                // 'y' with empty input = yank the last assistant reply to the system clipboard.
                if ch == 'y' && self.agent_panel.input.is_empty() {
                    if let Some(text) = self.agent_panel.last_assistant_reply() {
                        let len = text.lines().count();
                        self.sync_system_clipboard(&text);
                        self.set_status(format!("Copied {} lines to clipboard", len));
                    } else {
                        self.set_status("No reply to copy".to_string());
                    }
                    return Ok(());
                }

                // 'c' with empty input = cycle through code blocks in the last reply.
                if ch == 'c' && self.agent_panel.input.is_empty() {
                    if let Some(reply) = self.agent_panel.last_assistant_reply() {
                        let blocks = crate::agent::AgentPanel::extract_code_blocks(&reply);
                        if blocks.is_empty() {
                            self.set_status("No code blocks in last reply".to_string());
                        } else {
                            let idx = self.agent_panel.code_block_idx % blocks.len();
                            self.sync_system_clipboard(&blocks[idx]);
                            self.set_status(format!(
                                "Code block {}/{} copied",
                                idx + 1,
                                blocks.len()
                            ));
                            self.agent_panel.code_block_idx =
                                (self.agent_panel.code_block_idx + 1) % blocks.len();
                        }
                    } else {
                        self.set_status("No reply to copy".to_string());
                    }
                    return Ok(());
                }

                // 'a' with empty input = open apply-diff overlay.
                if ch == 'a' && self.agent_panel.input.is_empty() {
                    if let Some((path_hint, proposed_code)) = self.agent_panel.get_apply_candidate()
                    {
                        let cwd = std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
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
                                                == abs
                                                    .canonicalize()
                                                    .unwrap_or_else(|_| abs.clone())
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
                        let old: Vec<String> =
                            current_content.lines().map(str::to_string).collect();
                        let new: Vec<String> = proposed_code.lines().map(str::to_string).collect();
                        self.apply_diff_lines = lcs_diff(&old, &new);
                        self.apply_diff_path = resolved_path;
                        self.apply_diff_content = Some(proposed_code);
                        self.apply_diff_scroll = 0;
                        self.mode = Mode::ApplyDiff;
                    } else {
                        self.set_status("No code block in latest reply to apply".to_string());
                    }
                } else {
                    self.agent_panel.input_char(ch);
                }
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
            // Preserve line breaks so multi-line pastes work in the agent input.
            let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
            for ch in normalised.chars() {
                self.agent_panel.input_char(ch);
            }
        } else if self.mode == Mode::Insert {
            // In insert mode, paste the text as-is into the current buffer.
            let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
            if let Some(buf) = self.current_buffer_mut() {
                buf.insert_text_block(&normalised);
            }
        }
        Ok(())
    }

    // ── Explorer mode key handling ─────────────────────────────────────────────

    fn handle_explorer_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => {
                // Blur explorer, return to editor (keep panel visible)
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

    // ── Apply-diff mode ───────────────────────────────────────────────────────

    fn clear_apply_diff(&mut self) {
        self.apply_diff_path = None;
        self.apply_diff_content = None;
        self.apply_diff_lines.clear();
        self.apply_diff_scroll = 0;
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
                self.apply_diff_scroll = self.apply_diff_scroll.saturating_add(1);
            },
            KeyCode::Char('k') | KeyCode::Up => {
                self.apply_diff_scroll = self.apply_diff_scroll.saturating_sub(1);
            },
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.apply_diff_scroll = self.apply_diff_scroll.saturating_add(20);
            },
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.apply_diff_scroll = self.apply_diff_scroll.saturating_sub(20);
            },
            _ => {},
        }
        Ok(())
    }

    fn do_apply_diff(&mut self) -> Result<()> {
        let content = match self.apply_diff_content.take() {
            Some(c) => c,
            None => {
                self.clear_apply_diff();
                self.mode = Mode::Normal;
                return Ok(());
            },
        };
        let path = self.apply_diff_path.take();
        self.apply_diff_lines.clear();
        self.apply_diff_scroll = 0;
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.replace_all_lines(new_lines);
                }
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
                let count = if let Some(buf) = self.current_buffer_mut() {
                    buf.set_search_pattern(pattern)
                } else {
                    0
                };
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
                    if let Some(buf) = self.current_buffer_mut() {
                        buf.goto_line(line + 1); // goto_line expects 1-based
                    }
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
                // No ghost text — insert a literal tab character.
                if let Some(buf) = self.current_buffer_mut() {
                    buf.insert_char('\t');
                }
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.insert_char(c);
                }
                true
            },
            KeyCode::Enter => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.insert_newline();
                }
                true
            },
            KeyCode::Backspace => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_before();
                }
                true
            },
            KeyCode::Delete => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_at();
                }
                true
            },
            KeyCode::Left => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_left();
                }
                false
            },
            KeyCode::Right => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
                }
                false
            },
            KeyCode::Up => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_up();
                }
                false
            },
            KeyCode::Down => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_down();
                }
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
                    buf.save()?;
                    self.set_status("File saved".to_string());
                }
            },
            "wq" => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.save()?;
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
                    if let Some(buf) = self.current_buffer_mut() {
                        buf.set_search_pattern(pattern);
                    }
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
                self.set_status("Buffer has unsaved changes. Use :q! to force quit.".to_string());
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
        let (_uri, _position) = match self.get_current_lsp_position() {
            Some(pos) => pos,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            },
        };

        self.set_status("Go-to-definition requested (not yet fully implemented)".to_string());
    }

    /// Request find references at cursor position
    fn request_references(&mut self) {
        let (_uri, _position) = match self.get_current_lsp_position() {
            Some(pos) => pos,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            },
        };

        self.set_status("Find references requested (not yet fully implemented)".to_string());
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
            if let Some(buf) = self.current_buffer_mut() {
                buf.cursor.row = row;
                buf.cursor.col = col;
                buf.ensure_cursor_visible();
            }
            self.set_status(format!("Diagnostic: {}", msg));
        } else {
            // Wrap around to first diagnostic
            let first_diag = self.current_diagnostics.first().map(|d| {
                (d.range.start.line as usize, d.range.start.character as usize, d.message.clone())
            });

            if let Some((row, col, msg)) = first_diag {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.cursor.row = row;
                    buf.cursor.col = col;
                    buf.ensure_cursor_visible();
                }
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
            if let Some(buf) = self.current_buffer_mut() {
                buf.cursor.row = row;
                buf.cursor.col = col;
                buf.ensure_cursor_visible();
            }
            self.set_status(format!("Diagnostic: {}", msg));
        } else {
            // Wrap around to last diagnostic
            let last_diag = self.current_diagnostics.last().map(|d| {
                (d.range.start.line as usize, d.range.start.character as usize, d.message.clone())
            });

            if let Some((row, col, msg)) = last_diag {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.cursor.row = row;
                    buf.cursor.col = col;
                    buf.ensure_cursor_visible();
                }
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
  body {{ max-width: 800px; margin: 40px auto; padding: 0 20px;
          font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
          line-height: 1.6; color: #222; }}
  pre  {{ background: #f5f5f5; padding: 1em; border-radius: 4px; overflow-x: auto; }}
  code {{ font-family: "SFMono-Regular", Consolas, monospace; font-size: 0.9em; }}
  pre code {{ background: none; padding: 0; }}
  blockquote {{ border-left: 4px solid #ddd; margin: 0; padding-left: 1em; color: #555; }}
  img  {{ max-width: 100%; }}
  h1,h2 {{ border-bottom: 1px solid #eee; padding-bottom: 0.3em; }}
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
