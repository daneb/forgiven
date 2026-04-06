// Buffer module — re-exports
#[allow(clippy::module_inception)]
pub mod buffer;
pub mod cursor;
pub mod history;

pub use buffer::{visual_rows_for_len, Buffer, Selection};
pub use cursor::Cursor;
