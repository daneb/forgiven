use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

use super::{Editor, HoverPopupState, LocationEntry, LocationListState};
use crate::keymap::Mode;
use crate::lsp::LspManager;

// =============================================================================
// LSP location-list helpers (free functions)
// =============================================================================

/// Parse a `(uri, range)` JSON pair into `(PathBuf, line, col)`.
/// Handles both `Location` (`uri`/`range`) and `LocationLink` (`targetUri`/…) shapes.
pub(super) fn lsp_parse_location(
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
pub(super) fn lsp_flatten_symbol(
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

// =============================================================================
// Hover content extraction
// =============================================================================

/// Extract a plain-text string from an LSP Hover result value.
fn extract_hover_content(value: &serde_json::Value) -> String {
    let contents = match value.get("contents") {
        Some(c) => c,
        None => return String::new(),
    };
    // MarkupContent: { kind: "markdown"|"plaintext", value: "..." }
    if let Some(s) = contents.get("value").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    // MarkedString as a bare string
    if let Some(s) = contents.as_str() {
        return s.to_string();
    }
    // Array of MarkedString
    if let Some(arr) = contents.as_array() {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|item| item.as_str().or_else(|| item.get("value").and_then(|v| v.as_str())))
            .collect();
        return parts.join("\n\n");
    }
    String::new()
}

/// Return the word (identifier chars) under `col` in `line`.
fn word_at(line: &str, col: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let col = col.min(chars.len().saturating_sub(1));
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if chars.is_empty() || !is_word(chars[col]) {
        return String::new();
    }
    let start = (0..=col).rev().take_while(|&i| is_word(chars[i])).last().unwrap_or(col);
    let end = (col..chars.len()).take_while(|&i| is_word(chars[i])).last().unwrap_or(col);
    chars[start..=end].iter().collect()
}

// =============================================================================
// impl Editor — LSP methods
// =============================================================================

impl Editor {
    /// Request hover information at cursor position
    pub(super) fn request_hover(&mut self) {
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
        if let Some(client) = self.lsp.manager.get_client(&language) {
            match client.hover(uri, position) {
                Ok(rx) => {
                    self.lsp.pending_hover = Some(rx);
                    self.set_status("Loading hover…".to_string());
                },
                Err(e) => self.set_status(format!("LSP error: {e}")),
            }
        } else {
            self.set_status(format!("No LSP client for '{language}'"));
        }
    }

    /// Handle hover LSP response.
    pub(super) fn handle_hover_response(&mut self, value: serde_json::Value) {
        if value.is_null() {
            self.set_status("No hover info".to_string());
            return;
        }
        let content = extract_hover_content(&value);
        if content.is_empty() {
            self.set_status("No hover info".to_string());
            return;
        }
        self.lsp.hover_popup = Some(HoverPopupState { content, scroll: 0 });
        self.mode = Mode::LspHover;
    }

    /// Handle key events while Mode::LspHover is active.
    pub(super) fn handle_lsp_hover_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('K') => {
                self.mode = Mode::Normal;
                self.lsp.hover_popup = None;
            },
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(p) = &mut self.lsp.hover_popup {
                    p.scroll = p.scroll.saturating_add(1);
                }
            },
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(p) = &mut self.lsp.hover_popup {
                    p.scroll = p.scroll.saturating_sub(1);
                }
            },
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(p) = &mut self.lsp.hover_popup {
                    p.scroll = p.scroll.saturating_add(10);
                }
            },
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(p) = &mut self.lsp.hover_popup {
                    p.scroll = p.scroll.saturating_sub(10);
                }
            },
            _ => {},
        }
        Ok(())
    }

    /// Open the LSP rename input popup for the symbol under the cursor.
    pub(super) fn start_lsp_rename(&mut self) {
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
        if self.lsp.manager.get_client(&language).is_none() {
            self.set_status(format!("No LSP client for '{language}'"));
            return;
        }
        let word = self
            .current_buffer()
            .map(|buf| {
                let col = buf.cursor.col;
                buf.lines().get(buf.cursor.row).map(|l| word_at(l, col)).unwrap_or_default()
            })
            .unwrap_or_default();
        self.lsp.rename_buffer = word;
        self.lsp.rename_origin = Some((uri, position));
        self.mode = Mode::LspRename;
    }

    /// Handle key events while Mode::LspRename is active.
    pub(super) fn handle_lsp_rename_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.lsp.rename_buffer.clear();
                self.lsp.rename_origin = None;
            },
            KeyCode::Enter => {
                self.submit_lsp_rename();
            },
            KeyCode::Backspace => {
                self.lsp.rename_buffer.pop();
            },
            KeyCode::Char(c) => {
                self.lsp.rename_buffer.push(c);
            },
            _ => {},
        }
        Ok(())
    }

    /// Send the rename request with the text in `lsp_rename_buffer`.
    fn submit_lsp_rename(&mut self) {
        let new_name = self.lsp.rename_buffer.clone();
        if new_name.is_empty() {
            self.mode = Mode::Normal;
            self.lsp.rename_origin = None;
            return;
        }
        let (uri, position) = match self.lsp.rename_origin.take() {
            Some(pos) => pos,
            None => {
                self.mode = Mode::Normal;
                return;
            },
        };
        self.mode = Mode::Normal;
        let language = self
            .current_buffer()
            .and_then(|b| b.file_path.as_deref())
            .map(LspManager::language_from_path)
            .unwrap_or_default();
        if let Some(client) = self.lsp.manager.get_client(&language) {
            match client.rename(uri, position, new_name) {
                Ok(rx) => {
                    self.lsp.pending_rename = Some(rx);
                    self.set_status("Renaming…".to_string());
                },
                Err(e) => self.set_status(format!("LSP rename error: {e}")),
            }
        }
    }

    /// Apply a WorkspaceEdit returned by a rename request.
    pub(super) fn handle_rename_response(&mut self, value: serde_json::Value) {
        if value.is_null() {
            self.set_status("Rename returned no edits".to_string());
            return;
        }
        let mut total_edits = 0usize;

        if let Some(doc_changes) = value.get("documentChanges").and_then(|v| v.as_array()) {
            for change in doc_changes {
                let uri_str = match change
                    .get("textDocument")
                    .and_then(|td| td.get("uri"))
                    .and_then(|v| v.as_str())
                {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let edits = match change.get("edits").and_then(|v| v.as_array()) {
                    Some(e) => e,
                    None => continue,
                };
                total_edits += edits.len();
                if let Some(path) = lsp_uri_to_path(&uri_str) {
                    self.apply_text_edits(&path, edits);
                }
            }
        } else if let Some(changes) = value.get("changes").and_then(|v| v.as_object()) {
            for (uri_str, edits_val) in changes {
                let edits = match edits_val.as_array() {
                    Some(e) => e,
                    None => continue,
                };
                total_edits += edits.len();
                if let Some(path) = lsp_uri_to_path(uri_str) {
                    self.apply_text_edits(&path, edits);
                }
            }
        }

        if total_edits == 0 {
            self.set_status("Rename returned no edits".to_string());
        } else {
            self.set_status(format!("Renamed: {total_edits} edit(s) applied"));
        }
    }

    /// Apply a list of LSP TextEdits to a file, opening it if not already in a buffer.
    fn apply_text_edits(&mut self, path: &std::path::Path, edits: &[serde_json::Value]) {
        let buf_idx =
            self.buffers.iter().position(|b| b.file_path.as_deref() == Some(path)).or_else(|| {
                self.open_file(path).ok()?;
                Some(self.buffers.len().saturating_sub(1))
            });
        let buf_idx = match buf_idx {
            Some(i) => i,
            None => {
                tracing::warn!("apply_text_edits: could not open {}", path.display());
                return;
            },
        };

        // Sort edits bottom-to-top so earlier line numbers stay valid after each splice.
        let mut sorted: Vec<&serde_json::Value> = edits.iter().collect();
        sorted.sort_by(|a, b| {
            let line_of = |v: &&serde_json::Value| {
                v.get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
            };
            line_of(b).cmp(&line_of(a))
        });

        let mut lines: Vec<String> = self.buffers[buf_idx].lines().to_vec();
        for edit in &sorted {
            let range = match edit.get("range") {
                Some(r) => r,
                None => continue,
            };
            let sl = range
                .get("start")
                .and_then(|s| s.get("line"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let sc = range
                .get("start")
                .and_then(|s| s.get("character"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let el =
                range.get("end").and_then(|s| s.get("line")).and_then(|v| v.as_u64()).unwrap_or(0)
                    as usize;
            let ec = range
                .get("end")
                .and_then(|s| s.get("character"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let new_text = match edit.get("newText").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => continue,
            };

            // Collect the affected text into one string, splice, then split back.
            let start_line = sl.min(lines.len().saturating_sub(1));
            let end_line = el.min(lines.len().saturating_sub(1));

            let before = lines[start_line].chars().take(sc).collect::<String>();
            let after = lines[end_line].chars().skip(ec).collect::<String>();
            let spliced = format!("{before}{new_text}{after}");
            let new_lines: Vec<String> = spliced.split('\n').map(String::from).collect();

            lines.splice(start_line..=end_line, new_lines);
        }
        self.buffers[buf_idx].replace_all_lines(lines);
    }

    /// Request go-to-definition at cursor position
    pub(super) fn request_goto_definition(&mut self) {
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
        if let Some(client) = self.lsp.manager.get_client(&language) {
            match client.goto_definition(uri, position) {
                Ok(rx) => {
                    self.lsp.pending_goto_definition = Some(rx);
                    self.set_status("Finding definition…".to_string());
                },
                Err(e) => self.set_status(format!("LSP error: {e}")),
            }
        } else {
            self.set_status(format!("No LSP client for '{language}'"));
        }
    }

    /// Request find references at cursor position
    pub(super) fn request_references(&mut self) {
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
        if let Some(client) = self.lsp.manager.get_client(&language) {
            match client.references(uri, position) {
                Ok(rx) => {
                    self.lsp.pending_references = Some(rx);
                    self.set_status("Finding references…".to_string());
                },
                Err(e) => self.set_status(format!("LSP error: {e}")),
            }
        } else {
            self.set_status(format!("No LSP client for '{language}'"));
        }
    }

    /// Request document symbols for the current file
    pub(super) fn request_document_symbols(&mut self) {
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
        if let Some(client) = self.lsp.manager.get_client(&language) {
            match client.document_symbols(uri) {
                Ok(rx) => {
                    self.lsp.pending_symbols = Some(rx);
                    self.set_status("Loading symbols…".to_string());
                },
                Err(e) => self.set_status(format!("LSP error: {e}")),
            }
        } else {
            self.set_status(format!("No LSP client for '{language}'"));
        }
    }

    /// Navigate the editor to an absolute file path + 0-based line/col.
    pub(super) fn navigate_to_location(&mut self, path: std::path::PathBuf, line: u32, col: u32) {
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
    pub(super) fn handle_goto_definition_response(&mut self, value: serde_json::Value) {
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
                    self.lsp.location_list = Some(LocationListState {
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
    pub(super) fn handle_references_response(&mut self, value: serde_json::Value) {
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
        self.lsp.location_list = Some(LocationListState {
            title: format!("References ({count})"),
            entries,
            selected: 0,
        });
        self.mode = Mode::LocationList;
    }

    /// Handle a document-symbols LSP response value.
    pub(super) fn handle_symbols_response(&mut self, value: serde_json::Value) {
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
        self.lsp.location_list =
            Some(LocationListState { title: format!("Symbols ({count})"), entries, selected: 0 });
        self.mode = Mode::LocationList;
    }

    /// Handle key events while Mode::LocationList is active.
    pub(super) fn handle_location_list_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
                self.lsp.location_list = None;
            },
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(list) = &mut self.lsp.location_list {
                    if list.selected + 1 < list.entries.len() {
                        list.selected += 1;
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(list) = &mut self.lsp.location_list {
                    if list.selected > 0 {
                        list.selected -= 1;
                    }
                }
            },
            KeyCode::Enter => {
                if let Some(list) = &self.lsp.location_list {
                    if let Some(entry) = list.entries.get(list.selected) {
                        let path = entry.file_path.clone();
                        let line = entry.line;
                        let col = entry.col;
                        self.mode = Mode::Normal;
                        self.lsp.location_list = None;
                        self.navigate_to_location(path, line, col);
                    }
                }
            },
            _ => {},
        }
        Ok(())
    }

    /// Go to next diagnostic in current buffer
    pub(super) fn goto_next_diagnostic(&mut self) {
        if self.lsp.diagnostics.is_empty() {
            self.set_status("No diagnostics".to_string());
            return;
        }

        let current_line = self.current_buffer().map(|buf| buf.cursor.row).unwrap_or(0);

        // Find next diagnostic after current line and extract position
        let next_diag = self
            .lsp
            .diagnostics
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
            let first_diag = self.lsp.diagnostics.first().map(|d| {
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
    pub(super) fn goto_prev_diagnostic(&mut self) {
        if self.lsp.diagnostics.is_empty() {
            self.set_status("No diagnostics".to_string());
            return;
        }

        let current_line = self.current_buffer().map(|buf| buf.cursor.row).unwrap_or(0);

        // Find previous diagnostic before current line and extract position
        let prev_diag = self
            .lsp
            .diagnostics
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
            let last_diag = self.lsp.diagnostics.last().map(|d| {
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
    pub(super) fn get_current_lsp_position(&self) -> Option<(lsp_types::Uri, lsp_types::Position)> {
        let buf = self.current_buffer()?;
        let path = buf.file_path.as_ref()?;
        let uri = LspManager::path_to_uri(path).ok()?;
        let position =
            lsp_types::Position { line: buf.cursor.row as u32, character: buf.cursor.col as u32 };
        Some((uri, position))
    }

    /// Notify LSP about document changes and arm the completion debounce timer.
    pub(super) fn notify_lsp_change(&mut self) {
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

        if let Some(client) = self.lsp.manager.get_client(&language) {
            let _ = client.did_change(uri, version, text);
        }

        // Discard stale ghost text and reset debounce timer.
        self.ghost_text = None;
        self.pending_completion = None;
        self.last_edit_instant = Some(Instant::now());
        // Mark the sidecar debounce so the next flush_sidecar_events() call
        // sends a buffer_update after 300 ms of typing inactivity.
        self.last_sidecar_send = Some(std::time::Instant::now());
    }
}
