# ADR 0104 — Tree-sitter Core Integration

**Date:** 2026-04-04
**Status:** Accepted

---

## Context

Forgiven currently uses `syntect` (TextMate grammars) exclusively for syntax
highlighting. `syntect` is line-oriented: it processes one line at a time with a
state machine and has no concept of the surrounding AST. This is sufficient for
coloring tokens but insufficient for any operation that requires understanding
code structure:

- **Text objects** — `vif` (select inner function), `vac` (select outer class),
  `via` (select inner argument). These require knowing the boundaries of AST
  nodes relative to the cursor.
- **Code folding** — `za`/`zM`/`zR`. Fold regions must align with AST nodes
  (functions, blocks, imports), not arbitrary indentation levels.
- **Sticky scroll** — displaying the enclosing scope name at the top of the
  viewport requires querying the nearest ancestor scope node that spans the
  current viewport top row.

All three features are queued in the roadmap (items 1, 2, 3 — Complexity 1).
They share the same prerequisite: an in-process Tree-sitter parse tree for the
active buffer.

Tree-sitter is the industry standard for editor AST parsing (used by Neovim,
Helix, Zed, VS Code, GitHub). It produces a concrete syntax tree (CST) from any
supported language, supports incremental re-parsing when the source changes, and
ships a rich query language (S-expression patterns) for selecting nodes.

`syntect` is not being replaced — it remains the source of per-token colors via
TextMate theme scopes. Tree-sitter adds the structural AST layer alongside it.

---

## Decision

### 1. New module: `src/treesitter/`

A self-contained module that wraps the Tree-sitter `Parser` and exposes two
public types:

**`TsLang`** — an enum of supported languages:

```rust
pub enum TsLang {
    Rust, Python, JavaScript, TypeScript, Go, Json, Bash,
}
```

Language detection is done by file extension via `TsLang::detect(path: &Path)`.

**`TsSnapshot`** — the result of a successful parse:

```rust
pub struct TsSnapshot {
    pub tree: tree_sitter::Tree,
    pub source: String,   // the text that was parsed (owned)
    pub lang: TsLang,
}
```

`source` is kept alongside `tree` so callers can resolve byte-offset node spans
back to character positions without needing the original buffer.

**`TsEngine`** — owns the `Parser` and exposes `parse()`:

```rust
pub struct TsEngine {
    parser: tree_sitter::Parser,
}

impl TsEngine {
    pub fn new() -> Self
    pub fn detect(path: &Path) -> Option<TsLang>
    pub fn parse(&mut self, source: &str, lang: TsLang) -> Option<TsSnapshot>
}
```

A single `Parser` instance is reused across all parse calls (the parser is reset
to the new language before each call via `set_language()`). This is the
recommended pattern — `Parser` is not `Send` so it lives on the main thread.

### 2. Lazy tree cache in `Editor`

The `Editor` struct gains three fields:

```rust
ts_engine: treesitter::TsEngine,
ts_cache:  std::collections::HashMap<usize, treesitter::TsSnapshot>,
ts_versions: std::collections::HashMap<usize, i32>,
```

A private method `ts_tree_for_current_buffer()` returns
`Option<&treesitter::TsSnapshot>`:

1. Check if `ts_versions[current_buffer_idx] == buffer.lsp_version`. If yes,
   return the cached snapshot.
2. Otherwise, call `buffer.lines.join("\n")` to produce the source string,
   detect the language from `buffer.file_path`, call `ts_engine.parse()`, and
   store the result in `ts_cache` and `ts_versions`.
3. If the language is unknown or parsing fails, return `None` (graceful
   degradation — all callers must handle `None`).

The tree is parsed **lazily on first access** per buffer state, not on every
keypress. Since `lsp_version` is incremented on every buffer mutation, the
cache is automatically invalidated the next time a structural query is needed.
Parse time for a typical file is < 5 ms; this is acceptable for on-demand
queries (text object selection, fold computation) which are not frame-critical
operations.

### 3. Cargo dependencies

```toml
# Tree-sitter (AST parsing — text objects, folding, sticky scroll)
tree-sitter          = "0.22"
tree-sitter-rust     = "0.21"
tree-sitter-python   = "0.21"
tree-sitter-javascript = "0.21"
tree-sitter-typescript = "0.21"
tree-sitter-go       = "0.21"
tree-sitter-json     = "0.21"
tree-sitter-bash     = "0.21"
```

Each language crate compiles a C grammar via its own `build.rs`. The C
compilation is isolated to that crate's build artifact; no `build.rs` is needed
in Forgiven itself. `unsafe_code = "forbid"` in `[lints.rust]` applies only to
Forgiven's own Rust source and is unaffected.

TOML and Markdown are excluded from Phase 1: `tree-sitter-toml` has an
unstable ABI and `tree-sitter-md` requires a secondary injected grammar. Both
can be added in a follow-up once the language query infrastructure matures.

---

## Implementation

### `Cargo.toml`

Add to `[dependencies]`:

```toml
# Tree-sitter (AST parsing — text objects, folding, sticky scroll)
tree-sitter          = "0.22"
tree-sitter-rust     = "0.21"
tree-sitter-python   = "0.21"
tree-sitter-javascript = "0.21"
tree-sitter-typescript = "0.21"
tree-sitter-go       = "0.21"
tree-sitter-json     = "0.21"
tree-sitter-bash     = "0.21"
```

### New file: `src/treesitter/mod.rs`

See implementation in `src/treesitter/mod.rs`. Public surface:

- `TsLang` — language variant enum
- `TsSnapshot` — owned parse result (tree + source)
- `TsEngine` — parser wrapper with `new()`, `detect()`, `parse()`

### `src/main.rs`

Add `mod treesitter;` to the module declarations.

### `src/editor/mod.rs`

Add three fields to `Editor`:

```rust
// ── Tree-sitter AST cache ─────────────────────────────────────────────────
ts_engine: treesitter::TsEngine,
ts_cache: std::collections::HashMap<usize, treesitter::TsSnapshot>,
ts_versions: std::collections::HashMap<usize, i32>,
```

Initialise in `Editor::new()`:

```rust
ts_engine: treesitter::TsEngine::new(),
ts_cache: std::collections::HashMap::new(),
ts_versions: std::collections::HashMap::new(),
```

Add private method `ts_tree_for_current_buffer()` — see implementation.

---

## Consequences

**Positive**

- Unblocks text objects (ADR 0105), code folding (ADR 0106), and sticky scroll
  (ADR 0107) — all three are blocked on this foundation.
- Parse trees are accurate by construction; no regex edge cases.
- Incremental re-parsing can be added later by passing the previous `Tree` to
  `parser.parse(source, Some(&old_tree))` — the cache shape already supports
  this without API changes.
- `TsLang::detect()` is a single source of truth for file-extension → language
  mapping, avoiding duplication with `Highlighter::find_syntax_by_extension`.

**Negative / trade-offs**

- Build time increases: each language crate compiles a C grammar. Estimated
  +15–30 s for a cold `cargo build --release` (language grammars are large C
  files). Incremental rebuilds are unaffected.
- Binary size increases by ~2–4 MB (compiled grammar tables for 7 languages).
- Memory: each `TsSnapshot` stores a cloned source string alongside the tree.
  For a 1 000-line file (~40 KB) this adds ~40 KB per cached buffer. Acceptable.
- `Parser` is `!Send` — it must stay on the main thread. This is already the
  pattern for `Highlighter` and is not a new constraint.

**Future work (not in this ADR)**

- Incremental re-parse using edit notifications (tree-sitter `InputEdit`).
- `TsQuery` helper wrapping `tree_sitter::Query` for reusable S-expression
  patterns used by text objects and fold queries.
- Evict trees for buffers that have been closed (`ts_cache.remove(idx)` on
  buffer close).
- Additional languages: HTML, CSS, TOML, Markdown, TypeScript TSX.

---

## Alternatives considered

| Alternative | Rejected because |
|-------------|-----------------|
| Keep syntect only, implement text objects with regex | Regex cannot reliably parse nested structures; breaks on macros, multi-line expressions |
| Build syntect AST on top of highlight ranges | syntect scopes are too coarse for AST-level operations; no nesting info |
| Use LSP `textDocument/documentSymbol` for fold ranges | LSP round-trip latency (~50–200 ms) is unsuitable for interactive folding; also unavailable for files with no LSP server |
| Embed tree-sitter via WebAssembly | Unnecessary complexity; native tree-sitter Rust bindings compile cleanly |

---

## Related ADRs

| ADR | Relation |
|-----|----------|
| ADR 0105 (planned) | Tree-sitter text objects (`vif`, `vaf`, `via`, etc.) — first consumer of this foundation |
| ADR 0106 (planned) | AST-based code folding (`za`, `zM`, `zR`) |
| ADR 0107 (planned) | Sticky scroll context header |
| [0001](0001-tree-sitter-text-objects.md) | Original navigation design — Tree-sitter was anticipated |
