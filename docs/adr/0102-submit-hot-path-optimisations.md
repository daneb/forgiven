# ADR 0102 — Submit Hot-Path Optimisations

**Date:** 2026-04-04
**Status:** Accepted

---

## Context

A performance audit of the agent-panel `submit()` path identified three recurring
costs that ran on every user message, regardless of whether their inputs had
changed:

1. **Project-tree filesystem walk** — `build_project_tree()` calls `std::fs::read_dir`
   recursively (depth 2) and sorts + allocates a String on every `submit()` call.
   For a project with hundreds of files this is tens of syscalls + allocations
   per keystroke that sends a message.

2. **Four tiktoken BPE encode passes** — The context-breakdown block (lines
   ~896–910) called `token_count::count()` four times per submit:
   - Once over the full message history (`history_t`, O(N × text))
   - Once over the file-context snippet (`ctx_file_t`)
   - Once over the full system prompt (`system_t`)
   - Once over the user message (`user_msg_t`)

   `encode_with_special_tokens` is an O(text_len × vocab) BPE operation; for
   conversations of 20+ messages this added 5–20 ms of CPU work per submit.
   Critically, these numbers are *only* used for the `SPC d` diagnostics overlay
   and the status-bar fuel gauge — they have no effect on correctness.

3. **Two allocations per tree entry** — `tree_recursive` allocated:
   - `"  ".repeat(depth)` — a new `String` per entry
   - `format!("{indent}{name}/\n")` or `format!("{indent}{name}\n")` — a second
     intermediate `String` per entry, immediately appended and discarded

   With ~200 entries at depth 2 this is ~400 short-lived allocations per call.
   After the caching fix (item 1) this only matters on cache misses, but it is
   still avoidable.

---

## Decision

### 1. Cache the project tree (TTL = 30 s)

Added `cached_project_tree: Option<(String, std::time::Instant)>` to `AgentPanel`.

In `submit()` the tree is now rebuilt only when the cache is absent or older than
30 seconds:

```rust
const TREE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
let project_tree = match self.cached_project_tree.as_ref() {
    Some((tree, ts)) if ts.elapsed() < TREE_TTL => tree.clone(),
    _ => {
        let tree = build_project_tree(&project_root, 2);
        self.cached_project_tree = Some((tree.clone(), std::time::Instant::now()));
        tree
    },
};
```

The cache is cleared by `new_conversation()` so a fresh tree is always used on a
new session (covers switching projects or the user explicitly resetting context).

**Trade-off:** The tree shown to the model may lag up to 30 s behind actual
filesystem changes mid-session. This is acceptable because: (a) the model is told
to call `read_file` / `list_directory` for details, and (b) the tree is a layout
hint, not an authoritative listing. Users who add a new file and immediately ask
the model about it will see it on the next submit after the TTL expires.

### 2. Use `len/4` consistently for context-breakdown numbers

Replaced the four `token_count::count()` calls in the breakdown block with the
same `len / 4` approximation already used by the history-truncation algorithm:

```rust
// Before (four BPE passes)
let history_t = send_messages[1..n-1].iter()
    .map(|v| super::token_count::count(v["content"].as_str().unwrap_or("")))
    .sum();
let ctx_file_t = context_snippet.as_ref().map(|c| super::token_count::count(c)).unwrap_or(0);
let system_t   = super::token_count::count(&system);
let user_msg_t = super::token_count::count(&user_text);

// After (zero BPE passes — reuses len/4 already computed above)
let history_t  = send_messages[1..n-1].iter()
    .map(|v| (v["content"].as_str().unwrap_or("").len() / 4) as u32)
    .sum();
let ctx_file_t = context_snippet.as_ref().map(|c| (c.len() / 4) as u32).unwrap_or(0);
let system_t   = system_tokens;   // already len/4 from budget calculation
let user_msg_t = (user_text.len() / 4) as u32;
```

**Accuracy:** The `len/4` approximation is ±15 % on typical English text, and
already drives all truncation decisions. Making the *display* numbers match the
*algorithm* numbers improves consistency: the user sees the same units the
truncation logic reasons about.  If accurate token counts are needed for
breakdown in future, per-message caching in `ChatMessage` would be the right
approach (one BPE call per message when pushed, zero cost on subsequent submits).

### 3. Eliminate per-entry allocations in `tree_recursive`

Moved `"  ".repeat(depth)` outside the per-entry loop (it is constant per
recursion level). Replaced `format!("{indent}{name}/\n")` with three direct
`push_str` / `push` calls on the output `String`:

```rust
// Before — two allocations per entry
let indent = "  ".repeat(depth);           // allocated inside loop
out.push_str(&format!("{indent}{name}/\n")); // second allocation

// After — zero allocations per entry (indent allocated once per level)
let indent = "  ".repeat(depth);           // outside loop
out.push_str(&indent);
out.push_str(&name);
out.push_str("/\n");
```

At depth 2 with ~200 entries this eliminates ~400 transient allocations per tree
build (negligible in isolation, but meaningful when multiplied over a session).

---

## Consequences

| Area | Before | After | Notes |
|---|---|---|---|
| Tree build cost | ~40–200 µs per submit (syscalls + allocs) | ~1–5 µs (clone of cached String) | On cache hit; cache miss cost unchanged |
| Tiktoken calls per submit | 4 (O(N × history)) | 0 | Breakdown display only |
| Allocations per tree entry | 2 | 0 | indent moved out of loop; push_str instead of format! |
| Breakdown accuracy | Exact (BPE) | ±15 % (len/4) | Consistent with truncation algorithm |

**Negative / trade-offs**

- The cached tree may be stale up to 30 s. Files created mid-session appear in
  the tree only after the TTL expires or `new_conversation()` is called.
- Breakdown token numbers in `SPC d` now show `len/4` estimates, not exact BPE
  counts. They already matched the truncation budget algorithm numerically, so
  this is a labelling change more than a semantic one.

---

## Complexity rating

| Change | Implementation complexity | Value | Impact frequency |
|---|---|---|---|
| Project tree TTL cache | Low | High | Every submit |
| Eliminate tiktoken from breakdown | Low | Medium-High | Every submit |
| Indent/format allocation reduction | Low | Low-Medium | Every cache-miss tree build |

---

## Related ADRs

- **ADR 0087** — Context Bloat Audit (introduced the breakdown and budget system)
- **ADR 0099** — Context Breakdown Token Awareness (SPC d overlay)
- **ADR 0093** — Cap Open-File Context Injection
