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
| [0041](0041-agent-panel-copy-code-block.md) | Agent Panel Copy Code Block (`Ctrl+K`) and Yank Reply (`Ctrl+Y`) | Accepted |
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
| [0052](0052-dotnet-lsp-csharp-ls.md) | .NET LSP — Switch Default to csharp-ls | Accepted |
| [0053](0053-mcp-container-isolation.md) | MCP Non-blocking Startup (isolation superseded) | Accepted |
| [0054](0054-editor-quality-of-life-improvements.md) | Editor Quality-of-Life Improvements | Accepted |
| [0055](0055-release-notes-generation.md) | Release Notes Generation (`SPC g n`) | Accepted |
| [0056](0056-spec-framework-integration.md) | Pluggable Prompt-Framework Integration (spec-kit) | Accepted |
| [0057](0057-ask-user-tool.md) | Agent `ask_user` Tool | Accepted |
| [0058](0058-agent-panel-render-performance.md) | Agent Panel Rendering Performance | Accepted |
| [0059](0059-agent-file-context-picker.md) | Agent File Context Picker (Ctrl+P) | Accepted |
| [0060](0060-vim-char-motions.md) | Vim Character Motions (f/t/F/T, dt/df/yt/yf/ct/cf) | Accepted |
| [0061](0061-agent-stream-abort-and-ctrl-chord-migration.md) | Agent Stream Abort (`Ctrl+C`) and Ctrl-Chord Keybinding Migration | Accepted |
| [0062](0062-offline-resilience.md) | Offline Resilience: Request Timeouts, MCP Startup Bound, and Error Visibility | Accepted |
| [0063](0063-structural-refactor-buffer-combinator-render-context-editor-substates.md) | Structural Refactor: Buffer Combinator, RenderContext, and Editor Sub-states | Accepted |
| [0064](0064-filesystem-watcher-external-reload.md) | Filesystem Watcher: External Change Detection and Auto-Reload | Accepted |
| [0065](0065-terminal-redraw-on-resume.md) | Terminal Redraw on Resume (Resize, SIGCONT, Ctrl+L) | Accepted |
| [0066](0066-agent-image-clipboard-paste.md) | Agent Image Clipboard Paste | Accepted |
| [0067](0067-agent-input-scroll-follow-cursor.md) | Agent Input Box Scroll-to-Cursor | Accepted |
| [0068](0068-which-key-dynamic-height-and-ask-user-dialog-formatting.md) | Which-Key Dynamic Height and Ask-User Dialog Formatting | Accepted |
| [0069](0069-model-loading-modernisation.md) | Model Loading Modernisation | Accepted |
| [0070](0070-markdown-preview-visual-refresh.md) | Markdown Preview Visual Refresh | Accepted |
| [0071](0071-file-watcher-self-save-suppression.md) | File Watcher Self-Save Suppression | Accepted |
| [0072](0072-mcp-child-process-cleanup.md) | MCP Child Process Cleanup on Exit | Accepted |
| [0073](0073-mcp-http-transport-external-servers.md) | MCP HTTP Transport: Externally-Managed Servers | Accepted |
| [0074](0074-agentic-loop-token-refresh.md) | Agentic Loop Mid-Session Token Refresh | Accepted |
| [0075](0075-slash-menu-description-hints.md) | Slash-Menu Description Hints | Accepted |
| [0076](0076-mermaid-diagram-browser-preview.md) | Mermaid Diagram Browser Preview (`Ctrl+M`) | Accepted |
| [0077](0077-agent-context-window-management.md) | Agent Context Window Management (`SPC a n`, token-aware truncation) | Accepted |
| [0078](0078-prompt-caching.md) | Prompt Caching — Cached Token Tracking | Accepted |
| [0079](0079-diff-only-tool-results.md) | Diff-Only Tool Results for File Write/Edit Operations | Accepted |
| [0080](0080-tool-call-batching.md) | Tool Call Batching — read_files and search_files | Accepted |
| [0081](0081-importance-scored-history.md) | Importance-Scored History Retention | Accepted |
| [0082](0082-symbol-aware-context-tools.md) | Symbol-Aware Context Tools (get_file_outline, get_symbol_context) | Accepted |
| [0083](0083-mcp-memory-server.md) | MCP Memory Server for Cross-Session Context | Accepted |
| [0084](0084-llmlingua-mcp-sidecar.md) | LLMLingua MCP Sidecar for Tool Result Compression | Accepted |
| [0085](0085-lsp-navigation-and-csharp-revival.md) | LSP Navigation (Goto Definition, Find References, Symbols) and C# Revival | Accepted |
| [0086](0086-copilot-model-switch-detection-and-429-handling.md) | Copilot Model-Switch Detection and 429 Rate-Limit Handling | Accepted |
| [0087](0087-context-bloat-audit-and-instrumentation.md) | Context Bloat Audit and Session Token Instrumentation | Accepted |
| [0088](0088-auto-compress-tool-results-llmlingua.md) | Automatic Tool-Result Compression via LLMLingua | Accepted |
| [0089](0089-large-file-split-editor-agent-ui.md) | Large File Split: editor, agent, and ui modules | Accepted |
| [0090](0090-visual-indent-markdown-tables-mcp-call-log.md) | Visual indent/dedent, Markdown table rendering, MCP call log | Accepted |
| [0091](0091-lsp-hover-and-rename.md) | LSP Hover popup and Rename (WorkspaceEdit) | Accepted |
| [0092](0092-persistent-session-metrics-jsonl.md) | Persistent Session Metrics JSONL Log | Accepted |
| [0093](0093-cap-open-file-context-injection.md) | Cap Open-File Context Injection to 150 Lines | Accepted |
| [0094](0094-fetch-models-before-budget-computation.md) | Fetch Models Before Context-Budget Computation | Accepted |
| [0095](0095-persistent-log-file.md) | Persistent Log File at XDG Data Directory | Accepted |
| [0096](0096-session-rounds-and-avg-tokens-diagnostic.md) | Session Rounds Counter and Average Tokens per Invocation | Accepted |
| [0097](0097-speckit-auto-clear-context-per-phase.md) | Spec-Kit Auto-Clear Context Per Phase | Accepted |
| [0098](0098-ollama-local-provider.md) | Ollama Local Provider | Accepted |
| [0099](0099-context-breakdown-token-awareness.md) | Context Breakdown: Per-Segment Token Awareness (Phase 1) | Accepted |
| [0100](0100-spec-slicer-virtual-context.md) | Spec Slicer: Virtual Context for Implement Phase | Accepted |
| [0101](0101-auto-janitor-rolling-summary.md) | Auto-Janitor: Rolling History Compression (Phase 3) | Accepted |
| [0102](0102-submit-hot-path-optimisations.md) | Submit Hot-Path Optimisations | Accepted |
| [0103](0103-speckit-output-formatting-conventions.md) | Speckit Output Formatting Conventions | Accepted |
| [0104](0104-tree-sitter-core-integration.md) | Tree-sitter Core Integration | Accepted |
| [0105](0105-tree-sitter-text-objects.md) | Tree-sitter Text Objects | Accepted |
| [0106](0106-code-folding.md) | AST-Based Code Folding | Accepted |
| [0107](0107-sticky-scroll.md) | Sticky Scroll Context Header | Accepted |
| [0108](0108-no-multi-cursor.md) | Non-Decision: Multi-Cursor Editing | Accepted |
| [0109](0109-no-integrated-terminal.md) | Non-Decision: Integrated Terminal Pane | Accepted |
| [0110](0110-surround-operations.md) | Surround Operations | Accepted |
| [0111](0111-inline-assistant.md) | Inline Assistant (Selection Transform) | Accepted |
| [0112](0112-agent-checkpoints.md) | Agent Checkpoints / Session Undo | Accepted |
| [0113](0113-multi-file-review.md) | Multi-File Review / Change Set View | Accepted |
| [0114](0114-agent-hooks.md) | Agent Hooks / Background Automation | Accepted |
| [0115](0115-agent-brevity-constraints.md) | Agent Brevity Constraints | Accepted |
| [0116](0116-multi-provider-llm-backend.md) | Multi-Provider LLM Backend (Anthropic, OpenAI, Gemini, OpenRouter) | Accepted |
| [0117](0117-janitor-fixes.md) | Auto-Janitor Fixes: Archive Preservation, Input Save/Restore, Status Variants, Ollama Fallback | Accepted |
| [0118](0118-aggregate-file-block-token-cap.md) | Aggregate File-Block Token Cap | Accepted |
| [0119](0119-cpu-memory-performance-pass.md) | CPU & Memory Performance Pass (Janitor / Streaming) | Accepted |
| [0120](0120-janitor-distinct-streaming-ux.md) | Auto-Janitor Distinct Streaming UX | Accepted |
| [0121](0121-janitor-deferred-trigger-and-context-grounding.md) | Auto-Janitor: Deferred Trigger and Context Grounding | Accepted |
| [0122](0122-no-duplicate-file-buffers.md) | No Duplicate File Buffers | Accepted |
| [0123](0123-context-management-v2-observation-masking-and-disk-persistence.md) | Context Management v2: Observation Masking + Disk Persistence | Implemented |
| [0124](0124-agent-input-history-cursor-and-label-rename.md) | Agent Input History, Mid-line Cursor, and Label Rename | Accepted |
| [0125](0125-csv-and-json-preview-modes.md) | CSV and JSON Preview Modes | Accepted |
| [0126](0126-token-efficiency-llm-interaction-quality-analysis.md) | Token Efficiency and LLM Interaction Quality Analysis | Accepted |
| [0127](0127-dependency-audit-and-suppression-policy.md) | Dependency Audit and Advisory Suppression Policy | Accepted |
| [0128](0128-investigation-subagent.md) | Investigation Subagent (`SPC a v`) | Accepted |
| [0129](0129-insights-dashboard.md) | Insights Dashboard: Collaboration Analytics | Accepted |

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
         ── Ctrl+C      ──► stop agent  (cancels in-flight LLM request)
         ── Ctrl+K      ──► copy code block (cycles through blocks; wraps)
         ── Ctrl+Y      ──► yank full reply to clipboard (raw markdown)
         ── Ctrl+A      ──► ApplyDiff  (when a code block is present)

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

