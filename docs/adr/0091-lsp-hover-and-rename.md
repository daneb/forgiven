# ADR 0091 — LSP Hover and Rename

**Date:** 2026-03-27
**Status:** Accepted

---

## Context

Two LSP capabilities were wired up in the editor but had only stub
implementations:

1. **Hover** (`K` / `SPC l h`) — `Action::LspHover` already existed in the
   keymap and action dispatch, but the handler printed
   `"Hover requested (not yet fully implemented)"` and returned immediately. No
   popup was shown and no LSP request was issued.

2. **Rename** (`SPC l r`) — `Action::LspRename` was dispatched but its handler
   contained `set_status("Rename not yet implemented")` and a `// TODO` comment.
   The `WorkspaceEdit` response format (both `documentChanges` and `changes`
   variants) was unhandled.

Both features are first-class IDE operations that developers reach for
constantly (documentation lookup while reading code, bulk symbol rename across
files), so the stubs created noticeable friction.

---

## Decision

### 1 — LSP Hover (`editor/lsp.rs`, `editor/mod.rs`, `ui/search_lsp.rs`)

**State:**

```rust
pub struct HoverPopupState {
    pub content: String,
    pub scroll:  u16,
}
```

`content` is populated by `extract_hover_content()`, a free function that
handles all three LSP Hover response shapes:

- `MarkupContent` (`{ kind, value }`) — takes `value` directly.
- Bare `MarkedString` string — takes the string.
- Array of `MarkedString` — joins items with `\n\n`.

**Flow:**

1. `request_hover()` resolves the language from the current buffer's file path,
   fetches the LSP client, and calls `client.hover(uri, position)` → stores the
   `oneshot::Receiver` in `pending_hover`.
2. The main run loop polls `pending_hover` each tick (via the existing
   `poll_lsp_rx!` macro) and calls `handle_hover_response(value)`.
3. `handle_hover_response` sets `hover_popup = Some(HoverPopupState { … })` and
   transitions to `Mode::LspHover`.
4. In `Mode::LspHover`, `handle_lsp_hover_mode()` handles:
   - `Esc` / `q` / `K` → back to Normal, clears `hover_popup`.
   - `j` / `↓` → `scroll += 1`.
   - `k` / `↑` → `scroll -= 1` (saturating).
   - `Ctrl+d` / `Ctrl+u` → ±10 lines.
5. `render_hover_popup()` in `ui/search_lsp.rs` draws a centred 80×60%-height
   popup (LightYellow border, wrap-enabled `Paragraph` with `scroll` offset).
   Status bar shows `HOVER` in `LightYellow`.

**Helper:** `word_at(line, col)` extracts the identifier under the cursor
(alphanumeric + `_`) for use by the rename flow described below.

---

### 2 — LSP Rename (`editor/lsp.rs`, `editor/mod.rs`, `ui/search_lsp.rs`)

**Flow:**

1. `start_lsp_rename()` resolves the current symbol word with `word_at()`,
   stores it in `lsp_rename_buffer`, saves the cursor URI+position in
   `lsp_rename_origin`, and transitions to `Mode::LspRename`.
2. `render_lsp_rename_popup()` shows a compact 50×3 centred popup (LightGreen
   border) with the current buffer text and a block cursor (`█`). Status bar
   shows `RENAME` in `LightGreen`.
3. `handle_lsp_rename_mode()` accepts:
   - Printable chars → appended to `lsp_rename_buffer`.
   - `Backspace` → pops last char.
   - `Esc` → cancels, clears state, returns to Normal.
   - `Enter` → calls `submit_lsp_rename()`.
4. `submit_lsp_rename()` takes `lsp_rename_origin`, calls
   `client.rename(uri, position, new_name)` → stores receiver in
   `pending_rename`. Returns to Normal immediately so the editor stays
   responsive while the request is in-flight.
5. `handle_rename_response(value)` handles both `WorkspaceEdit` shapes:
   - `documentChanges` — array of `TextDocumentEdit`, each with its own URI and
     edits array.
   - `changes` — map of URI → edits array (older LSP servers).
6. `apply_text_edits(path, edits)` is a new private method that:
   - Opens the file into a buffer if not already loaded.
   - Sorts edits bottom-to-top so earlier line numbers stay valid after each
     splice.
   - For each edit: extracts `before` (chars before `start.character`) and
     `after` (chars from `end.character`), concatenates with `newText`, splits
     on `\n`, and splices into the lines vector.
   - Calls `replace_all_lines()` to commit the change.
   - Reports `"Renamed: N edit(s) applied"` in the status bar.

---

## Implementation notes

- `pending_hover` and `pending_rename` follow the exact same non-blocking
  `oneshot::Receiver` pattern already used for goto-definition, references, and
  symbols — no new concurrency primitives needed.
- `extract_hover_content` handles `null` responses gracefully (common when the
  cursor is on whitespace or an unsupported token); the status bar shows
  `"No hover info"` instead of an empty popup.
- `apply_text_edits` opens files that aren't in the buffer list so that
  cross-file renames work even when only one file is currently open.
- The bottom-to-top sort is a standard requirement for LSP text edit application:
  edits at higher line numbers are applied first so that earlier splice offsets
  are not invalidated.
- `lsp/mod.rs`: `Stdio::null()` added to the kill command to suppress spurious
  stderr noise on LSP server exit (originally noted in ADR 0090 implementation
  notes — placed here since it lives alongside the hover/rename LSP work).

---

## Consequences

**Positive**
- `K` now shows real hover documentation inline without leaving the editor,
  making API exploration practical in terminal-only workflows.
- `SPC l r` performs a project-wide symbol rename via the language server,
  applying `WorkspaceEdit` across all affected files atomically.
- Both features integrate cleanly with the existing LSP polling loop and the
  `Mode` dispatch table; no new async runtimes or threads required.

**Negative / trade-offs**
- `apply_text_edits` opens files that are not currently in the buffer list. For
  large projects with many rename sites this could load many files into memory.
  A future ADR could introduce a `virtual_edit` path that edits files on disk
  directly without loading them into the buffer list.
- Hover content is rendered as plain text (not Markdown). Servers that return
  rich Markdown hover docs (e.g. rust-analyzer) will render the raw Markdown
  syntax. A future improvement could pipe the content through the markdown
  renderer.

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0007](0007-lsp-integration.md) | Original LSP integration — `LspManager`, `LspClient` |
| [0063](0063-lsp-navigation.md) | Goto-definition, references, document symbols — the pattern hover/rename follow |
| [0089](0089-large-file-split-editor-agent-ui.md) | Module split that separated `lsp.rs`, `search_lsp.rs` |
