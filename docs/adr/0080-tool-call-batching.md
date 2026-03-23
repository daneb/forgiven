# ADR 0080: Tool Call Batching — read_files and search_files

**Date:** 2026-03-23
**Status:** Accepted

## Context

Each tool call carries ~50–200 tokens of overhead (invocation JSON + result framing). When the agent needs several files (e.g., scanning a module to understand its structure), it previously issued N sequential `read_file` calls — N × overhead, N × round-trips.

Similarly, searching for a symbol across multiple directories required either a `read_file`-per-file scan (expensive) or a single `list_directory` + many `read_file` calls.

## Decision

Two new batched tools are added to `tools.rs`:

**`read_files(paths: [string])`**
- Reads multiple files in a single call.
- Returns each file's content under a `=== path (N lines) ===` header.
- Falls through gracefully per-path (errors don't abort the batch).

**`search_files(pattern: string, paths: [string])`**
- Literal text search across files and directories (recursive).
- Returns `file:line: text` lines, capped at 200 matches.
- Skips hidden files/directories.
- Replaces the common pattern of `read_file` × N followed by manual scanning.

The singular `read_file` and `list_directory` tools are retained for single-file and directory-listing use cases.

## Consequences

- Reduces tool call overhead from N×overhead to 1×overhead for multi-file reads.
- `search_files` gives the model an efficient way to locate symbols without reading whole files, complementing the existing `read_file` + `edit_file` workflow.
- No new dependencies; both tools use `std::fs` only.
- The 200-match cap on `search_files` prevents runaway results from broad patterns on large repos.
