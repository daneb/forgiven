use super::Editor;

impl Editor {
    /// Toggle the fold at the cursor position.
    ///
    /// Finds the innermost foldable region (function or class node) that
    /// contains the cursor row and toggles its collapsed state.  When the
    /// cursor is inside a fold body (not on the start row), it is moved to the
    /// fold start row so it remains visible.
    pub(crate) fn fold_toggle(&mut self) {
        let buf_idx = self.current_buffer_idx;
        // Ensure tree is parsed.
        let _ = self.ts_tree_for_current_buffer();

        let cursor_row = match self.current_buffer() {
            Some(b) => b.cursor.row,
            None => return,
        };

        let fold_ranges = self
            .ts_cache
            .get(&buf_idx)
            .map(crate::treesitter::query::fold_ranges)
            .unwrap_or_default();

        if fold_ranges.is_empty() {
            self.set_status(
                "No foldable region (tree-sitter not available for this file type)".to_string(),
            );
            return;
        }

        // Find the innermost range whose [start, end] spans the cursor row.
        let target = fold_ranges
            .iter()
            .filter(|&&(s, e)| cursor_row >= s && cursor_row <= e)
            .min_by_key(|&&(s, e)| e - s); // innermost = smallest span

        if let Some(&(start, _)) = target {
            let closed = self.fold_closed.entry(buf_idx).or_default();
            if closed.contains(&start) {
                closed.remove(&start);
            } else {
                closed.insert(start);
                // Move cursor to fold start if it was inside the fold body.
                if cursor_row != start {
                    if let Some(buf) = self.current_buffer_mut() {
                        buf.cursor.row = start;
                        let col = buf.cursor.col;
                        buf.move_to_col(col);
                    }
                }
            }
        } else {
            self.set_status("No foldable region at cursor".to_string());
        }
    }

    /// Close all folds in the current buffer.
    pub(crate) fn fold_close_all(&mut self) {
        let buf_idx = self.current_buffer_idx;
        let _ = self.ts_tree_for_current_buffer();

        let fold_ranges = self
            .ts_cache
            .get(&buf_idx)
            .map(crate::treesitter::query::fold_ranges)
            .unwrap_or_default();

        if fold_ranges.is_empty() {
            self.set_status("No foldable regions found".to_string());
            return;
        }

        let count = fold_ranges.len();
        let closed = self.fold_closed.entry(buf_idx).or_default();
        for (start, _) in fold_ranges {
            closed.insert(start);
        }
        // Move cursor to the start of its fold if it ended up hidden.
        let cursor_row = self.current_buffer().map(|b| b.cursor.row).unwrap_or(0);
        let buf_idx = self.current_buffer_idx;
        let hidden = {
            let closed = self.fold_closed.get(&buf_idx).cloned().unwrap_or_default();
            let ranges = self
                .ts_cache
                .get(&buf_idx)
                .map(crate::treesitter::query::fold_ranges)
                .unwrap_or_default();
            let mut h = std::collections::HashSet::new();
            for (s, e) in &ranges {
                if closed.contains(s) {
                    for r in (s + 1)..=*e {
                        h.insert(r);
                    }
                }
            }
            h
        };
        if hidden.contains(&cursor_row) {
            // Find the enclosing fold start and move cursor there.
            let closed = self.fold_closed.get(&buf_idx).cloned().unwrap_or_default();
            let ranges = self
                .ts_cache
                .get(&buf_idx)
                .map(crate::treesitter::query::fold_ranges)
                .unwrap_or_default();
            if let Some(&(start, _)) = ranges
                .iter()
                .filter(|&&(s, e)| closed.contains(&s) && cursor_row > s && cursor_row <= e)
                .min_by_key(|&&(s, e)| e - s)
            {
                if let Some(buf) = self.current_buffer_mut() {
                    buf.cursor.row = start;
                    let col = buf.cursor.col;
                    buf.move_to_col(col);
                }
            }
        }
        self.set_status(format!("{count} fold{} closed", if count == 1 { "" } else { "s" }));
    }

    /// Open all folds in the current buffer.
    pub(crate) fn fold_open_all(&mut self) {
        let buf_idx = self.current_buffer_idx;
        let count = self.fold_closed.get(&buf_idx).map(|s| s.len()).unwrap_or(0);
        self.fold_closed.remove(&buf_idx);
        if count > 0 {
            self.set_status(format!("{count} fold{} opened", if count == 1 { "" } else { "s" }));
        }
    }
}
