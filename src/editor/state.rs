//! Pure data types and free functions that describe Editor mode state.
//!
//! Everything here is a struct, enum, or small helper that carries no `Editor`
//! methods and has no dependency on the terminal, buffers, or the event loop.
//! Kept separate so that `mod.rs` stays focused on `Editor` construction and
//! the public API surface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

// ── Inline assistant (ADR 0111) ───────────────────────────────────────────────

/// Lifecycle phase of the inline assist overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineAssistPhase {
    /// User is typing their transformation directive.
    Input,
    /// LLM request is in-flight; tokens are accumulating.
    Generating,
    /// Response complete; waiting for user to accept or reject.
    Preview,
}

/// All state owned by `Editor` while `Mode::InlineAssist` is active.
/// Dropped when the user accepts or cancels.
pub struct InlineAssistState {
    /// Directive typed by the user during `Phase::Input`.
    pub prompt: String,
    /// Original selected text (empty when invoked without a selection).
    pub original_text: String,
    /// Buffer selection at the moment `InlineAssistStart` was fired.
    /// Used to locate and replace the text on accept.
    pub original_selection: Option<crate::buffer::Selection>,
    /// Buffer index the assist is targeting.
    pub target_buffer_idx: usize,
    /// File language hint derived from the buffer's extension (e.g. "Rust", "Python").
    /// Injected into the system prompt so the model knows what language to produce.
    pub language: Option<String>,
    /// LLM response accumulator.
    pub response: String,
    pub phase: InlineAssistPhase,
    /// Populated when the LLM request is launched (Input → Generating).
    pub stream_rx: Option<tokio::sync::mpsc::Receiver<crate::agent::StreamEvent>>,
    /// Kept alive to abort on cancel; dropped (fires abort) when `inline_assist` is set to None.
    #[allow(dead_code)]
    pub abort_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// State for Mode::LspHover — a scrollable popup showing hover documentation.
pub struct HoverPopupState {
    /// Hover text (plain text or Markdown).
    pub content: String,
    /// Vertical scroll offset in lines.
    pub scroll: u16,
}

// ── Multi-file review / change set view (ADR 0113) ───────────────────────────

/// A single line in a per-file unified diff.
pub enum DiffLine {
    /// Unchanged context line (shown dimmed).
    Context(String),
    /// Line added in the current version (shown green).
    Added(String),
    /// Line removed relative to the snapshot (shown red).
    Removed(String),
    /// Start of hunk `n` (0-indexed).  Replaces the old `HunkSep` and carries
    /// the hunk index so the renderer can look up per-hunk verdict.
    HunkStart(usize),
}

/// Whether the user has accepted or rejected a file's (or hunk's) changes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pending,
    Accepted,
    Rejected,
}

/// Diff data for one agent-modified file.
pub struct FileDiff {
    /// Project-relative path (key in `session_snapshots`).
    pub rel_path: String,
    /// Unified-diff lines with `HunkStart(idx)` separators.
    pub lines: Vec<DiffLine>,
    /// Per-hunk accept/reject decision.  Length equals the number of hunks.
    pub hunk_verdicts: Vec<Verdict>,
    /// File content before any agent edits (from `session_snapshots`, or `""` for new files).
    pub original: String,
    /// File content after all agent edits (read from disk when the overlay was opened).
    pub agent_version: String,
}

impl FileDiff {
    /// Derive the file-level verdict from the hunk verdicts.
    /// Accepted if all hunks accepted; Rejected if all rejected; Pending otherwise.
    pub fn file_verdict(&self) -> Verdict {
        if self.hunk_verdicts.is_empty() {
            return Verdict::Accepted;
        }
        let all_acc = self.hunk_verdicts.iter().all(|v| *v == Verdict::Accepted);
        let all_rej = self.hunk_verdicts.iter().all(|v| *v == Verdict::Rejected);
        if all_acc {
            Verdict::Accepted
        } else if all_rej {
            Verdict::Rejected
        } else {
            Verdict::Pending
        }
    }
}

/// All state for `Mode::ReviewChanges`.
pub struct ReviewChangesState {
    /// One entry per agent-touched file, sorted by path.
    pub diffs: Vec<FileDiff>,
    /// Vertical scroll offset into the flat rendered line list.
    pub scroll: usize,
    /// Index of the currently focused file (target for file-level `y` / `n`).
    pub focused_file: usize,
    /// Precomputed first flat-rendered-line index for each file's header.
    /// `file_offsets[i]` is the scroll offset that brings file `i` to the top.
    pub file_offsets: Vec<usize>,
    /// Focused hunk index within the focused file (`None` = no hunk focused).
    pub focused_hunk: Option<usize>,
    /// For each file, the flat line index of each `HunkStart` within that file's block.
    /// `hunk_line_offsets[file_idx][hunk_idx]` is the absolute flat line index.
    pub hunk_line_offsets: Vec<Vec<usize>>,
}

impl ReviewChangesState {
    /// Build from agent session state vs the current on-disk state.
    /// `created_paths` lists files newly created by the agent (original = "").
    pub fn build(
        snapshots: &HashMap<String, String>,
        created_paths: &[String],
        project_root: &Path,
    ) -> Self {
        let mut diffs: Vec<FileDiff> = snapshots
            .iter()
            .map(|(rel_path, original)| {
                let abs = project_root.join(rel_path);
                let agent_version = std::fs::read_to_string(&abs).unwrap_or_default();
                let (lines, hunk_count) = review_diff_lines(original, &agent_version);
                let hunk_verdicts = vec![Verdict::Pending; hunk_count];
                FileDiff {
                    rel_path: rel_path.clone(),
                    lines,
                    hunk_verdicts,
                    original: original.clone(),
                    agent_version,
                }
            })
            .collect();

        // Add newly created files (original = empty string)
        for rel_path in created_paths {
            let abs = project_root.join(rel_path);
            let agent_version = std::fs::read_to_string(&abs).unwrap_or_default();
            let (lines, hunk_count) = review_diff_lines("", &agent_version);
            let hunk_verdicts = vec![Verdict::Pending; hunk_count];
            diffs.push(FileDiff {
                rel_path: rel_path.clone(),
                lines,
                hunk_verdicts,
                original: String::new(),
                agent_version,
            });
        }

        diffs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let (file_offsets, hunk_line_offsets) = review_compute_offsets(&diffs);
        Self {
            diffs,
            scroll: 0,
            focused_file: 0,
            file_offsets,
            focused_hunk: None,
            hunk_line_offsets,
        }
    }
}

/// Compute flat-line offsets for files and hunks within each file.
/// Returns `(file_offsets, hunk_line_offsets)`.
pub(crate) fn review_compute_offsets(diffs: &[FileDiff]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let mut file_offsets = Vec::with_capacity(diffs.len());
    let mut hunk_line_offsets: Vec<Vec<usize>> = Vec::with_capacity(diffs.len());
    let mut acc = 0usize;
    for d in diffs {
        file_offsets.push(acc);
        acc += 1; // file header line
        let mut hunks_in_file = Vec::new();
        for dl in &d.lines {
            if let DiffLine::HunkStart(_) = dl {
                hunks_in_file.push(acc);
            }
            acc += 1;
        }
        acc += 1; // blank spacer
        hunk_line_offsets.push(hunks_in_file);
    }
    (file_offsets, hunk_line_offsets)
}

/// Produce unified-diff lines (with 3-line context groups) via the `similar` crate.
/// Returns `(lines, hunk_count)`.
pub(crate) fn review_diff_lines(original: &str, current: &str) -> (Vec<DiffLine>, usize) {
    use similar::{ChangeTag, TextDiff};
    if original == current {
        return (vec![], 0);
    }
    let diff = TextDiff::from_lines(original, current);
    let mut out = Vec::new();
    let mut hunk_idx = 0usize;
    for group in diff.grouped_ops(3) {
        out.push(DiffLine::HunkStart(hunk_idx));
        for op in &group {
            for change in diff.iter_changes(op) {
                let line = change.value().trim_end_matches('\n').to_string();
                match change.tag() {
                    ChangeTag::Delete => out.push(DiffLine::Removed(line)),
                    ChangeTag::Insert => out.push(DiffLine::Added(line)),
                    ChangeTag::Equal => out.push(DiffLine::Context(line)),
                }
            }
        }
        hunk_idx += 1;
    }
    (out, hunk_idx)
}

/// Reconstruct file content by selectively reverting rejected hunks.
///
/// For each hunk: if `Rejected`, use the original lines; otherwise use the
/// agent's version.  Lines outside any hunk group (far context) are taken from
/// the agent version unchanged.
pub(crate) fn apply_hunk_verdicts(
    original: &str,
    agent_version: &str,
    verdicts: &[Verdict],
) -> String {
    use similar::TextDiff;

    if verdicts.iter().all(|v| *v != Verdict::Rejected) {
        return agent_version.to_string();
    }
    if verdicts.iter().all(|v| *v == Verdict::Rejected) {
        return original.to_string();
    }

    let diff = TextDiff::from_lines(original, agent_version);
    let groups = diff.grouped_ops(3);

    let orig_lines: Vec<&str> = original.split_inclusive('\n').collect();
    let curr_lines: Vec<&str> = agent_version.split_inclusive('\n').collect();

    let mut out = String::new();
    let mut orig_consumed = 0usize;

    for (hunk_idx, group) in groups.iter().enumerate() {
        let group_old_start = group[0].old_range().start;
        // Emit "far context" lines between previous group and this one (identical in both versions)
        for i in orig_consumed..group_old_start {
            if let Some(l) = orig_lines.get(i) {
                out.push_str(l);
            }
        }

        let rejected = matches!(verdicts.get(hunk_idx), Some(Verdict::Rejected));
        let group_old_end = group.last().unwrap().old_range().end;

        if rejected {
            // Revert: emit original lines for this hunk's old range
            for i in group_old_start..group_old_end {
                if let Some(l) = orig_lines.get(i) {
                    out.push_str(l);
                }
            }
        } else {
            // Accept: emit current (agent) lines for this hunk's new range
            let group_new_start = group[0].new_range().start;
            let group_new_end = group.last().unwrap().new_range().end;
            for i in group_new_start..group_new_end {
                if let Some(l) = curr_lines.get(i) {
                    out.push_str(l);
                }
            }
        }

        orig_consumed = group_old_end;
    }

    // Emit remaining original lines after all groups
    for i in orig_consumed..orig_lines.len() {
        if let Some(l) = orig_lines.get(i) {
            out.push_str(l);
        }
    }

    out
}

// ── Mode-specific sub-states ──────────────────────────────────────────────────
// Each struct owns all fields that are active only during a single Mode variant.
// Grouping them prevents the top-level Editor struct from growing unboundedly as
// new modes are added.

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

/// State for the commit message generation popup (Mode::CommitMsg).
#[derive(Default)]
pub(crate) struct CommitMsgState {
    /// Editable commit message buffer.
    pub buffer: String,
    /// Byte offset of the edit cursor within `buffer`.
    pub cursor: usize,
    /// In-flight AI generation task.
    pub rx: Option<oneshot::Receiver<anyhow::Result<String>>>,
    /// `true` = generated from staged diff (`SPC g s`); `false` = last commit (`SPC g l`).
    pub from_staged: bool,
}

/// State for the release notes generation popup (Mode::ReleaseNotes).
#[derive(Default)]
pub(crate) struct ReleaseNotesState {
    /// Commit count input string (count-entry phase).
    pub count_input: String,
    /// In-flight AI generation task.
    pub rx: Option<oneshot::Receiver<anyhow::Result<String>>>,
    /// Completed release notes text (display phase).
    pub buffer: String,
    /// Scroll offset for the display popup.
    pub scroll: u16,
}
