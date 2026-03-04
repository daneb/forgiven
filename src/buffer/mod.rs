// Buffer module — re-exports
#[allow(clippy::module_inception)]
pub mod buffer;
pub mod cursor;
pub mod history;

pub use buffer::{Buffer, Selection};
pub use cursor::Cursor;
