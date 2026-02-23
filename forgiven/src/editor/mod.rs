use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::oneshot;

use crate::agent::AgentPanel;
use crate::buffer::Buffer;
use crate::config::Config;
use crate::highlight::Highlighter;
use crate::keymap::{Action, KeyHandler, Mode};
use crate::lsp::{LspManager, parse_first_inline_completion};
use crate::ui::UI;
use ratatui::text::Span;
use lsp_types::Diagnostic;

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
    
    /// List of files for file picker
    file_list: Vec<PathBuf>,
    
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
    /// Last yanked / deleted text for p/P paste operations.
    clipboard: Option<String>,

    // ── Syntax highlighter ────────────────────────────────────────────────────
    /// Loaded once at startup; highlight_line() is called per visible line each frame.
    highlighter: Highlighter,
}

impl Editor {
    pub fn new() -> Result<Self> {
        // Set up terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
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
            file_list: Vec::new(),
            lsp_manager: LspManager::new(),
            current_diagnostics: Vec::new(),
            ghost_text: None,
            pending_completion: None,
            last_edit_instant: None,
            copilot_auth_rx: None,
            status_sticky: false,
            agent_panel: AgentPanel::new(),
            clipboard: None,
            highlighter: Highlighter::new(),
        })
    }

    /// Open a file into a new buffer.
    /// Creates an empty buffer for non-existent files (new file workflow).
    pub fn open_file(&mut self, path: &PathBuf) -> Result<()> {
        let buffer = if path.exists() {
            Buffer::from_file(path.clone())?
        } else {
            // New file — create an empty named buffer
            let mut buf = Buffer::new(path.to_string_lossy().as_ref());
            buf.file_path = Some(path.clone());
            buf
        };
        self.buffers.push(buffer);
        self.current_buffer_idx = self.buffers.len() - 1;
        self.set_status(format!("Opened {}", path.display()));

        // Notify LSP about opened document if a server is running for this language.
        let language = LspManager::language_from_path(path);
        let text = self.current_buffer()
            .map(|b| b.lines().join("\n"))
            .unwrap_or_default();

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
    pub async fn setup_lsp(&mut self, config: &Config) {
        let workspace_root = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        tracing::info!("LSP workspace root from current_dir: {:?}", workspace_root);

        for server in &config.lsp.servers {
            let args: Vec<&str> = server.args.iter().map(|s| s.as_str()).collect();
            tracing::info!(
                "Starting LSP server '{}' for language '{}'",
                server.command,
                server.language
            );
            match self
                .lsp_manager
                .add_server(
                    server.language.clone(),
                    &server.command,
                    &args,
                    workspace_root.clone(),
                )
                .await
            {
                Err(e) => {
                    let msg = format!("LSP '{}': {}", server.command, e);
                    tracing::warn!("{}", msg);
                    self.set_status(msg);
                }
                Ok(()) => {
                    // If this is the Copilot server, immediately check auth status
                    // so we can prompt the user to sign in if needed.
                    if server.language == "copilot" {
                        if let Some(client) = self.lsp_manager.get_client("copilot") {
                            match client.copilot_check_status() {
                                Ok(rx) => { self.copilot_auth_rx = Some(rx); }
                                Err(e) => { tracing::warn!("copilot checkStatus failed: {}", e); }
                            }
                        }
                    }
                }
            }
        }

        // Files were opened before LSP was ready — send did_open for each now.
        let notifications: Vec<_> = self.buffers.iter().filter_map(|buf| {
            let path = buf.file_path.as_ref()?;
            let language = LspManager::language_from_path(path);
            let uri = LspManager::path_to_uri(path).ok()?;
            let text = buf.lines().join("\n");
            Some((language, uri, text))
        }).collect();

        for (language, uri, text) in notifications {
            if let Some(client) = self.lsp_manager.get_client(&language) {
                let _ = client.did_open(uri, language, text);
            }
        }
    }

    /// Create a scratch buffer (unnamed, not tied to a file)
    pub fn open_scratch(&mut self) {
        let buffer = Buffer::new("*scratch*");
        self.buffers.push(buffer);
        self.current_buffer_idx = self.buffers.len() - 1;
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

        loop {
            // Process LSP messages (non-blocking, capped per frame)
            let _ = self.lsp_manager.process_messages();

            // Surface any human-readable LSP messages (e.g. Copilot auth instructions).
            // These are sticky so they persist until the user presses Esc.
            for msg in self.lsp_manager.drain_messages() {
                self.set_sticky(msg);
            }

            // Update diagnostics for current buffer
            if let Some(buf) = self.current_buffer() {
                if let Some(path) = &buf.file_path {
                    if let Ok(uri) = LspManager::path_to_uri(path) {
                        self.current_diagnostics = self.lsp_manager.get_diagnostics(&uri);
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
                let status = val.get("status").and_then(|s| s.as_str()).unwrap_or("");
                tracing::info!("Copilot auth response: {:?}", val);
                match status {
                    "OK" | "AlreadySignedIn" => {
                        let user = val.get("user").and_then(|u| u.as_str()).unwrap_or("unknown");
                        self.set_sticky(format!("Copilot: signed in as {}", user));
                    }
                    "NotSignedIn" => {
                        // Auto-escalate: start the device auth flow
                        if let Some(client) = self.lsp_manager.get_client("copilot") {
                            match client.copilot_sign_in_initiate() {
                                Ok(rx) => { self.copilot_auth_rx = Some(rx); }
                                Err(e) => { self.set_sticky(format!("Copilot sign-in failed: {}", e)); }
                            }
                        }
                    }
                    "PromptUserDeviceFlow" => {
                        let uri = val.get("verificationUri").and_then(|u| u.as_str()).unwrap_or("?");
                        let code = val.get("userCode").and_then(|c| c.as_str()).unwrap_or("?");
                        self.set_sticky(format!(
                            "Copilot auth: go to {}  and enter code: {}  (Esc to dismiss)",
                            uri, code
                        ));
                    }
                    _ => {
                        self.set_sticky(format!("Copilot: {}", val));
                    }
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // ── Agent panel stream polling ─────────────────────────────────────
            self.agent_panel.poll_stream();
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
                    }
                }
            } else {
                None
            };
            if let Some(value) = completed {
                self.pending_completion = None;
                if let Some(text) = parse_first_inline_completion(value) {
                    if let Some(buf) = self.current_buffer() {
                        let row = buf.cursor.row;
                        let col = buf.cursor.col;
                        self.ghost_text = Some((text, row, col));
                    }
                }
            }
            // ──────────────────────────────────────────────────────────────────

            // Render the UI
            self.render()?;

            // Handle input
            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key(key)?;
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
        let status = self.status_message.as_deref();
        let command_buffer = if self.mode == Mode::Command {
            Some(self.command_buffer.as_str())
        } else {
            None
        };

        // Check if we should show which-key
        let show_which_key = self.key_handler.should_show_which_key();
        let which_key_options = if show_which_key {
            Some(self.key_handler.which_key_options())
        } else {
            None
        };

        // Get key sequence for display
        let key_sequence = self.key_handler.sequence();

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
            Some((self.file_list.clone(), self.file_picker_idx))
        } else {
            None
        };

        let ghost = self.ghost_text.as_ref()
            .map(|(text, row, col)| (text.as_str(), *row, *col));

        let agent_ref = if self.agent_panel.visible { Some(&self.agent_panel) } else { None };
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
            );
        })?;

        Ok(())
    }

    /// Handle a key press
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Esc always clears sticky notifications (user explicitly dismissing).
        if key.code == KeyCode::Esc {
            self.status_sticky = false;
        }
        // Clear transient status message on any new input (except sticky messages and picker modes).
        if self.mode != Mode::PickBuffer && self.mode != Mode::PickFile && !self.status_sticky {
            self.status_message = None;
        }

        match self.mode {
            Mode::Normal => self.handle_normal_mode(key)?,
            Mode::Insert => self.handle_insert_mode(key)?,
            Mode::Command => self.handle_command_mode(key)?,
            Mode::Visual => self.handle_visual_mode(key)?,
            Mode::PickBuffer => self.handle_pick_buffer_mode(key)?,
            Mode::PickFile => self.handle_pick_file_mode(key)?,
            Mode::Agent => self.handle_agent_mode(key)?,
            Mode::Explorer => self.handle_normal_mode(key)?, // placeholder until explorer is built
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
        match action {
            Action::Noop => {}
            Action::Insert => self.mode = Mode::Insert,
            Action::InsertAppend => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
                }
                self.mode = Mode::Insert;
            }
            Action::InsertLineStart => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_start();
                }
                self.mode = Mode::Insert;
            }
            Action::InsertLineEnd => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end();
                }
                self.mode = Mode::Insert;
            }
            Action::InsertNewlineBelow => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end();
                    buf.insert_newline();
                }
                self.mode = Mode::Insert;
            }
            Action::InsertNewlineAbove => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_start();
                    buf.insert_newline();
                    buf.move_cursor_up();
                }
                self.mode = Mode::Insert;
            }
            Action::MoveLeft => {
                // h — clamped, no line wrap
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_left_clamp();
                }
            }
            Action::MoveRight => {
                // l — clamped, no line wrap
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right_clamp();
                }
            }
            Action::MoveUp => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_up();
                }
            }
            Action::MoveDown => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_down();
                }
            }
            Action::MoveLineStart => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_start();
                }
            }
            Action::MoveLineEnd => {
                // Used by A / InsertLineEnd (cursor goes past last char)
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end();
                }
            }
            Action::MoveLineEndNormal => {
                // Used by $ in Normal mode (cursor lands ON last char)
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end_normal();
                }
            }
            Action::GotoFileTop => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.goto_first_line();
                }
            }
            Action::GotoFileBottom => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.goto_last_line();
                }
            }
            Action::MoveWordForward => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_word_forward();
                }
            }
            Action::MoveWordBackward => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_word_backward();
                }
            }
            Action::Command => {
                self.mode = Mode::Command;
                self.command_buffer.clear();
            }
            Action::Visual => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.start_selection();
                }
                self.mode = Mode::Visual;
            }
            Action::BufferList => {
                if self.buffers.is_empty() {
                    self.set_status("No buffers open".to_string());
                } else {
                    self.buffer_picker_idx = self.current_buffer_idx;
                    self.mode = Mode::PickBuffer;
                }
            }
            Action::BufferNext => {
                if !self.buffers.is_empty() {
                    self.current_buffer_idx = (self.current_buffer_idx + 1) % self.buffers.len();
                    self.set_status(format!("Switched to buffer: {}", self.buffers[self.current_buffer_idx].name));
                }
            }
            Action::BufferPrevious => {
                if !self.buffers.is_empty() {
                    self.current_buffer_idx = if self.current_buffer_idx == 0 {
                        self.buffers.len() - 1
                    } else {
                        self.current_buffer_idx - 1
                    };
                    self.set_status(format!("Switched to buffer: {}", self.buffers[self.current_buffer_idx].name));
                }
            }
            Action::BufferClose => {
                if !self.buffers.is_empty() {
                    let buf = &self.buffers[self.current_buffer_idx];
                    if buf.is_modified {
                        self.set_status("Buffer has unsaved changes. Save first!".to_string());
                    } else {
                        let name = buf.name.clone();
                        self.buffers.remove(self.current_buffer_idx);
                        if !self.buffers.is_empty() {
                            self.current_buffer_idx = self.current_buffer_idx.min(self.buffers.len() - 1);
                        }
                        self.set_status(format!("Closed buffer: {}", name));
                    }
                }
            }
            Action::FileFind => {
                self.scan_files();
                if self.file_list.is_empty() {
                    self.set_status("No files found".to_string());
                } else {
                    self.file_picker_idx = 0;
                    self.mode = Mode::PickFile;
                }
            }
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
            }
            Action::Quit => {
                self.check_quit()?;
            }
            Action::LspHover => {
                self.request_hover();
            }
            Action::LspGoToDefinition => {
                self.request_goto_definition();
            }
            Action::LspReferences => {
                self.request_references();
            }
            Action::LspRename => {
                self.set_status("Rename not yet implemented".to_string());
                // TODO: Implement rename workflow
            }
            Action::LspDocumentSymbols => {
                self.set_status("Document symbols not yet implemented".to_string());
                // TODO: Implement symbol picker
            }
            Action::LspNextDiagnostic => {
                self.goto_next_diagnostic();
            }
            Action::LspPrevDiagnostic => {
                self.goto_prev_diagnostic();
            }
            Action::AgentToggle => {
                self.agent_panel.toggle_visible();
                if self.agent_panel.visible {
                    self.mode = Mode::Agent;
                } else {
                    self.mode = Mode::Normal;
                }
            }
            Action::AgentFocus => {
                if !self.agent_panel.visible {
                    self.agent_panel.visible = true;
                }
                self.agent_panel.focus();
                self.mode = Mode::Agent;
            }
            Action::ExplorerToggle => {
                self.set_status("Explorer not yet implemented".to_string());
            }
            Action::ExplorerFocus => {
                self.set_status("Explorer not yet implemented".to_string());
            }
            // ── Edit operations ───────────────────────────────────────────────
            Action::DeleteChar => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_at_cursor();
                }
                self.notify_lsp_change();
            }
            Action::DeleteLine => {
                let deleted = self.current_buffer_mut()
                    .map(|buf| buf.delete_current_line());
                if let Some(text) = deleted {
                    self.clipboard = Some(text);
                }
                self.notify_lsp_change();
            }
            Action::DeleteToLineEnd => {
                let deleted = self.current_buffer_mut()
                    .map(|buf| buf.delete_to_line_end());
                if let Some(text) = deleted {
                    self.clipboard = Some(text);
                }
                self.notify_lsp_change();
            }
            Action::YankLine => {
                let yanked = self.current_buffer()
                    .map(|buf| buf.yank_current_line());
                if let Some(text) = yanked {
                    self.clipboard = Some(text);
                    self.set_status("Line yanked".to_string());
                }
            }
            Action::PasteAfter => {
                if let Some(text) = self.clipboard.clone() {
                    if let Some(buf) = self.current_buffer_mut() {
                        buf.paste_after_cursor(&text);
                    }
                    self.notify_lsp_change();
                }
            }
            Action::PasteBefore => {
                if let Some(text) = self.clipboard.clone() {
                    if let Some(buf) = self.current_buffer_mut() {
                        buf.paste_before_cursor(&text);
                    }
                    self.notify_lsp_change();
                }
            }
            Action::Undo => {
                self.set_status("Undo not yet implemented".to_string());
            }
        }
        Ok(())
    }

    /// Handle keys in Visual mode
    fn handle_visual_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.clear_selection();
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_left();
                    buf.update_selection();
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
                    buf.update_selection();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_up();
                    buf.update_selection();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_down();
                    buf.update_selection();
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle keys in PickBuffer mode
    fn handle_pick_buffer_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status_message = None;
            }
            KeyCode::Enter => {
                self.current_buffer_idx = self.buffer_picker_idx;
                self.mode = Mode::Normal;
                self.status_message = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.buffer_picker_idx > 0 {
                    self.buffer_picker_idx -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.buffer_picker_idx + 1 < self.buffers.len() {
                    self.buffer_picker_idx += 1;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle keys in PickFile mode
    fn handle_pick_file_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status_message = None;
            }
            KeyCode::Enter => {
                if let Some(path) = self.file_list.get(self.file_picker_idx) {
                    let path_clone = path.clone();
                    self.mode = Mode::Normal;
                    self.open_file(&path_clone)?;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.file_picker_idx > 0 {
                    self.file_picker_idx -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.file_picker_idx + 1 < self.file_list.len() {
                    self.file_picker_idx += 1;
                }
            }
            _ => {}
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
            }
            // Tab — toggle focus back to editor without closing.
            KeyCode::Tab => {
                self.agent_panel.blur();
                self.mode = Mode::Normal;
            }
            // Enter — submit the input.
            KeyCode::Enter => {
                // Snapshot current buffer content as context.
                let context = self.current_buffer().map(|buf| buf.lines().join("\n"));
                // Submit is async; spawn a task and let the stream_rx handle tokens.
                let panel = &mut self.agent_panel;
                // We need a blocking submit here.  Use a one-shot channel via block_in_place
                // or simply call submit synchronously via tokio::task::block_in_place.
                // Since we are inside an async context, we use a local async block.
                let fut = panel.submit(context);
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
            }
            // Backspace — delete last input character.
            KeyCode::Backspace => {
                self.agent_panel.input_backspace();
            }
            // Scroll history.
            KeyCode::Up => self.agent_panel.scroll_up(),
            KeyCode::Down => self.agent_panel.scroll_down(),
            // Regular characters — handle special agent commands before appending to input.
            KeyCode::Char(ch) => {
                // 'a' with empty input = apply code block from latest reply.
                if ch == 'a' && self.agent_panel.input.is_empty() {
                    if let Some(code) = self.agent_panel.get_code_to_apply() {
                        let line_count = code.lines().count();
                        if let Some(buf) = self.current_buffer_mut() {
                            buf.insert_text_block(&code);
                        }
                        // Return focus to the editor so the user can see the applied code.
                        self.agent_panel.blur();
                        self.mode = Mode::Normal;
                        self.set_status(format!(
                            "Applied {} lines from Copilot (Tab or SPC-a-a to return)",
                            line_count
                        ));
                    } else {
                        self.set_status("No code block in latest reply to apply".to_string());
                    }
                } else {
                    self.agent_panel.input_char(ch);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Scan filesystem for files (excluding common ignored directories)
    fn scan_files(&mut self) {
        self.file_list.clear();
        
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.scan_directory(&current_dir, 0);
        
        // Sort files by name for easier navigation
        self.file_list.sort();
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
            let file_name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // Skip hidden files, common build dirs, and IDE folders
            if file_name.starts_with('.') 
                || file_name == "target" 
                || file_name == "node_modules"
                || file_name == "dist"
                || file_name == "build" {
                continue;
            }

            if path.is_file() {
                // Skip binary and lock files
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_str().unwrap_or("");
                    if ext_str == "lock" || ext_str == "exe" || ext_str == "dll" || ext_str == "so" {
                        continue;
                    }
                }
                self.file_list.push(path);
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
                    let cursor_matches = self.current_buffer()
                        .map(|b| b.cursor.row == row && b.cursor.col == col)
                        .unwrap_or(false);
                    if cursor_matches {
                        for ch in text.chars() {
                            if ch == '\n' {
                                if let Some(buf) = self.current_buffer_mut() {
                                    buf.insert_newline();
                                }
                            } else {
                                if let Some(buf) = self.current_buffer_mut() {
                                    buf.insert_char(ch);
                                }
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
            }
            KeyCode::Esc => {
                // Clear ghost text when leaving Insert mode.
                self.ghost_text = None;
                self.pending_completion = None;
                self.last_edit_instant = None;
                self.mode = Mode::Normal;
                false
            }
            KeyCode::Char(c) => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.insert_char(c);
                }
                true
            }
            KeyCode::Enter => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.insert_newline();
                }
                true
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_before();
                }
                true
            }
            KeyCode::Delete => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_at();
                }
                true
            }
            KeyCode::Left => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_left();
                }
                false
            }
            KeyCode::Right => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
                }
                false
            }
            KeyCode::Up => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_up();
                }
                false
            }
            KeyCode::Down => {
                self.ghost_text = None;
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_down();
                }
                false
            }
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
            }
            KeyCode::Enter => {
                self.execute_command()?;
                self.mode = Mode::Normal;
                self.command_buffer.clear();
            }
            KeyCode::Char(c) => {
                self.command_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.command_buffer.pop();
            }
            _ => {}
        }

        Ok(())
    }

    /// Execute a command entered in command mode
    fn execute_command(&mut self) -> Result<()> {
        let cmd = self.command_buffer.trim();

        match cmd {
            "q" | "quit" => {
                self.check_quit()?;
            }
            "q!" | "quit!" => {
                self.should_quit = true;
            }
            "w" | "write" => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.save()?;
                    self.set_status("File saved".to_string());
                }
            }
            "wq" => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.save()?;
                }
                self.should_quit = true;
            }
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
            }
            "copilot auth" => {
                // Re-run the auth check + sign-in initiate flow manually.
                if let Some(client) = self.lsp_manager.get_client("copilot") {
                    match client.copilot_check_status() {
                        Ok(rx) => {
                            self.copilot_auth_rx = Some(rx);
                            self.set_status("Copilot: checking auth status…".to_string());
                        }
                        Err(e) => {
                            self.set_status(format!("Copilot auth error: {}", e));
                        }
                    }
                } else {
                    self.set_status("Copilot: server not connected (check config.toml)".to_string());
                }
            }
            _ => {
                self.set_status(format!("Unknown command: {}", cmd));
            }
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

    /// Request hover information at cursor position
    fn request_hover(&mut self) {
        // Get current buffer and position
        let (uri, position) = match self.get_current_lsp_position() {
            Some(pos) => pos,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            }
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
            }
        };

        self.set_status("Go-to-definition requested (not yet fully implemented)".to_string());
    }

    /// Request find references at cursor position
    fn request_references(&mut self) {
        let (uri, position) = match self.get_current_lsp_position() {
            Some(pos) => pos,
            None => {
                self.set_status("No file open or LSP not available".to_string());
                return;
            }
        };

        self.set_status("Find references requested (not yet fully implemented)".to_string());
    }

    /// Go to next diagnostic in current buffer
    fn goto_next_diagnostic(&mut self) {
        if self.current_diagnostics.is_empty() {
            self.set_status("No diagnostics".to_string());
            return;
        }

        let current_line = self.current_buffer()
            .map(|buf| buf.cursor.row)
            .unwrap_or(0);
        
        // Find next diagnostic after current line and extract position
        let next_diag = self.current_diagnostics
            .iter()
            .find(|d| d.range.start.line as usize > current_line)
            .map(|d| (d.range.start.line as usize, d.range.start.character as usize, d.message.clone()));

        if let Some((row, col, msg)) = next_diag {
            if let Some(buf) = self.current_buffer_mut() {
                buf.cursor.row = row;
                buf.cursor.col = col;
                buf.ensure_cursor_visible();
            }
            self.set_status(format!("Diagnostic: {}", msg));
        } else {
            // Wrap around to first diagnostic
            let first_diag = self.current_diagnostics.first()
                .map(|d| (d.range.start.line as usize, d.range.start.character as usize, d.message.clone()));
            
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

        let current_line = self.current_buffer()
            .map(|buf| buf.cursor.row)
            .unwrap_or(0);
        
        // Find previous diagnostic before current line and extract position
        let prev_diag = self.current_diagnostics
            .iter()
            .rev()
            .find(|d| (d.range.start.line as usize) < current_line)
            .map(|d| (d.range.start.line as usize, d.range.start.character as usize, d.message.clone()));

        if let Some((row, col, msg)) = prev_diag {
            if let Some(buf) = self.current_buffer_mut() {
                buf.cursor.row = row;
                buf.cursor.col = col;
                buf.ensure_cursor_visible();
            }
            self.set_status(format!("Diagnostic: {}", msg));
        } else {
            // Wrap around to last diagnostic
            let last_diag = self.current_diagnostics.last()
                .map(|d| (d.range.start.line as usize, d.range.start.character as usize, d.message.clone()));
            
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
        let position = lsp_types::Position {
            line: buf.cursor.row as u32,
            character: buf.cursor.col as u32,
        };
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
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
