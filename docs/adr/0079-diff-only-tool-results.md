# ADR 0079: Diff-Only Tool Results for File Write/Edit Operations

**Date:** 2026-03-23
**Status:** Accepted

## Context

After the agent calls `write_file` or `edit_file`, it commonly calls `read_file` on the same path to verify its change was applied correctly. For a 500-line file this verification read costs ~2,000 tokens in the tool result alone — per round.

The old tool results were intentionally terse (`"wrote foo.rs (1234 bytes)"`, `"edited foo.rs (replaced 45 chars with 78 chars)"`), but they gave the model no structural information about what actually changed, forcing the verification read.

## Decision

`write_file` and `edit_file` now return a compact unified diff of the change instead of a byte/char count summary:

```
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -12,7 +12,7 @@
     let x = 1;
-    let y = x + 1;
+    let y = x + 2;
     y
```

A new `unified_diff(path, old, new, max_lines)` helper in `tools.rs` produces this output with 3 lines of context per hunk, capped at 120 output lines to prevent re-introducing token bloat for massive rewrites.

For `write_file` on a new file (no prior content), `old` is the empty string, producing a creation diff (`+line` for every line).

## Consequences

- The model can verify its edit succeeded without a follow-up `read_file` call, eliminating ~2,000 tokens per edit-verify cycle.
- For large rewrites (e.g. writing a 300-line file from scratch), the diff is capped at 120 lines — the model gets a representative view, not the full file.
- No new dependencies. The diff algorithm is a simple greedy line-by-line matcher; it is not LCS-optimal but is sufficient for verification purposes.
