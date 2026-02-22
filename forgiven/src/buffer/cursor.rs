/// The position of the editing cursor within a buffer.
/// `row` and `col` are both 0-indexed char positions (not byte offsets).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

impl Cursor {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}
