# ADR 0085 — LSP Navigation (Goto Definition, Find References, Symbols) and C# Revival

**Date:** 2026-03-23
**Status:** Accepted
**Supersedes:** [ADR 0052](0052-dotnet-lsp-csharp-ls.md) (C# LSP dropped)

---

## Context

### LSP navigation stubs

Since the initial LSP integration (ADR 0003) the client has been able to receive
and display diagnostics. However, the three most useful navigation requests —
`textDocument/definition`, `textDocument/references`, and
`textDocument/documentSymbol` — were wired to Action variants
(`LspGoToDefinition`, `LspReferences`, `LspDocumentSymbols`) but the handler
methods only set a `"not yet fully implemented"` status message. The LSP client
methods (`goto_definition`, `references`, `document_symbols`) were fully
implemented in `src/lsp/mod.rs` and correctly serialised LSP request payloads;
the gap was purely on the editor side: no polling, no response parsing, no UI.

### C# LSP history

ADR 0052 (2026-03-08) dropped C# LSP support after three blockers:

1. `csharp-ls` 0.22.0 failed to install on .NET 9 SDK (`DotnetToolSettings.xml`
   packaging bug in NuGet).
2. Workspace detection required `.csproj` at the workspace root — fragile for
   solution-style repos.
3. `Microsoft.CodeAnalysis.LanguageServer` (Roslyn) uses named pipes, not stdio.

The decision was to ship nothing rather than a broken experience, with a note to
revisit when named-pipe transport was available.

By 2026-03-23 `csharp-ls` can be built from source and invoked via
`dotnet /path/to/CSharpLanguageServer.dll`, bypassing the NuGet packaging bug.
A user needing navigation for a live C# codebase provided the impetus to
implement the navigation layer and restore C# support simultaneously.

### LSP protocol gaps

Two protocol-level bugs were also found during testing:

1. **Server-initiated requests were silently ignored.** `csharp-ls` (and other
   servers) send `window/workDoneProgress/create` before responding to most
   requests. The LSP spec requires the client to ack this with a `null` success
   response. Forgiven was logging a warning and sending nothing, causing the
   server to stall indefinitely waiting for the ack — this explained the
   "Finding definition…" status that never resolved.

2. **Error responses silently dropped the sender.** When a server returned an
   LSP error response (`resp.error` set), the `oneshot::Sender` was dropped
   without sending, leaving the receiver in `TryRecvError::Empty` state rather
   than `TryRecvError::Closed`. The receiver would hang until the next
   application exit rather than resolving immediately.

---

## Decision

### 1. LSP navigation — wiring the existing request methods

Implement the editor side of goto-definition, find-references, and
document-symbols as a unified **location list** pattern:

- Each request fires the corresponding `LspClient` method and stores a
  `oneshot::Receiver<serde_json::Value>` in a dedicated field
  (`pending_goto_definition`, `pending_references`, `pending_symbols`).
- The run loop polls all three receivers every tick (~50 ms) using a shared
  `poll_lsp_rx!` macro.
- On receipt the response JSON is parsed into `Vec<LocationEntry>` (each entry:
  label, file path, 0-based line/col).
- **Single result** (goto-definition): navigate directly, no overlay.
- **Multiple results** (multiple definitions, references, or symbols): open a
  `Mode::LocationList` popup overlay — j/k navigate, Enter jump, Esc close.

### 2. `Mode::LocationList` — shared navigation overlay

A new `LocationListState { title, entries, selected }` drives a centred popup
rendered by `render_location_list()`. This reuses the visual language of the
existing search panel (`render_search_panel`) — blue selection highlight,
`► ` prefix, scrollable viewport — without duplicating its query-input
machinery.

### 3. C# language mapping restored

`language_from_path` had no entry for `.cs`, returning `"plaintext"`. This meant
`did_open` was never sent for C# files and `get_client("csharp")` always returned
`None`. A single `"cs" => "csharp"` entry restores the mapping.

### 4. Workspace filter restored with broader C# detection

`filter_servers_for_workspace` previously checked only for a `.csproj` at the
workspace root, missing solution-style repos where `.sln` sits at the root and
`.csproj` files live in subdirectories. The filter is now:

1. `.sln` or `.csproj` at the workspace root, **or**
2. `.csproj` in any immediate subdirectory (e.g. `src/MyProject/`)

This covers both single-project repos and solution-style layouts without
starting `csharp-ls` on Rust, TypeScript, or other non-C# workspaces.

### 5. Protocol compliance fixes

**Server-initiated requests:** `process_messages()` and `wait_for_response()`
now send a `Response::new_ok(req.id, Value::Null)` for any request received from
the server, satisfying `window/workDoneProgress/create` and
`workspace/configuration` without implementing the full protocol surfaces.

**Error responses:** The `oneshot::Sender` is now sent `Value::Null` on error
rather than being silently dropped, so receivers always resolve on the next
poll frame.

### 6. `csharp-ls` via DLL path

No code change required; the existing `command`/`args` config split already
supports `command = "dotnet"` + `args = ["/path/to/CSharpLanguageServer.dll"]`.
This is the recommended installation path until the NuGet packaging bug is fixed.

---

## Implementation

### `src/keymap/mod.rs`

```rust
LocationList,    // LSP location list overlay (goto-definition / references / symbols)
```

### `src/lsp/mod.rs`

- `"cs" => "csharp"` added to `language_from_path`.
- `"csharp" => return true` in `filter_servers_for_workspace`.
- `process_messages` and `wait_for_response`: server requests now acked with
  `Response::new_ok(req.id, Value::Null)`; error responses now send
  `Value::Null` to the pending receiver instead of dropping the sender.

### `src/editor/mod.rs`

New public types:

```rust
pub struct LocationEntry {
    pub label: String,
    pub file_path: PathBuf,
    pub line: u32,
    pub col: u32,
}

pub struct LocationListState {
    pub title: String,
    pub entries: Vec<LocationEntry>,
    pub selected: usize,
}
```

New `Editor` fields:

```rust
pending_goto_definition: Option<oneshot::Receiver<serde_json::Value>>,
pending_references:      Option<oneshot::Receiver<serde_json::Value>>,
pending_symbols:         Option<oneshot::Receiver<serde_json::Value>>,
pub location_list:       Option<LocationListState>,
```

Run-loop polling (macro to avoid three identical blocks):

```rust
macro_rules! poll_lsp_rx {
    ($field:expr) => {{
        if let Some(rx) = $field.as_mut() {
            match rx.try_recv() {
                Ok(v)  => { $field = None; needs_render = true; Some(v) },
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(_) => { $field = None; Some(serde_json::Value::Null) },
            }
        } else { None }
    }};
}
```

Free functions:

- `lsp_parse_location(uri_val, range_val) -> Option<(PathBuf, u32, u32)>`
- `lsp_uri_to_path(uri: &str) -> Option<PathBuf>` — strips `file://`, decodes `%20`/`%23`
- `lsp_flatten_symbol(sym, path) -> Vec<LocationEntry>` — handles both
  `DocumentSymbol` (hierarchical, recurse into `children`) and
  `SymbolInformation` (flat, `location` field); depth-capped at 32 to prevent
  stack exhaustion on pathological trees
- `lsp_symbol_kind_name(kind: u64) -> &'static str` — maps LSP `SymbolKind`
  integers to short labels (`cls`, `meth`, `fn`, `prop`, …)

`navigate_to_location` checks `buffers` for an already-open path before calling
`open_file`, preventing duplicate buffer entries.

### `src/ui/mod.rs`

- `location_list: Option<&'a LocationListState>` added to `RenderContext`.
- `render_location_list()` — centred popup, O(viewport_height) allocation per
  frame, popup height clamped via `entries.len().min(u16::MAX as usize)` to
  prevent u16 overflow on very large symbol lists.
- `Mode::LocationList => "LSP"` in the status-bar mode label.

---

## Keybindings (unchanged — pre-existing `SPC l` subtree)

| Key | Action | Behaviour |
|-----|--------|-----------|
| `SPC l d` | `LspGoToDefinition` | Jump directly (1 result) or open list (multiple) |
| `SPC l f` | `LspReferences` | Open location list of all references |
| `SPC l s` | `LspDocumentSymbols` | Open symbol picker for current file |
| `SPC l h` | `LspHover` | Not yet implemented |
| `SPC l r` | `LspRename` | Not yet implemented |

**In the location list overlay:**

| Key | Action |
|-----|--------|
| `j` / `↓` | Move selection down |
| `k` / `↑` | Move selection up |
| `Enter` | Jump to selected location |
| `Esc` / `q` | Close overlay |

---

## Consequences

**Positive**

- Goto-definition, find-references, and document-symbols now work for any
  language whose LSP server is configured — Rust (rust-analyzer), TypeScript,
  Python, Go, and C# all benefit from the same implementation.
- The `window/workDoneProgress/create` ack fix unblocks any server that uses
  work-done progress (csharp-ls, rust-analyzer, pylsp). Previously all such
  servers would stall on navigation requests.
- C# codebases with solution-style layouts (`.sln` at root, `.csproj` in
  subdirectories) now connect correctly.
- `csharp-ls` can be used via a direct DLL path, bypassing the NuGet packaging
  issue that caused ADR 0052 to drop support.
- No new Cargo dependencies added.

**Negative / trade-offs**

- `"csharp" => return true` means `csharp-ls` is always started when configured,
  regardless of whether the workspace contains C# code. The server itself emits
  a log warning and exits cleanly if no project is found, so the cost is a
  brief startup attempt, not a hang.
- The null-ack for server-initiated requests satisfies `window/workDoneProgress/create`
  but does not implement `workspace/configuration`. Servers that require
  workspace configuration to function correctly may behave unexpectedly; a
  future ADR should implement configuration response routing.
- Hover (`SPC l h`) and rename (`SPC l r`) are still stubs. Hover requires a
  new floating tooltip overlay; rename requires a multi-buffer edit workflow.
  Both are deferred to a future ADR.
- `lsp_symbol_kind_name` returns abbreviated strings (`cls`, `meth`) rather
  than full names. These are intentionally compact for the narrow popup column;
  a future ADR could make them configurable.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Per-language response queues instead of single-field receivers | Adds complexity; only one in-flight request per type is needed |
| Modal overlay reusing `Mode::Search` | Search has query input machinery that symbols/references don't need |
| Implement `workspace/configuration` response routing fully | Scope creep; null ack satisfies csharp-ls in practice |
| CTags / ripgrep-based symbol navigation | No LSP awareness — cross-file type resolution, generics, and generated code all require a real language server |
| Roslyn LSP (named pipes) | Transport layer does not support named pipes; deferred per ADR 0052 |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| [0003](0003-lsp-integration-architecture.md) | Original LSP architecture — `LspClient`, `LspManager`, notification routing |
| [0052](0052-dotnet-lsp-csharp-ls.md) | Previous decision to drop C# LSP — superseded by this ADR |
| [0063](0063-structural-refactor-buffer-combinator-render-context-editor-substates.md) | `RenderContext` pattern used for `location_list` field |
| [0049](0049-diagnostics-overlay.md) | Diagnostics overlay — UI pattern reference for `Mode::LocationList` |
