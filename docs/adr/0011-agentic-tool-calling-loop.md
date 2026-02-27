# ADR 0011 — Agentic Tool-Calling Loop

**Date:** 2026-02-23
**Status:** Accepted
**Supersedes planned:** 0014 (agent tool-calling loop)

---

## Context

ADR 0006 established a single-turn chat panel: the user submits a message, Copilot
responds with text, and the user can optionally apply a fenced code block to the
open buffer. This is useful for Q&A and generating code snippets, but it cannot
autonomously read, write, or modify files in the project — every change requires
manual copy-paste.

The next capability required is an **agentic loop** in which the model can use tools
to directly read and edit the project's source files, retrying on errors, and only
producing a final text response once all work is done.

GitHub Copilot's API is OpenAI-compatible, so it supports the standard `tools` /
`tool_calls` response format.

---

## Decision

### Tool definitions (`src/agent/tools.rs` — new file)

Four tools are exposed to the model via `tool_definitions() -> serde_json::Value`:

| Tool | Purpose |
|------|---------|
| `read_file` | Read any file; returns line-numbered output |
| `write_file` | Write a complete file (new files or full rewrites) |
| `edit_file` | Surgical find-and-replace — `old_str` must appear exactly once |
| `list_directory` | List a directory's immediate children |

All file operations are sandboxed to the project root via `safe_path()`, which
rejects any path containing `..` components:

```rust
fn safe_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let candidate = root.join(relative);
    if candidate.components().any(|c| c.as_os_str() == "..") {
        return Err(format!("path traversal rejected: {relative}"));
    }
    Ok(candidate)
}
```

`execute_tool(call: &ToolCall, root: &Path) -> String` dispatches on `call.name`
and returns a human-readable result string that is sent back to the model as a
`role: "tool"` message.

### Streaming tool-call assembly

The Copilot SSE stream delivers tool call arguments as partial JSON chunks, keyed by
index, across multiple delta events:

```
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"read_file","arguments":""}}]}}]}
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"src/main.rs\"}"}}]}}]}
```

A `HashMap<usize, PartialToolCall>` accumulates chunks per index during SSE parsing.
After the stream ends (`[DONE]`), entries are sorted by index and converted to
`Vec<ToolCall>` for execution.

### Agentic loop (`agentic_loop` — free async fn)

```
submit()
  └─ tokio::spawn(agentic_loop(token, messages, project_root, tx))

agentic_loop (MAX_ROUNDS = 20):
  loop:
    call start_chat_stream_with_tools()
    parse SSE → text tokens + partial tool_calls
    if no tool_calls:
      send Done → exit
    else:
      for each tool_call:
        send ToolStart
        execute_tool() → result string
        send FileModified (if write_file/edit_file succeeded)
        send ToolDone
        append role:"tool" message to messages
      append role:"assistant" + tool_calls to messages
      continue loop
```

`MAX_ROUNDS = 20` caps runaway loops. If the limit is reached without a final text
response an `Error` event is sent.

### StreamEvent extensions

New variants added to the existing `StreamEvent` enum:

```rust
pub enum StreamEvent {
    Token(String),
    ToolStart { name: String, args_summary: String },
    ToolDone  { result_summary: String },
    FileModified { path: String },   // triggers buffer reload
    Done,
    Error(String),
}
```

`args_summary()` on `ToolCall` extracts the `path` argument for display so the user
can see which file is being touched without seeing the full JSON.

### API request shape

```json
{
  "model": "gpt-4o",
  "messages": [...],
  "tools": [...],
  "tool_choice": "auto",
  "stream": true,
  "temperature": 0.1,
  "max_tokens": 4096
}
```

`temperature: 0.1` keeps edits deterministic. `tool_choice: "auto"` lets the model
decide when to use tools vs reply with text.

### Editor wiring (`src/editor/mod.rs`)

`handle_agent_mode()` passes the project root to `submit()`:

```rust
let project_root = std::env::current_dir()
    .unwrap_or_else(|_| PathBuf::from("."));
let fut = panel.submit(context, project_root);
```

The project root is computed at submit time (not stored globally) so it reflects
the working directory at the moment the user sends the message.

---

## Consequences

- **Security boundary**: `safe_path()` prevents the model from traversing outside
  the project root, but the tool can still overwrite any file within it. No
  read-only enforcement — the model could delete or corrupt source files.
- **Blocking tool execution**: `execute_tool()` is synchronous and runs inside the
  `agentic_loop` tokio task. Large reads (multi-MB files) block that task's thread
  for the read duration. Acceptable for a terminal editor on local files.
- **No tool result truncation to model**: the full tool result is sent back to the
  model regardless of length. Very large files could saturate the context window and
  degrade response quality.
- **Context window growth**: each round appends assistant + tool messages. A 20-round
  conversation on a 500-line file can easily reach 50 k tokens, approaching gpt-4o's
  128 k limit. A summarisation or sliding-window strategy should be added.
- **Error recovery**: the `edit_file` tool returns a descriptive error when `old_str`
  is not found or appears multiple times. The agentic loop feeds this error back to
  the model, which is expected to `read_file` and retry with correct content.
