# Architecture Decision Records — forgiven

This directory contains the Architecture Decision Records (ADRs) for the **forgiven**
terminal code editor. Each ADR captures a significant technical decision: the context
that motivated it, what was decided, and the consequences.

## Index

| # | Title | Status |
|---|-------|--------|
| [0001](0001-terminal-ui-framework.md) | Terminal UI Framework: ratatui + crossterm | Accepted |
| [0002](0002-async-runtime-and-event-loop.md) | Async Runtime and Event Loop Design | Accepted |
| [0003](0003-lsp-integration-architecture.md) | LSP Integration Architecture | Accepted |
| [0004](0004-copilot-authentication.md) | GitHub Copilot Enterprise Authentication | Accepted |
| [0005](0005-copilot-inline-completions-ghost-text.md) | Copilot Inline Completions and Ghost Text | Accepted |
| [0006](0006-agent-chat-panel.md) | Copilot Chat / Agent Panel | Accepted |
| [0007](0007-vim-modal-keybindings.md) | Vim-style Modal Editing and Spacemacs Leader Keys | Accepted |
| [0008](0008-normal-mode-editing-operations.md) | Normal Mode Editing Operations and Multi-key Sequences | Accepted |
| [0009](0009-syntax-highlighting-syntect.md) | Syntax Highlighting with syntect | Accepted |
| [0010](0010-file-explorer-tree-sidebar.md) | File Explorer Tree Sidebar | Accepted |
| [0011](0011-agentic-tool-calling-loop.md) | Agentic Tool-Calling Loop | Accepted |
| [0012](0012-agent-ux-context-and-file-refresh.md) | Agent UX: Context Injection, File Refresh, and Chat Rendering | Accepted |

## What is an ADR?

An Architecture Decision Record documents an architectural decision made in a project.
The format used here follows the lightweight template:

- **Context** — why was this decision needed?
- **Decision** — what was decided?
- **Consequences** — what are the trade-offs and implications?

## Current Architecture Snapshot

```
┌───────────────────────────────────────────────────────────────────────────────┐
│                               forgiven editor                                 │
│                                                                               │
│  main.rs → Editor::new() → Editor::run()  (tokio async main)                 │
│                                                                               │
│  ┌────────────────────────────────────────────────────────────────────────┐   │
│  │  Editor event loop  (50 ms poll)                                       │   │
│  │  handle_key() → KeyHandler → Action → execute_action()                 │   │
│  │  drain_lsp_msgs() │ poll_agent_stream() │ render()                     │   │
│  └───────────┬────────────────────────────────────────────────────────────┘   │
│              │                                                                │
│    ┌─────────┴──────────┐   ┌───────────────────────────────────────────┐    │
│    │  LspManager        │   │  UI (ratatui) — three-panel layout        │    │
│    │  ┌──────────────┐  │   │                                           │    │
│    │  │rust-analyzer │  │   │  ┌──────────┐  ┌──────────┐  ┌────────┐  │    │
│    │  │LspClient     │  │   │  │ Explorer │  │  Editor  │  │ Agent  │  │    │
│    │  │reader thread │  │   │  │  25 cols │  │  Min(1)  │  │  35%   │  │    │
│    │  │writer thread │  │   │  │          │  │          │  │        │  │    │
│    │  └──────────────┘  │   │  │ ▼ src/   │  │ syntect  │  │ chat   │  │    │
│    │  ┌──────────────┐  │   │  │   mod.rs │  │ highlight│  │ history│  │    │
│    │  │copilot-ls    │  │   │  │ ▶ tests/ │  │ ghost txt│  │ input  │  │    │
│    │  │LspClient     │  │   │  └──────────┘  └──────────┘  └────────┘  │    │
│    │  └──────────────┘  │   │                                           │    │
│    └────────────────────┘   └───────────────────────────────────────────┘    │
│              │                                                                │
│    ┌─────────┴──────────────────────────────────────────────────────────┐   │
│    │  AgentPanel                                                         │   │
│    │  messages: Vec<ChatMessage>      pending_reloads: Vec<String>       │   │
│    │  stream_rx: mpsc::UnboundedRx    streaming_reply: Option<String>    │   │
│    │                                                                     │   │
│    │  tokio::spawn(agentic_loop)  ──► api.githubcopilot.com             │   │
│    │    MAX_ROUNDS=20                  tools: read_file / write_file     │   │
│    │    parse SSE tool_call deltas           edit_file / list_directory  │   │
│    │    execute tools (safe_path sandbox)                                │   │
│    │    StreamEvent: Token | ToolStart | ToolDone | FileModified | Done  │   │
│    │                                                                     │   │
│    │  FileModified → pending_reloads → Buffer::reload_from_disk()       │   │
│    └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│  Highlighter (syntect)  — SyntaxSet + ThemeSet loaded once at startup        │
│  FileExplorer           — lazy tree rooted at current_dir()                  │
│  clipboard: Option<String>  — shared yank/delete register                    │
└───────────────────────────────────────────────────────────────────────────────┘
```

## Mode Map

```
Normal ──── i/a/I/A/o/O ──► Insert
       ──── v           ──► Visual
       ──── :           ──► Command
       ──── SPC b b     ──► PickBuffer
       ──── SPC f f     ──► PickFile
       ──── SPC a a/f   ──► Agent
       ──── SPC e e/f   ──► Explorer

Explorer ── Esc/Tab     ──► Normal
         ── Enter/l     ──► (opens file → Normal) or (toggles dir)

Insert ──── Esc         ──► Normal
Agent  ──── Esc/Tab     ──► Normal
```

## Planned ADRs (future decisions)

- `0013` — Undo/redo history (persistent `EditHistory` with Op log)
- `0014` — Buffer model improvements (line-gap buffer or rope for large files)
- `0015` — Hover / go-to-definition popup widget
- `0016` — Configuration system (`~/.config/forgiven/config.toml`) — themes, keybindings, LSP per-language
- `0017` — File system watcher for live explorer refresh (`notify` crate)
- `0018` — Agent file attachment picker (`@`-mention files as context)
