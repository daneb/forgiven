# ADR 0012 — Agent UX: Context Injection, File Refresh, and Chat Rendering

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

After the agentic tool-calling loop (ADR 0011) was implemented, several UX problems
were observed in practice:

1. **Directory crawl**: the model spent its first 7–8 rounds calling `list_directory`
   on each subdirectory one at a time because it had no upfront map of the project.
   With `MAX_ROUNDS = 10` this consumed the entire budget before any file was read.

2. **Unknown open file path**: `context` passed the raw text of the open buffer with
   no filename, so the model could not reference it in tool calls.

3. **Stale buffer after agent edit**: when `write_file` or `edit_file` succeeded on
   disk the open buffer in the editor still showed the old content. There was no
   mechanism to trigger a reload.

4. **Mixed visual style**: tool-call status lines (`⚙ …`) were rendered identically
   to the model's prose responses, making it hard to distinguish "work being done"
   from "the final answer". The model also emitted intermediate reasoning text between
   tool rounds ("The first edit failed, let me try…").

---

## Decision

### 1. Project file tree in system prompt

`submit()` now calls `build_project_tree(root, depth=2)` before constructing the
system message and includes the result verbatim:

```
Project file tree (depth 2 — use read_file to see contents):
```
docs/
  adr/
src/
  agent/
    mod.rs
    tools.rs
  buffer/
  ...
Cargo.toml
```
```

`build_project_tree()` walks the directory recursively to depth 2, sorting
directories before files and skipping hidden entries and build artefact directories
(`target`, `node_modules`, `dist`, `build`). The output is generated once per
`submit()` call, not cached.

This means the model never needs to call `list_directory` for normal tasks — it can
read the file it needs in round 1.

### 2. Open file path included in context

Previously:
```rust
let context = self.current_buffer().map(|buf| buf.lines().join("\n"));
```

Now:
```rust
let context = self.current_buffer().map(|buf| {
    let path_header = buf.file_path
        .as_deref()
        .and_then(|p| p.to_str())
        .unwrap_or(&buf.name);
    format!("File: {path_header}\n\n{}", buf.lines().join("\n"))
});
```

The system message labels this section:
`"Currently open file (already read — you may use this content directly for edits)"`
so the model understands it can reference the file by path in `edit_file` without
an additional `read_file` round.

### 3. Automatic buffer reload on agent file modification

**Event flow:**

```
agentic_loop
  write_file / edit_file succeeds
  → tx.send(StreamEvent::FileModified { path: "src/foo.rs" })

AgentPanel::poll_stream()
  FileModified { path } → pushed to pending_reloads: Vec<String>

Editor::run() loop (every ~50 ms)
  reloads = mem::take(&mut agent_panel.pending_reloads)
  for rel_path in reloads:
    canonical = current_dir().join(rel_path).canonicalize()
    for buf in &mut buffers:
      if matches(buf.file_path, canonical):
        buf.reload_from_disk()
    set_status("↺ reloaded src/foo.rs")
```

**`Buffer::reload_from_disk()`** (new method on `Buffer`):
- Re-reads lines from the associated `file_path` using the same normalisation as
  `Buffer::from_file` (strip trailing `\n`, handle `\r\n`)
- Clears `is_modified`
- Clamps `cursor.row` and `cursor.col` to the new line count / line length

**Path matching** uses a three-layer strategy to handle the different ways a buffer's
`file_path` may have been set:

```rust
// Layer 1: canonicalize both paths (resolves symlinks, cleans ..)
let fp_canon = fp.canonicalize().unwrap_or_else(|_| fp.clone());
if fp_canon == canonical { return true; }

// Layer 2: component-wise suffix match
// Handles buffers opened from CLI with a relative path
fp.ends_with(Path::new(&rel_path))
```

`PathBuf::ends_with` is path-component-wise, so `"test.rs"` matches the last
component of `/project/test.rs` without false positives for files with the same name
in different directories (the full `rel_path` is used as the suffix).

### 4. System prompt: no intermediate text between tool rounds

A new **COMMUNICATION RULE** was added to the mandatory protocol:

> Do NOT output any text while working through tool calls. Work silently.
> Only write a single, concise final response AFTER all tools have completed.
> Do not narrate steps, explain retries, or announce what you are about to do.

This prevents the model from emitting progress commentary ("The first edit failed,
let me try again…") that would appear between tool-call lines in the chat panel.

### 5. Visual separation of tool lines from prose (`src/ui/mod.rs`)

The `render_content()` helper, extracted from the previous inline rendering, applies
different styles depending on whether each line starts with `⚙`:

| Line type | Colour | Word-wrap |
|-----------|--------|-----------|
| `⚙ …` (tool event) | `Color::DarkGray` | No — already compact |
| Regular prose | `Color::White` | Yes — split at `content_width` |
| Blank | — | Emitted only in prose sections |

A thin separator `────────────────────` in `Color::DarkGray` is inserted at the
first prose line that follows a run of tool lines, visually marking the boundary
between "work" and "answer":

```
  ⚙  read_file(src/config/mod.rs) → pub enum ShellType {         ← dim
  ⚙  edit_file(src/config/mod.rs) → edited (replaced 49 chars)   ← dim
  ────────────────────                                             ← dim separator
  ShellType now includes Fish. The Display impl and              ← white
  generate_fish method have been added.
```

The same `render_content()` closure is used for both historical messages and the
live streaming reply, so the visual style is consistent at all times.

### 6. MAX_ROUNDS raised from 10 → 20

10 rounds was insufficient for tasks requiring: list (optional now) + read + multiple
edits + verification. 20 rounds provides comfortable headroom while still preventing
runaway loops.

---

## Consequences

- **project_tree cost**: `build_project_tree()` does synchronous filesystem I/O
  on every `submit()` call. For a project with thousands of files this could add
  tens of milliseconds of latency and a large system prompt. Depth is capped at 2
  to bound both time and token cost.
- **reload races**: if the user edits the same file manually while the agent is
  writing to it, `reload_from_disk()` will overwrite the in-memory buffer with the
  agent's version. No merge strategy exists — last write wins.
- **Silent model**: the `COMMUNICATION RULES` instruction works with gpt-4o but
  LLMs may still emit inter-round text. The renderer handles this gracefully (it
  becomes white prose between dim ⚙ lines, separated by `────`).
- **No reload for new files**: if the agent creates a file that was not already open
  in a buffer, the `FileModified` event finds no matching buffer and is silently
  ignored. The user must open the new file manually via the explorer.
