use std::path::PathBuf;
use crate::buffer::cursor::Cursor;
use crate::buffer::history::{EditHistory, EditOp};

/// Selection range for visual mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub start: Cursor,
    pub end: Cursor,
}

impl Selection {
    pub fn new(start: Cursor, end: Cursor) -> Self {
        Self { start, end }
    }

    /// Return normalized selection (start before end)
    pub fn normalized(&self) -> (Cursor, Cursor) {
        if self.start.row < self.end.row || 
           (self.start.row == self.end.row && self.start.col <= self.end.col) {
            (self.start.clone(), self.end.clone())
        } else {
            (self.end.clone(), self.start.clone())
        }
    }
}

/// A Buffer holds the content of a single file or virtual document.
/// All editing operations go through Buffer — it is the source of truth.
#[derive(Debug, Clone)]
pub struct Buffer {
    /// Internal name, e.g. "*scratch*" or the file path
    pub name: String,

    /// The file this buffer is associated with, if any
    pub file_path: Option<PathBuf>,

    /// The actual text content, stored as a Vec of lines.
    /// Each line does NOT include a trailing newline.
    lines: Vec<String>,

    /// Current cursor position
    pub cursor: Cursor,

    /// Visual mode selection (if any)
    pub selection: Option<Selection>,

    /// Whether the buffer has unsaved changes
    pub is_modified: bool,

    /// LSP document version (incremented on each change)
    pub lsp_version: i32,

    /// Edit history for undo/redo
    history: EditHistory,

    /// Horizontal scroll offset (column)
    pub scroll_col: usize,

    /// Vertical scroll offset (row)
    pub scroll_row: usize,
}

impl Buffer {
    /// Create a new, empty buffer with the given name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            file_path: None,
            lines: vec![String::new()],
            cursor: Cursor::default(),
            selection: None,
            is_modified: false,
            lsp_version: 0,
            history: EditHistory::new(),
            scroll_col: 0,
            scroll_row: 0,
        }
    }

    /// Create a buffer from a file path, loading its content
    pub fn from_file(path: PathBuf) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(&path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            // Normalise line endings
            content
                .replace("\r\n", "\n")
                .split('\n')
                // If the file ends with \n, split produces a trailing empty string — drop it
                .collect::<Vec<_>>()
                .into_iter()
                .enumerate()
                .filter_map(|(i, l)| {
                    // Keep all lines; just strip the phantom trailing empty line
                    let is_last_empty = i == content.replace("\r\n", "\n").matches('\n').count()
                        && l.is_empty();
                    if is_last_empty { None } else { Some(l.to_string()) }
                })
                .collect()
        };

        let lines = if lines.is_empty() { vec![String::new()] } else { lines };

        Ok(Self {
            name,
            file_path: Some(path),
            lines,
            cursor: Cursor::default(),
            selection: None,
            lsp_version: 0,
            is_modified: false,
            history: EditHistory::new(),
            scroll_col: 0,
            scroll_row: 0,
        })
    }

    /// Reload this buffer's content from disk, preserving cursor position (clamped).
    /// Does nothing (returns Ok) if the buffer has no associated file path.
    pub fn reload_from_disk(&mut self) -> anyhow::Result<()> {
        let path = match self.file_path.clone() {
            Some(p) => p,
            None => return Ok(()),
        };
        let content = std::fs::read_to_string(&path)?;
        let new_lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            let normalised = content.replace("\r\n", "\n");
            let mut v: Vec<String> = normalised.split('\n').map(|l| l.to_string()).collect();
            // Drop the phantom empty line that split() appends when file ends with \n
            if v.last().map(|l| l.is_empty()).unwrap_or(false) {
                v.pop();
            }
            if v.is_empty() { vec![String::new()] } else { v }
        };
        self.lines = new_lines;
        self.is_modified = false;
        // Clamp cursor so it stays in bounds after a potential line-count change
        self.cursor.row = self.cursor.row.min(self.lines.len().saturating_sub(1));
        let row = self.cursor.row;
        self.cursor.col = self.cursor.col.min(
            self.lines[row].len().saturating_sub(1)
        );
        tracing::info!("Reloaded buffer '{}' from disk", self.name);
        Ok(())
    }

    /// Save the buffer back to its associated file
    pub fn save(&mut self) -> anyhow::Result<()> {
        let path = self
            .file_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Buffer '{}' has no file path", self.name))?
            .clone();

        let content = self.lines.join("\n") + "\n";
        std::fs::write(&path, content)?;
        self.is_modified = false;
        tracing::info!("Saved buffer '{}' to {:?}", self.name, path);
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Line access
    // -------------------------------------------------------------------------

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, row: usize) -> Option<&str> {
        self.lines.get(row).map(|s| s.as_str())
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    // -------------------------------------------------------------------------
    // Text insertion / deletion
    // -------------------------------------------------------------------------

    /// Mark buffer as modified and increment LSP version
    fn mark_modified(&mut self) {
        self.is_modified = true;
        self.lsp_version += 1;
    }

    // -------------------------------------------------------------------------
    // Normal-mode edit operations (delete, yank, paste, goto)
    // -------------------------------------------------------------------------

    /// Delete the character at the cursor position (Normal-mode `x`).
    pub fn delete_char_at_cursor(&mut self) {
        self.delete_char_at();
    }

    /// Delete the entire current line and return it (Normal-mode `dd`).
    /// Cursor stays on the same row (or the last row if the last line was deleted).
    pub fn delete_current_line(&mut self) -> String {
        let row = self.cursor.row;
        let deleted = self.lines.remove(row);

        // Ensure at least one line always exists.
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        // Clamp cursor row.
        self.cursor.row = row.min(self.lines.len() - 1);
        self.clamp_cursor_col();
        self.mark_modified();
        deleted
    }

    /// Delete from cursor to end of line and return the deleted text (Normal-mode `D`).
    pub fn delete_to_line_end(&mut self) -> String {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let byte_idx = char_to_byte_idx(&self.lines[row], col);
        let deleted = self.lines[row].split_off(byte_idx);
        self.mark_modified();
        deleted
    }

    /// Return the content of the current line without modifying the buffer (Normal-mode `yy`).
    pub fn yank_current_line(&self) -> String {
        self.lines[self.cursor.row].clone()
    }

    /// Insert `text` as a new line BELOW the cursor row (Normal-mode `p`).
    pub fn paste_after_cursor(&mut self, text: &str) {
        let row = self.cursor.row;
        self.lines.insert(row + 1, text.to_string());
        self.cursor.row = row + 1;
        self.cursor.col = 0;
        self.mark_modified();
    }

    /// Insert `text` as a new line ABOVE the cursor row (Normal-mode `P`).
    pub fn paste_before_cursor(&mut self, text: &str) {
        let row = self.cursor.row;
        self.lines.insert(row, text.to_string());
        // cursor row stays the same (now points at the pasted line).
        self.cursor.col = 0;
        self.mark_modified();
    }

    /// Move cursor to the first line of the buffer (Normal-mode `gg`).
    pub fn goto_first_line(&mut self) {
        self.cursor.row = 0;
        self.cursor.col = 0;
        self.scroll_row = 0;
    }

    /// Move cursor to the last line of the buffer (Normal-mode `G`).
    pub fn goto_last_line(&mut self) {
        self.cursor.row = self.lines.len().saturating_sub(1);
        self.clamp_cursor_col();
    }

    /// Insert a multi-line block of text at the current cursor position.
    ///
    /// - The first line of `text` is appended to the current line at the cursor column.
    /// - Subsequent lines are inserted as new lines below.
    /// - The cursor ends up at the last inserted column.
    pub fn insert_text_block(&mut self, text: &str) {
        let input_lines: Vec<&str> = text.lines().collect();
        if input_lines.is_empty() {
            return;
        }

        let row = self.cursor.row;
        let col = self.cursor.col;

        // Split the current line at the cursor.
        let byte_idx = char_to_byte_idx(&self.lines[row], col);
        let tail = self.lines[row].split_off(byte_idx);

        // Append the first input line to the current line.
        self.lines[row].push_str(input_lines[0]);

        if input_lines.len() == 1 {
            // Single-line insertion: re-attach the tail.
            let new_col = col + input_lines[0].chars().count();
            self.lines[row].push_str(&tail);
            self.cursor.col = new_col;
        } else {
            // Multi-line: insert all middle lines and then the last + tail.
            let last_input = input_lines[input_lines.len() - 1];
            let last_col = last_input.chars().count();
            let mut last_line = last_input.to_string();
            last_line.push_str(&tail);

            // Insert middle lines (indices 1..len-1).
            for (i, &line) in input_lines[1..input_lines.len() - 1].iter().enumerate() {
                self.lines.insert(row + 1 + i, line.to_string());
            }
            // Insert the last line.
            self.lines.insert(row + input_lines.len() - 1, last_line);

            self.cursor.row = row + input_lines.len() - 1;
            self.cursor.col = last_col;
        }

        self.mark_modified();
    }

    /// Insert a single character at the current cursor position
    pub fn insert_char(&mut self, ch: char) {
        let row = self.cursor.row;
        let col = self.cursor.col;

        if ch == '\n' {
            self.insert_newline();
            return;
        }

        let line = &mut self.lines[row];
        // Clamp col to actual char boundary
        let byte_idx = char_to_byte_idx(line, col);
        line.insert(byte_idx, ch);

        self.cursor.col += 1;
        self.mark_modified();

        self.history.record(EditOp::InsertChar { row, col, ch });
    }

    /// Insert a newline at the current cursor position, splitting the current line
    pub fn insert_newline(&mut self) {
        let row = self.cursor.row;
        let col = self.cursor.col;

        let byte_idx = char_to_byte_idx(&self.lines[row], col);
        let tail = self.lines[row].split_off(byte_idx);
        self.lines.insert(row + 1, tail);

        self.cursor.row += 1;
        self.cursor.col = 0;
        self.mark_modified();

        self.history.record(EditOp::InsertNewline { row, col });
    }

    /// Delete the character before the cursor (backspace)
    pub fn delete_char_before(&mut self) {
        let row = self.cursor.row;
        let col = self.cursor.col;

        if col == 0 {
            if row == 0 {
                return; // Nothing to delete
            }
            // Merge this line with the previous one
            let current_line = self.lines.remove(row);
            let prev_len = self.lines[row - 1].chars().count();
            self.lines[row - 1].push_str(&current_line);
            self.cursor.row -= 1;
            self.cursor.col = prev_len;
        } else {
            let line = &mut self.lines[row];
            let byte_idx = char_to_byte_idx(line, col);
            // Find the start of the previous char
            let prev_byte = line[..byte_idx]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            line.remove(prev_byte);
            self.cursor.col -= 1;
        }

        self.mark_modified();
        self.history.record(EditOp::DeleteCharBefore { row, col });
    }

    /// Delete the character at the cursor position (delete key)
    pub fn delete_char_at(&mut self) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let line_len = self.lines[row].chars().count();

        if col >= line_len {
            if row + 1 >= self.lines.len() {
                return; // Nothing to delete at end of last line
            }
            // Merge next line into current
            let next_line = self.lines.remove(row + 1);
            self.lines[row].push_str(&next_line);
        } else {
            let line = &mut self.lines[row];
            let byte_idx = char_to_byte_idx(line, col);
            line.remove(byte_idx);
        }

        self.mark_modified();
        self.history.record(EditOp::DeleteCharAt { row, col });
    }

    // -------------------------------------------------------------------------
    // Cursor movement (clamped to valid positions)
    // -------------------------------------------------------------------------

    pub fn move_cursor_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.cursor.col = self.current_line_len();
        }
    }

    pub fn move_cursor_right(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor.col < line_len {
            self.cursor.col += 1;
        } else if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_cursor_up(&mut self) {
        if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.clamp_cursor_col();
        }
    }

    pub fn move_cursor_down(&mut self) {
        if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.clamp_cursor_col();
        }
    }

    pub fn move_cursor_line_start(&mut self) {
        self.cursor.col = 0;
    }

    pub fn move_cursor_line_end(&mut self) {
        self.cursor.col = self.current_line_len();
    }

    /// Move cursor to last character on the line (Normal-mode `$`).
    /// Stays on the last character rather than moving past it.
    pub fn move_cursor_line_end_normal(&mut self) {
        let len = self.current_line_len();
        self.cursor.col = if len == 0 { 0 } else { len - 1 };
    }

    /// Move cursor left without wrapping to the previous line (vim `h`).
    pub fn move_cursor_left_clamp(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    /// Move cursor right without wrapping to the next line (vim `l`).
    pub fn move_cursor_right_clamp(&mut self) {
        let max = self.current_line_len().saturating_sub(1);
        if self.cursor.col < max {
            self.cursor.col += 1;
        }
    }

    pub fn move_cursor_word_forward(&mut self) {
        let line = &self.lines[self.cursor.row];
        let chars: Vec<char> = line.chars().collect();
        let mut col = self.cursor.col;

        if col >= chars.len() {
            // Move to next line if at end
            if self.cursor.row + 1 < self.lines.len() {
                self.cursor.row += 1;
                self.cursor.col = 0;
            }
            return;
        }

        // Skip whitespace
        while col < chars.len() && chars[col].is_whitespace() {
            col += 1;
        }

        // Skip word characters
        while col < chars.len() && !chars[col].is_whitespace() {
            col += 1;
        }

        self.cursor.col = col;
    }

    pub fn move_cursor_word_backward(&mut self) {
        if self.cursor.col == 0 {
            // Move to end of previous line
            if self.cursor.row > 0 {
                self.cursor.row -= 1;
                self.cursor.col = self.current_line_len();
            }
            return;
        }

        let line = &self.lines[self.cursor.row];
        let chars: Vec<char> = line.chars().collect();
        let mut col = self.cursor.col.saturating_sub(1);

        // Skip whitespace backwards
        while col > 0 && chars[col].is_whitespace() {
            col = col.saturating_sub(1);
        }

        // Skip word characters backwards
        while col > 0 && !chars[col].is_whitespace() {
            col = col.saturating_sub(1);
        }

        // Adjust if we stopped on whitespace
        if col > 0 || chars[0].is_whitespace() {
            col += 1;
        }

        self.cursor.col = col;
    }

    pub fn move_cursor_to(&mut self, row: usize, col: usize) {
        self.cursor.row = row.min(self.lines.len().saturating_sub(1));
        self.clamp_cursor_col_at(col);
    }

    // -------------------------------------------------------------------------
    // Visual mode / Selection
    // -------------------------------------------------------------------------

    pub fn start_selection(&mut self) {
        self.selection = Some(Selection::new(self.cursor.clone(), self.cursor.clone()));
    }

    pub fn update_selection(&mut self) {
        if let Some(sel) = &mut self.selection {
            sel.end = self.cursor.clone();
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Ensure cursor is visible with default viewport size
    /// This is a convenience method for when exact viewport size is unknown
    pub fn ensure_cursor_visible(&mut self) {
        // Use reasonable defaults (can be improved later with actual terminal size)
        self.scroll_to_cursor(30, 80);
    }

    // -------------------------------------------------------------------------
    // Scrolling helpers (called by the renderer to keep cursor in view)
    // -------------------------------------------------------------------------

    pub fn scroll_to_cursor(&mut self, viewport_rows: usize, viewport_cols: usize) {
        // Vertical
        if self.cursor.row < self.scroll_row {
            self.scroll_row = self.cursor.row;
        } else if self.cursor.row >= self.scroll_row + viewport_rows {
            self.scroll_row = self.cursor.row.saturating_sub(viewport_rows - 1);
        }

        // Horizontal
        if self.cursor.col < self.scroll_col {
            self.scroll_col = self.cursor.col;
        } else if self.cursor.col >= self.scroll_col + viewport_cols {
            self.scroll_col = self.cursor.col.saturating_sub(viewport_cols - 1);
        }
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    fn current_line_len(&self) -> usize {
        self.lines[self.cursor.row].chars().count()
    }

    fn clamp_cursor_col(&mut self) {
        let max = self.current_line_len();
        if self.cursor.col > max {
            self.cursor.col = max;
        }
    }

    fn clamp_cursor_col_at(&mut self, col: usize) {
        let max = self.current_line_len();
        self.cursor.col = col.min(max);
    }
}

/// Convert a char-index `col` to a UTF-8 byte index within `s`.
/// If `col` exceeds the string's char count, returns `s.len()`.
pub fn char_to_byte_idx(s: &str, col: usize) -> usize {
    s.char_indices()
        .nth(col)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_char() {
        let mut buf = Buffer::new("test");
        buf.insert_char('H');
        buf.insert_char('i');
        assert_eq!(buf.line(0), Some("Hi"));
        assert_eq!(buf.cursor.col, 2);
    }

    #[test]
    fn test_insert_newline_splits_line() {
        let mut buf = Buffer::new("test");
        buf.insert_char('A');
        buf.insert_char('B');
        buf.cursor.col = 1;
        buf.insert_newline();
        assert_eq!(buf.line(0), Some("A"));
        assert_eq!(buf.line(1), Some("B"));
        assert_eq!(buf.cursor.row, 1);
        assert_eq!(buf.cursor.col, 0);
    }

    #[test]
    fn test_delete_char_before_merges_lines() {
        let mut buf = Buffer::new("test");
        buf.insert_char('A');
        buf.insert_newline();
        buf.insert_char('B');
        // Cursor: row=1, col=1 — backspace twice to merge
        buf.delete_char_before(); // removes 'B'
        buf.delete_char_before(); // merges lines
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line(0), Some("A"));
    }
}
