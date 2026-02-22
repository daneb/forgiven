/// A single recorded editing operation, used for undo/redo.
#[derive(Debug, Clone)]
pub enum EditOp {
    InsertChar { row: usize, col: usize, ch: char },
    InsertNewline { row: usize, col: usize },
    DeleteCharBefore { row: usize, col: usize },
    DeleteCharAt { row: usize, col: usize },
}

/// Tracks the edit history of a buffer for undo/redo support.
/// Phase 1: record-only. Undo/redo will be wired up in a follow-on pass.
#[derive(Debug, Clone, Default)]
pub struct EditHistory {
    ops: Vec<EditOp>,
    /// Points to the next slot after the most recent committed op.
    /// Ops at index >= cursor have been undone.
    cursor: usize,
}

impl EditHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new operation, discarding any undone future ops.
    pub fn record(&mut self, op: EditOp) {
        self.ops.truncate(self.cursor);
        self.ops.push(op);
        self.cursor += 1;
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.ops.len()
    }

    /// Pop the most recent op for undo (caller is responsible for applying inverse).
    pub fn undo(&mut self) -> Option<&EditOp> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.ops.get(self.cursor)
    }

    /// Advance cursor for redo (caller is responsible for re-applying the op).
    pub fn redo(&mut self) -> Option<&EditOp> {
        let op = self.ops.get(self.cursor)?;
        self.cursor += 1;
        Some(op)
    }
}
