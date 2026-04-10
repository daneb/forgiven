use super::{ClipboardType, Editor};
use crate::keymap::{Mode, TextObjectKind};

impl Editor {
    /// Resolve the tree-sitter range for a text object at the cursor.
    ///
    /// Returns `Some((sr, sc, er, ec))` in inclusive char-index coordinates,
    /// or `None` (and sets a status message) when no node is found.
    pub(super) fn text_object_range(
        &mut self,
        inner: bool,
        kind: TextObjectKind,
    ) -> Option<(usize, usize, usize, usize)> {
        let cursor = self.current_buffer().map(|b| (b.cursor.row, b.cursor.col))?;
        let range = self.ts_tree_for_current_buffer().and_then(|snap| {
            crate::treesitter::query::text_object_range(snap, cursor.0, cursor.1, inner, kind)
        });
        if range.is_none() {
            self.set_status("No tree-sitter node at cursor".to_string());
        }
        range
    }

    /// Set the buffer selection to cover `(sr, sc) … (er, ec)`.
    pub(super) fn set_selection_range(&mut self, sr: usize, sc: usize, er: usize, ec: usize) {
        if let Some(buf) = self.current_buffer_mut() {
            buf.cursor.row = sr;
            buf.cursor.col = sc;
            buf.start_selection();
            buf.cursor.row = er;
            buf.cursor.col = ec;
            buf.update_selection();
        }
    }

    /// `SelectTextObject` — enter Visual mode and select the text object.
    pub(super) fn apply_text_object_select(&mut self, inner: bool, kind: TextObjectKind) {
        if let Some((sr, sc, er, ec)) = self.text_object_range(inner, kind) {
            self.mode = Mode::Visual;
            self.set_selection_range(sr, sc, er, ec);
        }
    }

    /// `DeleteTextObject` — delete the text object into the clipboard.
    pub(super) fn apply_text_object_delete(&mut self, inner: bool, kind: TextObjectKind) {
        if let Some((sr, sc, er, ec)) = self.text_object_range(inner, kind) {
            self.with_buffer(|buf| buf.save_undo_snapshot());
            self.set_selection_range(sr, sc, er, ec);
            if let Some(text) = self.current_buffer_mut().and_then(|buf| buf.delete_selection()) {
                self.sync_system_clipboard(&text);
                self.clipboard = Some((text, ClipboardType::Charwise));
                self.notify_lsp_change();
            }
            self.mode = Mode::Normal;
        }
    }

    /// `YankTextObject` — yank the text object into the clipboard.
    pub(super) fn apply_text_object_yank(&mut self, inner: bool, kind: TextObjectKind) {
        if let Some((sr, sc, er, ec)) = self.text_object_range(inner, kind) {
            self.set_selection_range(sr, sc, er, ec);
            if let Some(text) = self.current_buffer().and_then(|buf| buf.yank_selection()) {
                self.sync_system_clipboard(&text);
                let lines = text.lines().count().max(1);
                self.clipboard = Some((text, ClipboardType::Charwise));
                self.set_status(format!(
                    "{lines} line{} yanked",
                    if lines == 1 { "" } else { "s" }
                ));
            }
            self.with_buffer(|buf| buf.clear_selection());
            self.mode = Mode::Normal;
        }
    }

    /// `ChangeTextObject` — delete the text object and enter Insert mode.
    pub(super) fn apply_text_object_change(&mut self, inner: bool, kind: TextObjectKind) {
        if let Some((sr, sc, er, ec)) = self.text_object_range(inner, kind) {
            self.with_buffer(|buf| buf.save_undo_snapshot());
            self.set_selection_range(sr, sc, er, ec);
            if let Some(text) = self.current_buffer_mut().and_then(|buf| buf.delete_selection()) {
                self.sync_system_clipboard(&text);
                self.clipboard = Some((text, ClipboardType::Charwise));
                self.notify_lsp_change();
            }
            self.mode = Mode::Insert;
        }
    }
}
