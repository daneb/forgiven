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
| [0013](0013-project-folder-argument.md) | Multi-Project Support: Project Folder Argument | Accepted |
| [0014](0014-agent-model-selection.md) | Agent Model Selection: Dynamic Discovery and Ctrl+T Cycling | Accepted |
| [0015](0015-file-creation-and-explorer-enhancements.md) | File Creation and Explorer Enhancements | Accepted |
| [0016](0016-vim-yank-paste-register.md) | Vim Yank / Paste Register | Accepted |
| [0017](0017-multi-line-yank-delete-visual-line.md) | Multi-line Yank / Delete and Visual Line Mode | Accepted |
| [0018](0018-horizontal-scroll-viewport-fix.md) | Horizontal Scroll Viewport Fix | Accepted |
| [0019](0019-snapshot-undo-redo.md) | Snapshot-based Undo / Redo | Accepted |
| [0020](0020-lazygit-integration.md) | LazyGit Integration | Accepted |
| [0021](0021-render-loop-performance.md) | Render Loop Performance Optimisations | Accepted |
| [0022](0022-markdown-rendering.md) | Markdown Rendering (Agent Panel + Editor Preview) | Accepted |
| [0023](0023-which-key-render-timer.md) | Which-Key Popup Render Timer | Accepted |
| [0024](0024-project-wide-text-search.md) | Project-wide Text Search | Accepted |
| [0025](0025-explorer-hidden-files-toggle.md) | Explorer Hidden Files Toggle | Accepted |
| [0026](0026-copilot-stream-resilience.md) | Copilot Stream Resilience | Accepted |
| [0027](0027-agent-round-limits-and-continuation-prompts.md) | Agent Round Limits and Continuation Prompts | Accepted |
| [0028](0028-model-selection-persistence.md) | Model Selection Persistence | Accepted |
| [0029](0029-task-panel-for-work-tracking.md) | Task Panel for Work Tracking | Accepted |
| [0030](0030-in-file-search-and-replace.md) | In-File Search and Replace | Accepted |
| [0031](0031-agent-task-creation.md) | Agent-Driven Plan Strip | Accepted |
| [0032](0032-recent-files-in-file-picker.md) | Recent Files in the Find File Picker | Accepted |
| [0033](0033-mermaid-and-markdown-browser-export.md) | Mermaid Diagrams and Markdown Browser Export | Accepted |
| [0034](0034-explorer-file-deletion.md) | Explorer File Deletion | Accepted |
| [0035](0035-agent-apply-diff.md) | Agent Apply-Diff Overlay | Accepted |
| [0036](0036-multi-line-agent-input.md) | Multi-line Agent Panel Input | Accepted |
| [0037](0037-think-block-rendering.md) | Think-Block Rendering in the Agent Panel | Accepted |
| [0038](0038-unified-model-selection.md) | Unified Model Selection: Removing the `model_picker_enabled` Filter | Accepted |
| [0039](0039-agent-status-indicator.md) | Agent Status Indicator: Live Phase Tracking in the Agent Panel Title | Accepted |
| [0040](0040-context-gauge.md) | Context Gauge: Token Usage Display in the Agent Panel Title | Accepted |
| [0041](0041-agent-panel-copy-code-block.md) | Agent Panel Copy Code Block (`c` key) | Accepted |
| [0042](0042-agent-paste-summary.md) | Agent Panel Paste Summary | Accepted |
| [0043](0043-vertical-split-screen.md) | Vertical Split Screen | Accepted |
| [0044](0044-explorer-new-folder.md) | Explorer New Folder | Accepted |
| [0045](0045-mcp-client.md) | MCP Client Integration | Accepted |
| [0046](0046-agent-retry-visibility.md) | Agent Retry Visibility | Accepted |
| [0047](0047-git-commit-message-generation.md) | Git Commit Message Generation | Accepted |
| [0048](0048-mcp-status-visualisation.md) | MCP Server Status Visualisation | Accepted |
| [0049](0049-diagnostics-overlay.md) | Diagnostics Overlay (`SPC d`) | Accepted |
| [0050](0050-mcp-env-var-secrets.md) | MCP Server Environment Variable Secret Resolution | Accepted |
| [0051](0051-startup-loading-indicator-and-parallelisation.md) | Startup Loading Indicator and Service Parallelisation | Accepted |

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
│  main.rs  [--dir DIR | DIR positional]                                        │
│    → set_current_dir(canonical)   ← project root for all downstream calls    │
│    → Editor::new() → Editor::run()  (tokio async main)                        │
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
│    │  │writer thread │  │   │  │          │  │          │  │[model] │  │    │
│    │  └──────────────┘  │   │  │ ▼ src/   │  │ syntect  │  │ chat   │  │    │
│    │  ┌──────────────┐  │   │  │   mod.rs │  │ highlight│  │ history│  │    │
│    │  │copilot-ls    │  │   │  │ ▶ tests/ │  │ ghost txt│  │ input  │  │    │
│    │  │LspClient     │  │   │  │ n=new    │  │          │  │Ctrl+T  │  │    │
│    │  └──────────────┘  │   │  │ r=reload │  │          │  │=model  │  │    │
│    └────────────────────┘   │  └──────────┘  └──────────┘  └────────┘  │    │
│                             └───────────────────────────────────────────┘    │
│              │                                                                │
│    ┌─────────┴──────────────────────────────────────────────────────────┐   │
│    │  AgentPanel                                                         │   │
│    │  messages: Vec<ChatMessage>      pending_reloads: Vec<String>       │   │
│    │  stream_rx: mpsc::UnboundedRx    streaming_reply: Option<String>    │   │
│    │  available_models: Vec<String>   selected_model: usize              │   │
│    │                                                                     │   │
│    │  ensure_models() ──────────────► GET /models (lazy, cached)        │   │
│    │  tokio::spawn(agentic_loop)  ──► api.githubcopilot.com             │   │
│    │    model_id = selected_model_id()                                   │   │
│    │    MAX_ROUNDS=20                  tools: read_file / write_file     │   │
│    │    parse SSE tool_call deltas           edit_file / list_directory  │   │
│    │    execute tools (safe_path sandbox)                                │   │
│    │    StreamEvent: Token | ToolStart | ToolDone | FileModified | Done  │   │
│    │                                                                     │   │
│    │  FileModified → pending_reloads → Buffer::reload_from_disk()       │   │
│    └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│  Highlighter (syntect)  — SyntaxSet + ThemeSet loaded once at startup        │
│  FileExplorer           — lazy tree rooted at current_dir(); reload() on r   │
│  clipboard: Option<String>  — shared yank/delete register                    │
└───────────────────────────────────────────────────────────────────────────────┘
```

## Mode Map

```
Normal ──── i/a/I/A/o/O ──► Insert
       ──── v           ──► Visual       (charwise, extend with h/j/k/l/w/b/0/$)
       ──── V           ──► VisualLine   (linewise, extend with j/k/G/g)
       ──── :           ──► Command      (:e path, :w, :q, :wq, :q!, copilot status/auth)
       ──── /           ──► InFileSearch (type pattern, Enter=search, Esc=cancel)
       ──── SPC b b     ──► PickBuffer
       ──── SPC f f     ──► PickFile     (fuzzy search)
       ──── SPC f n     ──► Command      (pre-filled "e " for new file)
       ──── SPC a a/f   ──► Agent
       ──── SPC e e/f   ──► Explorer
       ──── SPC m p     ──► MarkdownPreview

Explorer ── Esc/Tab     ──► Normal
         ── Enter/l     ──► (opens file → Normal) or (toggles dir)
         ── n           ──► Command      (pre-filled "e <dir>/" for new file)
         ── r           ──► RenameFile   (inline popup)
         ── d           ──► DeleteFile   (confirmation popup)
         ── h           ──► (toggle hidden files, stays in Explorer)
         ── R           ──► (reload tree from disk, stays in Explorer)

RenameFile ── Enter     ──► Explorer  (rename confirmed)
           ── Esc       ──► Explorer  (cancelled)

DeleteFile ── y/Y       ──► Explorer  (deleted)
           ── n/N/Esc   ──► Explorer  (cancelled)

Agent    ── Esc/Tab     ──► Normal
         ── Ctrl+T      ──► cycle model (loads /models list on first press)
         ── a (empty)   ──► ApplyDiff  (when a code block is present)

ApplyDiff ── y/Enter    ──► Normal     (change applied to file or buffer)
          ── n/Esc      ──► Agent      (discarded)
          ── j/k        ──► (scroll down/up one line)
          ── Ctrl+D/U   ──► (scroll down/up half-page)

Preview  ── Esc/q       ──► Normal
         ── j/k         ──► scroll down/up one line
         ── Ctrl+D/U    ──► scroll down/up half-page
         ── g/G         ──► jump to top/bottom

Insert ──── Esc         ──► Normal
```

