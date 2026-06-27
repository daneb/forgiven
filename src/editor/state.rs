//! Pure data types and free functions that describe Editor mode state.
//!
//! Everything here is a struct, enum, or small helper that carries no `Editor`
//! methods and has no dependency on the terminal, buffers, or the event loop.
//! Kept separate so that `mod.rs` stays focused on `Editor` construction and
//! the public API surface.

use std::path::PathBuf;
use std::sync::Arc;

use ratatui::text::Span;
use tokio::sync::oneshot;

/// Whether the clipboard was populated by a line-wise or char-wise operation.
/// Controls how `p`/`P` pastes the content.
#[derive(Clone)]
pub(crate) enum ClipboardType {
    /// Produced by `yy`/`dd`/`cc` — paste inserts whole new line(s).
    Linewise,
    /// Produced by `yw`/`y$`/visual-y etc — paste inserts inline at cursor.
    Charwise,
}

/// Cached syntax-highlight spans for the visible viewport.
///
/// The key is `(buffer_idx, scroll_row, lsp_version)`. When any of these change the
/// cache is stale and syntect is re-run; otherwise the spans are reused without touching
/// the highlighter at all.  A full re-highlight of 40 visible lines takes ~3–8 ms; with
/// the cache that cost drops to ~0 for all frames where the user is just moving the
/// cursor or reading.
pub(crate) struct HighlightCache {
    pub buffer_idx: usize,
    pub scroll_row: usize,
    pub lsp_version: i32,
    pub spans: Arc<Vec<Vec<Span<'static>>>>,
}

/// Cached sticky-scroll context header.
///
/// Keyed on `(buffer_idx, scroll_row, lsp_version)` — the same staleness
/// signal used by `HighlightCache`.  Walking the tree-sitter CST on every
/// render frame is measurable (~0.5 ms/frame); this cache drops that to ~0
/// for the common case where the viewport does not move between frames.
pub(crate) struct StickyScrollCache {
    pub buffer_idx: usize,
    pub scroll_row: usize,
    pub lsp_version: i32,
    pub header: Option<String>,
}

/// Cached rendered markdown lines for Mode::MarkdownPreview.
/// Keyed on `(buffer_idx, lsp_version, viewport_width)` — regenerated only when
/// the active buffer changes, the content changes, or the terminal is resized.
pub(crate) struct MarkdownCache {
    pub buffer_idx: usize,
    pub lsp_version: i32,
    pub viewport_width: usize,
    pub lines: Vec<ratatui::text::Line<'static>>,
}

/// Cached fold hidden-row set and stub map (ADR 0138).
///
/// Keyed on `(buffer_idx, lsp_version, fold_fingerprint)`.  `fold_fingerprint`
/// is a cheap XOR hash of the sorted closed-fold start rows so that toggling a
/// fold invalidates the cache without a full set comparison.  `lsp_version`
/// covers buffer edits (same signal used by HighlightCache and StickyScrollCache).
///
/// On a cache hit the pre-built `HashSet`/`HashMap` are reused directly,
/// eliminating the per-frame allocation that existed before this cache.
pub(crate) struct FoldCache {
    pub buffer_idx: usize,
    pub lsp_version: i32,
    /// XOR of all closed-fold start rows. Cheap to compute; collisions are
    /// benign (worst case: one extra recomputation, never wrong output).
    pub fold_fingerprint: u64,
    pub hidden_rows: std::collections::HashSet<usize>,
    pub stub_map: std::collections::HashMap<usize, usize>,
}

// ── LSP state cluster (ADR 0144) ──────────────────────────────────────────────

/// All LSP-related state owned by the Editor.
///
/// Clusters the LSP manager, current diagnostics, in-flight RPC receivers, and
/// per-mode UI overlays (location list, hover popup, rename input) into one
/// sub-struct. Replaces eleven loose fields on `Editor` (ADR 0144).
#[derive(Default)]
pub(crate) struct LspState {
    /// Owns the per-language LSP client child processes.
    pub manager: crate::lsp::LspManager,
    /// Diagnostics for the current buffer (refreshed when LSP publishes).
    pub diagnostics: Vec<lsp_types::Diagnostic>,

    // ── In-flight LSP RPCs (polled in event_loop.rs each tick) ────────────────
    pub pending_goto_definition: Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_references: Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_symbols: Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_hover: Option<oneshot::Receiver<serde_json::Value>>,
    pub pending_rename: Option<oneshot::Receiver<serde_json::Value>>,

    // ── Per-mode overlay state ────────────────────────────────────────────────
    /// Mode::LocationList — populated by goto-definition / references / symbols.
    pub location_list: Option<LocationListState>,
    /// Mode::LspHover — popup body + scroll.
    pub hover_popup: Option<HoverPopupState>,
    /// Mode::LspRename — text typed into the rename prompt.
    pub rename_buffer: String,
    /// Mode::LspRename — origin URI + position to send to `textDocument/rename`.
    pub rename_origin: Option<(lsp_types::Uri, lsp_types::Position)>,
}

// ── LSP location list ─────────────────────────────────────────────────────────

/// A single navigable entry produced by goto-definition, find-references, or
/// document-symbols requests.
pub struct LocationEntry {
    /// Human-readable label shown in the list.
    pub label: String,
    /// Absolute path of the target file.
    pub file_path: PathBuf,
    /// 0-based target line.
    pub line: u32,
    /// 0-based target column.
    pub col: u32,
}

/// State for Mode::LocationList — a lightweight overlay listing LSP locations.
pub struct LocationListState {
    /// Title shown in the popup border.
    pub title: String,
    pub entries: Vec<LocationEntry>,
    pub selected: usize,
}

/// State for Mode::LspHover — a scrollable popup showing hover documentation.
pub struct HoverPopupState {
    /// Hover text (plain text or Markdown).
    pub content: String,
    /// Vertical scroll offset in lines.
    pub scroll: u16,
}

/// State for the vertical split pane (Mode::Normal with an active split).
#[derive(Default)]
pub(crate) struct SplitState {
    /// Index of the background pane's buffer; `None` = no split active.
    pub other_idx: Option<usize>,
    /// `true` when the right pane has focus.
    pub right_focused: bool,
    /// Per-viewport highlight cache for the inactive (background) pane.
    pub highlight_cache: Option<HighlightCache>,
}
