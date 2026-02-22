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

use crate::buffer::Buffer;
use crate::keymap::{Action, KeyHandler, Mode};
use crate::ui::UI;

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
        })
    }

    /// Open a file into a new buffer
    pub fn open_file(&mut self, path: &PathBuf) -> Result<()> {
        let buffer = Buffer::from_file(path.clone())?;
        self.buffers.push(buffer);
        self.current_buffer_idx = self.buffers.len() - 1;
        self.set_status(format!("Opened {}", path.display()));
        Ok(())
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
        loop {
            // Render the UI
            self.render()?;

            // Handle input
            if event::poll(std::time::Duration::from_millis(100))? {
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
            );
        })?;

        Ok(())
    }

    /// Handle a key press
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Clear status message on new input (except in picker modes)
        if self.mode != Mode::PickBuffer && self.mode != Mode::PickFile {
            self.status_message = None;
        }

        match self.mode {
            Mode::Normal => self.handle_normal_mode(key)?,
            Mode::Insert => self.handle_insert_mode(key)?,
            Mode::Command => self.handle_command_mode(key)?,
            Mode::Visual => self.handle_visual_mode(key)?,
            Mode::PickBuffer => self.handle_pick_buffer_mode(key)?,
            Mode::PickFile => self.handle_pick_file_mode(key)?,
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_left();
                }
            }
            Action::MoveRight => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_line_end();
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
                if let Some(buf) = self.current_buffer_mut() {
                    buf.save()?;
                    self.set_status("File saved".to_string());
                }
            }
            Action::Quit => {
                self.check_quit()?;
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
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char(c) => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.insert_char(c);
                }
            }
            KeyCode::Enter => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.insert_newline();
                }
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_before();
                }
            }
            KeyCode::Delete => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.delete_char_at();
                }
            }
            KeyCode::Left => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_left();
                }
            }
            KeyCode::Right => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_right();
                }
            }
            KeyCode::Up => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_up();
                }
            }
            KeyCode::Down => {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.move_cursor_down();
                }
            }
            _ => {}
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

    /// Set a status message to display
    fn set_status(&mut self, msg: String) {
        self.status_message = Some(msg);
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
