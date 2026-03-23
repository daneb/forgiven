# ADR 0082: Symbol-Aware Context Tools (get_file_outline, get_symbol_context)

**Date:** 2026-03-23
**Status:** Accepted

## Context

When the agent needs to understand a codebase it typically calls `read_file` on each relevant file, receiving the full line-numbered content. For a 2,000-line file this costs ~4,000 tokens in the tool result. The agent often needs only:

1. A structural overview (what functions/structs exist and where) — to decide which symbol to look at.
2. The body of one specific symbol — to understand or edit it.
3. The signatures of what that symbol calls — to understand its dependencies.

Returning the full file for all three use cases is the dominant source of token waste in code-editing sessions.

## Options considered

**A. tree-sitter** — accurate parse trees, but adds ~5 crate dependencies (tree-sitter + per-language grammars) and requires unsafe FFI bindings.

**B. LSP `textDocument/documentSymbol`** — accurate and already wired up, but synchronous execution in `execute_tool` cannot await the LSP oneshot receiver without a runtime handle, and the LSP may not be initialised for every file type.

**C. Heuristic line scanning** — no new dependencies, works on any language, fast. Less accurate than A/B for edge cases (macros, decorators) but covers the 90% case.

Option C was chosen as the pragmatic fit for this codebase.

## Decision

Two new tools are added to `tools.rs`:

**`get_file_outline(path)`**
- Scans for top-level definitions (fn/struct/enum/impl/trait/class/interface/def/func) using heuristic line patterns.
- Returns only the signature line for each definition with its line number.
- Token cost: ~100–400 tokens for a typical 500–2,000 line file vs. ~1,000–4,000 tokens for `read_file`.

**`get_symbol_context(path, symbol)`**
- Finds the named symbol via the same heuristics.
- Returns its full body (up to 150 lines) with line numbers.
- Appends signatures of any sibling symbols referenced within the body.
- Token cost: ~200–600 tokens for a typical function, replacing a 4,000-token `read_file`.

The heuristic covers: Rust, Python, TypeScript/JavaScript, Go, Java, C/C#.

### Recommended agent workflow (documented in tool description)

1. `get_file_outline` → find where the target symbol lives.
2. `get_symbol_context` → get its full definition.
3. `edit_file` → make the change (returns a diff, no re-read needed per ADR 0079).

## Consequences

- Typical 3-step edit (outline → context → edit) costs ~800 tokens vs. ~6,000 tokens for the old read → read → edit pattern (~7× reduction).
- Heuristic symbol detection misses symbols defined via macros, decorators, or multi-line signatures split across lines. For those cases `read_file` remains available.
- No new dependencies.
