/// Maximum number of undo/redo snapshots kept per buffer.
/// At ~10 KB average per snapshot this caps memory use at ~1 MB per buffer.
const MAX_SNAPSHOTS: usize = 100;

/// A full copy of the buffer content + cursor position captured before a mutation.
#[derive(Debug, Clone)]
pub struct BufferSnapshot {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

/// Snapshot-based undo / redo history for a single buffer.
///
/// ### How it works
///
/// Before every mutating action the caller pushes a snapshot of the *current*
/// state.  `undo()` pops from `past`, saves the current state to `future`, and
/// returns the snapshot to restore.  `redo()` is the mirror image.
///
/// ### Insert-mode coalescing
///
/// The snapshot is saved **once** when the editor enters Insert mode (not on
/// every keystroke), so the entire Insert session collapses into a single undo
/// step — matching standard vim behaviour.
#[derive(Debug, Clone, Default)]
pub struct EditHistory {
    /// Saved states, oldest first.  Top of stack = most recent.
    past: Vec<BufferSnapshot>,
    /// States saved during undo so they can be redone.
    future: Vec<BufferSnapshot>,
}

impl EditHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Save `lines` + `cursor` as the state *before* an upcoming mutation.
    /// Clears the redo stack — a new edit invalidates any future chain.
    pub fn save(&mut self, lines: &[String], cursor_row: usize, cursor_col: usize) {
        if self.past.len() >= MAX_SNAPSHOTS {
            self.past.remove(0);
        }
        self.past.push(BufferSnapshot {
            lines: lines.to_vec(),
            cursor_row,
            cursor_col,
        });
        self.future.clear();
    }

    /// Returns `true` if there is anything to undo.
    #[allow(dead_code)]
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// Returns `true` if there is anything to redo.
    #[allow(dead_code)]
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Pop the most recent past snapshot and save the current state for redo.
    /// Returns the snapshot the buffer should be restored to, or `None` if
    /// there is nothing to undo.
    pub fn undo(
        &mut self,
        current_lines: &[String],
        cursor_row: usize,
        cursor_col: usize,
    ) -> Option<BufferSnapshot> {
        let snap = self.past.pop()?;
        if self.future.len() >= MAX_SNAPSHOTS {
            self.future.remove(0);
        }
        self.future.push(BufferSnapshot {
            lines: current_lines.to_vec(),
            cursor_row,
            cursor_col,
        });
        Some(snap)
    }

    /// Pop the most recent future snapshot and save the current state for undo.
    /// Returns the snapshot the buffer should be restored to, or `None` if
    /// there is nothing to redo.
    pub fn redo(
        &mut self,
        current_lines: &[String],
        cursor_row: usize,
        cursor_col: usize,
    ) -> Option<BufferSnapshot> {
        let snap = self.future.pop()?;
        if self.past.len() >= MAX_SNAPSHOTS {
            self.past.remove(0);
        }
        self.past.push(BufferSnapshot {
            lines: current_lines.to_vec(),
            cursor_row,
            cursor_col,
        });
        Some(snap)
    }
}
