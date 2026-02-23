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

## What is an ADR?

An Architecture Decision Record documents an architectural decision made in a project.
The format used here follows the lightweight template:

- **Context** — why was this decision needed?
- **Decision** — what was decided?
- **Consequences** — what are the trade-offs and implications?

## Current Architecture Snapshot

```
┌─────────────────────────────────────────────────────────────────────┐
│                          forgiven editor                            │
│                                                                     │
│  main.rs → Editor::new() → Editor::run()  (tokio async main)       │
│                                                                     │
│  ┌──────────────────────┐    ┌────────────────────────────────────┐ │
│  │   Editor event loop  │    │  LspManager                        │ │
│  │   (16 ms poll)       │◄──►│  ┌──────────────┐ ┌────────────┐  │ │
│  │                      │    │  │ rust-analyzer │ │  copilot-  │  │ │
│  │  - handle_key()      │    │  │ LspClient     │ │  language- │  │ │
│  │  - drain_lsp_msgs()  │    │  │ reader thread │ │  server    │  │ │
│  │  - poll_agent_stream │    │  │ writer thread │ │  LspClient │  │ │
│  │  - render()          │    │  └──────────────┘ └────────────┘  │ │
│  └──────────────────────┘    └────────────────────────────────────┘ │
│           │                                                         │
│           │ ghost_text / pending_completion                         │
│           │                                                         │
│  ┌────────▼────────────┐    ┌────────────────────────────────────┐ │
│  │   AgentPanel        │    │   UI (ratatui)                     │ │
│  │                     │    │                                    │ │
│  │  messages: Vec      │    │  60% editor │ 40% agent panel      │ │
│  │  stream_rx: mpsc    │    │             │ history (scrollable) │ │
│  │  scroll: usize      │◄───┤             │ input box (3 lines)  │ │
│  │  token: CopilotToken│    │  ghost text │                      │ │
│  └─────────────────────┘    └────────────────────────────────────┘ │
│           │                                                         │
│  tokio::spawn → reqwest SSE stream → StreamEvent mpsc channel       │
│           │                                                         │
│  api.githubcopilot.com/chat/completions                             │
└─────────────────────────────────────────────────────────────────────┘
```

## Planned ADRs (future decisions)

- `0008` — Hover / Go-to-definition popup widget
- `0009` — Buffer model and undo/redo history
- `0010` — Syntax highlighting strategy (tree-sitter vs LSP semantic tokens)
- `0011` — Agent tool-calling loop (MCP / function calling)
- `0012` — Configuration system (`~/.config/forgiven/config.toml`)
