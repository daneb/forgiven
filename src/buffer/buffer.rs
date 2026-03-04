use crate::buffer::cursor::Cursor;
use crate::buffer::history::EditHistory;
use std::path::PathBuf;

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
        if self.start.row < self.end.row
            || (self.start.row == self.end.row && self.start.col <= self.end.col)
        {
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

    /// Anchor row for Visual Line mode (`V`).
    /// Kept separately from `selection.start` so up/down movement can always
    /// recompute the correct inclusive range without losing the anchor.
    pub visual_line_anchor: Option<usize>,

    /// Current in-file search pattern
    pub search_pattern: Option<String>,

    /// Cached search matches: (row, col, match_length)
    pub search_matches: Vec<(usize, usize, usize)>,

    /// Current match index (for n/N navigation)
    pub current_match_idx: Option<usize>,
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
            visual_line_anchor: None,
            search_pattern: None,
            search_matches: Vec::new(),
            current_match_idx: None,
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
                    let is_last_empty =
                        i == content.replace("\r\n", "\n").matches('\n').count() && l.is_empty();
                    if is_last_empty {
                        None
                    } else {
                        Some(l.to_string())
                    }
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
            visual_line_anchor: None,
            search_pattern: None,
            search_matches: Vec::new(),
            current_match_idx: None,
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
            if v.is_empty() {
                vec![String::new()]
            } else {
                v
            }
        };
        self.lines = new_lines;
        self.is_modified = false;
        // Clamp cursor so it stays in bounds after a potential line-count change
        self.cursor.row = self.cursor.row.min(self.lines.len().saturating_sub(1));
        let row = self.cursor.row;
        self.cursor.col = self.cursor.col.min(self.lines[row].len().saturating_sub(1));
        tracing::info!("Reloaded buffer '{}' from disk", self.name);
        Ok(())
    }

    /// Replace entire buffer content in-memory (used by apply-diff).
    /// Clamps cursor, increments lsp_version, marks modified.
    pub fn replace_all_lines(&mut self, new_lines: Vec<String>) {
        self.lines = if new_lines.is_empty() { vec![String::new()] } else { new_lines };
        self.is_modified = true;
        self.lsp_version += 1;
        self.cursor.row = self.cursor.row.min(self.lines.len().saturating_sub(1));
        let row = self.cursor.row;
        self.cursor.col = self.cursor.col.min(self.lines[row].len());
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

    #[allow(dead_code)]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    #[allow(dead_code)]
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
    // Undo / redo
    // -------------------------------------------------------------------------

    /// Save the current buffer state to the undo history.
    /// Call this **once** before any mutating action, not per-keystroke.
    /// For Insert mode, call it when *entering* Insert (not on each character),
    /// so the whole Insert session is a single undo step.
    pub fn save_undo_snapshot(&mut self) {
        self.history.save(&self.lines, self.cursor.row, self.cursor.col);
    }

    /// Undo the most recent action.  Returns `true` if a snapshot was available.
    pub fn undo(&mut self) -> bool {
        let snap = self.history.undo(&self.lines, self.cursor.row, self.cursor.col);
        if let Some(s) = snap {
            self.lines = s.lines;
            self.cursor.row = s.cursor_row;
            self.cursor.col = s.cursor_col;
            self.clamp_cursor_col();
            self.is_modified = true;
            self.lsp_version += 1;
            true
        } else {
            false
        }
    }

    /// Redo the most recently undone action.  Returns `true` if available.
    pub fn redo(&mut self) -> bool {
        let snap = self.history.redo(&self.lines, self.cursor.row, self.cursor.col);
        if let Some(s) = snap {
            self.lines = s.lines;
            self.cursor.row = s.cursor_row;
            self.cursor.col = s.cursor_col;
            self.clamp_cursor_col();
            self.is_modified = true;
            self.lsp_version += 1;
            true
        } else {
            false
        }
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn yank_current_line(&self) -> String {
        self.lines[self.cursor.row].clone()
    }

    /// Line-wise paste BELOW the cursor row (`yy`/`dd` → `p`).
    /// Splits text on `\n` so multi-line content becomes multiple real lines.
    pub fn paste_linewise_after(&mut self, text: &str) {
        let parts: Vec<&str> = text.split('\n').collect();
        let row = self.cursor.row;
        for (i, part) in parts.iter().enumerate() {
            self.lines.insert(row + 1 + i, part.to_string());
        }
        self.cursor.row = row + 1;
        self.cursor.col = 0;
        self.mark_modified();
    }

    /// Line-wise paste ABOVE the cursor row (`yy`/`dd` → `P`).
    pub fn paste_linewise_before(&mut self, text: &str) {
        let parts: Vec<&str> = text.split('\n').collect();
        let row = self.cursor.row;
        for (i, part) in parts.iter().enumerate() {
            self.lines.insert(row + i, part.to_string());
        }
        // cursor stays pointing at first inserted line
        self.cursor.col = 0;
        self.mark_modified();
    }

    /// Char-wise paste AFTER the cursor character (`yw`/`y$`/visual → `p`).
    /// Advances one column then calls insert_text_block (handles multi-line).
    pub fn paste_charwise_after(&mut self, text: &str) {
        let line_len = self.current_line_len();
        if self.cursor.col < line_len {
            self.cursor.col += 1;
        }
        self.insert_text_block(text);
    }

    /// Char-wise paste AT the cursor position (`yw`/`y$`/visual → `P`).
    pub fn paste_charwise_before(&mut self, text: &str) {
        self.insert_text_block(text);
    }

    // -------------------------------------------------------------------------
    // Word-motion helpers (yw / dw)
    // -------------------------------------------------------------------------

    /// Column AFTER the end of the current word — the exclusive end for yw/dw.
    /// Mirrors vim's `w` motion: skip the current token, then trailing spaces.
    fn word_end_col(&self) -> usize {
        let chars: Vec<char> = self.lines[self.cursor.row].chars().collect();
        let len = chars.len();
        let mut col = self.cursor.col;

        if col >= len {
            return col;
        }

        let is_word = |c: char| c.is_alphanumeric() || c == '_';

        if is_word(chars[col]) {
            while col < len && is_word(chars[col]) {
                col += 1;
            }
        } else if chars[col].is_whitespace() {
            while col < len && chars[col].is_whitespace() {
                col += 1;
            }
        } else {
            // Punctuation / operator run
            while col < len && !is_word(chars[col]) && !chars[col].is_whitespace() {
                col += 1;
            }
        }
        // Consume trailing spaces (like vim `dw`)
        while col < len && chars[col] == ' ' {
            col += 1;
        }
        col
    }

    /// Return the text from the cursor to the end of the current word (yw).
    pub fn yank_word(&self) -> String {
        let end = self.word_end_col();
        self.lines[self.cursor.row]
            .chars()
            .skip(self.cursor.col)
            .take(end.saturating_sub(self.cursor.col))
            .collect()
    }

    /// Return text from cursor to end of line (y$).
    pub fn yank_to_line_end(&self) -> String {
        self.lines[self.cursor.row].chars().skip(self.cursor.col).collect()
    }

    /// Remove and return text from cursor to end of word (dw).
    pub fn delete_word(&mut self) -> String {
        let end = self.word_end_col();
        let row = self.cursor.row;
        let col = self.cursor.col;
        let chars: Vec<char> = self.lines[row].chars().collect();
        let deleted: String = chars[col..end].iter().collect();
        let new_line: String = chars[..col].iter().chain(&chars[end..]).collect();
        self.lines[row] = new_line;
        self.mark_modified();
        deleted
    }

    // -------------------------------------------------------------------------
    // Visual selection extract / delete
    // -------------------------------------------------------------------------

    /// Return the text covered by the current selection without modifying the buffer.
    pub fn yank_selection(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.normalized();
        let mut result = String::new();

        if start.row == end.row {
            let chars: Vec<char> = self.lines[start.row].chars().collect();
            let from = start.col.min(chars.len());
            let to = (end.col + 1).min(chars.len());
            result.extend(&chars[from..to]);
        } else {
            // First row: from start.col to EOL
            let first: Vec<char> = self.lines[start.row].chars().collect();
            result.extend(&first[start.col.min(first.len())..]);
            result.push('\n');
            // Middle rows
            for row in (start.row + 1)..end.row {
                result.push_str(&self.lines[row]);
                result.push('\n');
            }
            // Last row: up to and including end.col
            let last: Vec<char> = self.lines[end.row].chars().collect();
            let to = (end.col + 1).min(last.len());
            result.extend(&last[..to]);
        }

        Some(result)
    }

    /// Remove and return the text covered by the current selection.
    /// Cursor is placed at the start of the deleted region.
    pub fn delete_selection(&mut self) -> Option<String> {
        let yanked = self.yank_selection()?;
        let sel = self.selection.take()?;
        let (start, end) = sel.normalized();

        if start.row == end.row {
            let chars: Vec<char> = self.lines[start.row].chars().collect();
            let from = start.col.min(chars.len());
            let to = (end.col + 1).min(chars.len());
            let new_line: String = chars[..from].iter().chain(&chars[to..]).collect();
            self.lines[start.row] = new_line;
        } else {
            let start_prefix: String = self.lines[start.row].chars().take(start.col).collect();
            let end_suffix: String = self.lines[end.row].chars().skip(end.col + 1).collect();
            // Remove rows from start.row+1 up through end.row
            let rows_to_remove = end.row - start.row;
            for _ in 0..rows_to_remove {
                if start.row + 1 < self.lines.len() {
                    self.lines.remove(start.row + 1);
                }
            }
            self.lines[start.row] = start_prefix + &end_suffix;
        }

        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor = start;
        self.clamp_cursor_col();
        self.mark_modified();
        Some(yanked)
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
            let prev_byte = line[..byte_idx].char_indices().last().map(|(i, _)| i).unwrap_or(0);
            line.remove(prev_byte);
            self.cursor.col -= 1;
        }

        self.mark_modified();
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

    /// Move cursor to the first non-whitespace character on the current line (`^`).
    /// If the line is all whitespace (or empty), falls back to column 0.
    pub fn move_cursor_first_nonblank(&mut self) {
        let col = self
            .lines
            .get(self.cursor.row)
            .and_then(|line| {
                line.char_indices().find(|(_, c)| !c.is_whitespace()).map(|(byte_idx, _)| {
                    // Convert byte index to char index
                    line[..byte_idx].chars().count()
                })
            })
            .unwrap_or(0);
        self.cursor.col = col;
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

    #[allow(dead_code)]
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
        self.visual_line_anchor = None;
    }

    // ── Visual Line mode selection helpers ────────────────────────────────────

    /// Enter Visual Line mode: record the anchor row and set the initial selection.
    pub fn start_selection_line(&mut self) {
        self.visual_line_anchor = Some(self.cursor.row);
        self.update_selection_line();
    }

    /// Recompute the linewise selection from `visual_line_anchor` to `cursor.row`.
    /// The selection always spans complete lines (col=0 … col=usize::MAX).
    pub fn update_selection_line(&mut self) {
        let anchor = self.visual_line_anchor.unwrap_or(self.cursor.row);
        let cur = self.cursor.row;
        let (min_row, max_row) = if anchor <= cur { (anchor, cur) } else { (cur, anchor) };
        self.selection = Some(Selection {
            start: Cursor { row: min_row, col: 0 },
            end: Cursor { row: max_row, col: usize::MAX },
        });
    }

    // ── Multi-line yank / delete (count-prefix and Visual Line operators) ─────

    /// Yank `count` lines starting at cursor row (for `Nyy`).
    /// Returns lines joined with `\n`; caller tags register as Linewise.
    pub fn yank_lines(&self, count: usize) -> String {
        let start = self.cursor.row;
        let end = (start + count).min(self.lines.len());
        self.lines[start..end].join("\n")
    }

    /// Delete `count` lines starting at cursor row (for `Ndd`).
    /// Returns deleted text joined with `\n`.
    pub fn delete_lines(&mut self, count: usize) -> String {
        let start = self.cursor.row;
        let end = (start + count).min(self.lines.len());
        let yanked = self.lines[start..end].join("\n");
        self.lines.drain(start..end);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor.row = start.min(self.lines.len() - 1);
        self.clamp_cursor_col();
        self.mark_modified();
        yanked
    }

    /// Yank the lines covered by the current Visual Line selection.
    pub fn yank_selection_lines(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.normalized();
        let end_row = end.row.min(self.lines.len().saturating_sub(1));
        Some(self.lines[start.row..=end_row].join("\n"))
    }

    /// Delete the lines covered by the current Visual Line selection.
    /// Returns the deleted text joined with `\n`.
    pub fn delete_selection_lines(&mut self) -> Option<String> {
        let yanked = self.yank_selection_lines()?;
        let sel = self.selection.take()?;
        let (start, end) = sel.normalized();
        let end_row = end.row.min(self.lines.len().saturating_sub(1));
        self.lines.drain(start.row..=end_row);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor.row = start.row.min(self.lines.len() - 1);
        self.cursor.col = 0;
        self.visual_line_anchor = None;
        self.mark_modified();
        Some(yanked)
    }

    /// Jump to an absolute line number (1-based, as typed by the user).
    /// Used by `5G` / `5gg` count-prefix navigation.
    pub fn goto_line(&mut self, one_based: usize) {
        self.cursor.row = one_based.saturating_sub(1).min(self.lines.len().saturating_sub(1));
        self.clamp_cursor_col();
    }

    // -------------------------------------------------------------------------
    // In-file search and replace
    // -------------------------------------------------------------------------

    /// Set the search pattern and find all matches in the buffer.
    /// Returns the number of matches found.
    pub fn set_search_pattern(&mut self, pattern: String) -> usize {
        if pattern.is_empty() {
            self.clear_search();
            return 0;
        }

        self.search_pattern = Some(pattern.clone());
        self.search_matches.clear();

        // Find all occurrences (case-insensitive search)
        let pattern_lower = pattern.to_lowercase();
        for (row_idx, line) in self.lines.iter().enumerate() {
            let line_lower = line.to_lowercase();
            let mut col = 0;
            while let Some(pos) = line_lower[col..].find(&pattern_lower) {
                let match_col = col + pos;
                self.search_matches.push((row_idx, match_col, pattern.len()));
                col = match_col + 1;
            }
        }

        // Jump to the first match after cursor, or first match overall
        if !self.search_matches.is_empty() {
            let current_pos = (self.cursor.row, self.cursor.col);
            let idx = self
                .search_matches
                .iter()
                .position(|(r, c, _)| (*r, *c) > current_pos)
                .unwrap_or(0);
            self.current_match_idx = Some(idx);
            self.jump_to_current_match();
        }

        self.search_matches.len()
    }

    /// Clear the search pattern and matches.
    pub fn clear_search(&mut self) {
        self.search_pattern = None;
        self.search_matches.clear();
        self.current_match_idx = None;
    }

    /// Jump to the next search match.
    pub fn search_next(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        if let Some(idx) = self.current_match_idx {
            self.current_match_idx = Some((idx + 1) % self.search_matches.len());
        } else {
            self.current_match_idx = Some(0);
        }
        self.jump_to_current_match();
    }

    /// Jump to the previous search match.
    pub fn search_prev(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        if let Some(idx) = self.current_match_idx {
            self.current_match_idx =
                Some(if idx == 0 { self.search_matches.len() - 1 } else { idx - 1 });
        } else {
            self.current_match_idx = Some(self.search_matches.len() - 1);
        }
        self.jump_to_current_match();
    }

    /// Move cursor to the current match.
    fn jump_to_current_match(&mut self) {
        if let Some(idx) = self.current_match_idx {
            if let Some(&(row, col, _)) = self.search_matches.get(idx) {
                self.cursor.row = row;
                self.cursor.col = col;
            }
        }
    }

    /// Replace the current match with the given text.
    /// Returns true if a replacement was made.
    pub fn replace_current(&mut self, replacement: &str) -> bool {
        if let Some(idx) = self.current_match_idx {
            if let Some(&(row, col, len)) = self.search_matches.get(idx) {
                let line = &mut self.lines[row];
                let chars: Vec<char> = line.chars().collect();

                if col + len <= chars.len() {
                    let before: String = chars[..col].iter().collect();
                    let after: String = chars[col + len..].iter().collect();
                    *line = format!("{}{}{}", before, replacement, after);

                    self.mark_modified();

                    // Update match list - need to recalculate
                    if let Some(pattern) = self.search_pattern.clone() {
                        self.set_search_pattern(pattern);
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Replace all occurrences of the search pattern with the given text.
    /// Returns the number of replacements made.
    pub fn replace_all(&mut self, replacement: &str) -> usize {
        let pattern = match &self.search_pattern {
            Some(p) => p.clone(),
            None => return 0,
        };

        let pattern_lower = pattern.to_lowercase();
        let mut count = 0;

        for line in &mut self.lines {
            let mut new_line = String::new();
            let mut remaining = line.as_str();

            loop {
                let remaining_lower = remaining.to_lowercase();
                match remaining_lower.find(&pattern_lower) {
                    Some(pos) => {
                        // Get the actual case-preserved match
                        let byte_pos = remaining
                            .char_indices()
                            .nth(remaining_lower[..pos].chars().count())
                            .map(|(i, _)| i)
                            .unwrap_or(0);

                        new_line.push_str(&remaining[..byte_pos]);
                        new_line.push_str(replacement);

                        // Skip past the match
                        let skip_bytes = remaining
                            .char_indices()
                            .nth(remaining_lower[..pos].chars().count() + pattern.chars().count())
                            .map(|(i, _)| i)
                            .unwrap_or(remaining.len());

                        remaining = &remaining[skip_bytes..];
                        count += 1;
                    },
                    None => {
                        new_line.push_str(remaining);
                        break;
                    },
                }
            }

            *line = new_line;
        }

        if count > 0 {
            self.mark_modified();
            // Refresh search matches
            self.set_search_pattern(pattern);
        }

        count
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

    #[allow(dead_code)]
    fn clamp_cursor_col_at(&mut self, col: usize) {
        let max = self.current_line_len();
        self.cursor.col = col.min(max);
    }
}

/// Convert a char-index `col` to a UTF-8 byte index within `s`.
/// If `col` exceeds the string's char count, returns `s.len()`.
pub fn char_to_byte_idx(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
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
