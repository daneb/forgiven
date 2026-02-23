# AI-First Terminal Editor: Strategic Development Plan

## Executive Summary

Build a terminal-based, buffer-centric code editor inspired by Emacs and Spacemacs, engineered from the ground up for AI-assisted development with native GitHub Copilot agent integration. This editor prioritizes developer productivity, customization depth, and seamless AI-augmented workflows while maintaining the philosophical principles of Emacs (buffer model, composability, extensibility) in a modern, performant architecture.

---

## Vision & Goals

### Core Philosophy
- **Buffer-centric model**: All content (files, REPLs, LSP diagnostics, AI interactions) live in buffers, managed through a unified interface
- **Terminal-first**: No dependency on graphical toolkits; lightweight, ssh-friendly, fast
- **AI-native from inception**: Not bolted-on; core workflows assume AI assistance is available
- **Full user control**: Deep customization and scripting capabilities for advanced users
- **Spacemacs-inspired UX**: Mnemonic keybindings, which-key popups, transient menus, discoverability through the UI

### Business/Productivity Goals
1. Provide GitHub Enterprise Copilot integration with agent/agentic capabilities (not just autocomplete)
2. Enable AI agents to perform meaningful code operations: refactoring, file creation, codebase search, test generation
3. Support "match and apply" functionality where AI understands code changes and applies them intelligently
4. Deliver a smooth, integrated experience that feels cohesive and thought-out (unlike Emacs + plugin patchworks)
5. Allow developers to stay in the terminal without switching contexts to VSCode or browser-based tools

### Technical Goals
1. Build a performant, maintainable codebase in Rust for reliability and speed
2. Create an extensible architecture that allows scripting/customization without recompiling
3. Establish clear boundaries between core editor, AI integration, and user customization layers
4. Support multiple language servers (LSP) and integrate them seamlessly with AI features
5. Maintain a clean, composable design that follows Unix philosophy (do one thing well)

---

## Why This Approach

### Why Not Emacs?
- Emacs's extension model (Elisp + imperative plugin system) doesn't naturally accommodate agentic AI workflows
- Terminal rendering in Emacs is functional but not optimized for smooth, streaming AI interactions
- Enterprise GitHub Copilot integration in Emacs is inconsistent and not first-class
- Emacs's architecture makes it difficult to implement agent mode where the AI can perform multiple coordinated operations

### Why Tauri/Rust + Terminal TUI?
- **Rust**: Memory-safe, performant, excellent for systems programming; avoids entire classes of bugs
- **Terminal-based**: Lightweight, universally accessible, matches developer workflow preferences, no GUI overhead
- **Custom architecture**: Freedom to design buffer model, keybinding system, and AI integration without fighting existing paradigms
- **Tauri consideration**: Initially rejected because terminal-first is primary; however, optional Tauri GUI layer could be added later if needed

### Why This Will Succeed Where Emacs Struggles
1. **Native agent support**: Architecture designed for multi-step AI operations, context management, and tool use
2. **Enterprise auth**: GitHub Enterprise Copilot authentication built in from day one
3. **Streaming UX**: Terminal rendering optimized for real-time AI suggestions and agentic responses
4. **Composable AI**: AI features compose with editor operations (select buffer → send to AI → apply changes → continue editing)

---

## High-Level Architecture

### Layer 1: Core Editor Engine
**Purpose**: Provide fundamental text editing, buffer management, and rendering

**What needs to be achieved:**
- Buffer abstraction: Multiple independent buffers, each with cursor position, selection state, undo/redo history
- Text operations: Insertion, deletion, selection, navigation, find/replace
- Keybinding dispatch: Map key sequences to editor commands
- Terminal rendering: Efficient screen updates, syntax highlighting, status line, UI elements
- File I/O: Load/save buffers, watch for external changes, handle encodings

**Key architectural decisions:**
- Buffers as first-class entities, not tied to files
- Immutable-friendly design for undo/redo
- Pluggable rendering backend (initially TUI, optionally Tauri later)

---

### Layer 2: UI/UX & Keybinding System (Spacemacs-Inspired)
**Purpose**: Provide intuitive, discoverable, mnemonic interface

**What needs to be achieved:**
- Keybinding system supporting leader key (space), transient menus, which-key style help
- Modal system: normal mode (navigation), insert mode (editing), command mode (actions)
- Transient menus: Context-sensitive popups for related commands (buffer operations, AI operations, etc.)
- Status line and mode line: Display current mode, buffer state, AI connectivity status
- Discoverability: Users can explore available commands through UI without memorizing all keybindings
- Customizable keybindings: Users can remap keys, create macros, define sequences

**Key design principles:**
- Mnemonics: `SPC b b` = buffer list, `SPC a i` = AI operations, `SPC f f` = find file
- Consistency: Similar operations grouped under same prefix
- Progressive disclosure: Basic operations are discoverable; advanced features are available but not overwhelming

---

### Layer 3: Language & Diagnostic Support (LSP Integration)
**Purpose**: Provide IDE-like features (autocomplete, diagnostics, navigation) without being an IDE

**What needs to be achieved:**
- LSP client: Connect to language servers (Rust, Python, TypeScript, Go, etc.)
- Diagnostics display: Show errors/warnings in buffers, configurable presentation
- Hover information: Display type info, documentation on demand
- Go-to-definition: Navigate to symbol definitions across project
- Autocomplete: Non-AI autocomplete from LSP for baseline functionality
- Symbol navigation: Outline view, document symbols, workspace symbols
- Refactoring: Rename, extract, organize imports (via LSP)

**Integration with AI:**
- AI features build on top of LSP context (AI knows about available symbols, types, documentation)
- LSP diagnostics inform AI about what needs fixing (errors become context for agent)

---

### Layer 4: GitHub Copilot Integration (Agent-First)
**Purpose**: Seamless AI-assisted development as a first-class feature

**What needs to be achieved:**

#### 4.1 Authentication & API Access
- GitHub Enterprise OAuth flow for token acquisition
- Secure token storage and refresh
- Connection to GitHub Copilot API (or copilot-node-server)
- Fallback/graceful degradation if Copilot unavailable

#### 4.2 AI Suggestion System (Beyond Autocomplete)
- **In-line suggestions**: Real-time suggestions as you type (streaming)
- **Context gathering**: Collect relevant context (current buffer, related files, LSP info, project structure)
- **Suggestion filtering**: Filter/rank suggestions based on relevance
- **Acceptance/rejection UI**: Accept, reject, show alternatives, customize suggestions

#### 4.3 Agent Mode (Multi-Step Operations)
- **Agent invocation**: `SPC a a` = "Ask AI agent"
- **Intent specification**: User describes what they want (via prompt)
- **Tool availability**: Agent has access to editor tools:
  - Create/edit files
  - Search codebase (grep, symbol search)
  - Run commands/tests
  - Apply diffs to buffers
  - Check syntax/build
- **Multi-turn interaction**: Agent can ask clarifying questions, propose changes, iterate
- **Change approval**: User reviews proposed changes before application
- **Result buffers**: Agent's output goes to named buffers (e.g., `*copilot-suggestions*`, `*generated-tests*`)

#### 4.4 Match & Apply Functionality
- **Diff parsing**: Parse unified diffs from AI responses
- **Intelligent application**: Apply diffs even if file has changed since context was gathered
- **Conflict resolution**: Show conflicts, allow manual resolution
- **Preview mode**: Show changes before applying (split view, side-by-side)
- **Undo support**: All AI-applied changes are undoable

#### 4.5 Contextual AI Features
- **Explain code**: Ask AI to explain selection or buffer
- **Generate tests**: AI writes unit tests for function/class
- **Refactor**: AI suggests and applies refactorings
- **Documentation**: AI writes docstrings, comments
- **Fix errors**: AI proposes fixes for LSP diagnostics
- **Code review**: AI reviews code, suggests improvements

---

### Layer 5: Customization & Scripting
**Purpose**: Allow power users to extend and customize without recompiling

**What needs to be achieved:**
- Scripting language: Embedded Lua or similar (lightweight, embeddable, easy to learn)
- Configuration file: User-writable config (e.g., `~/.editor/config.lua`) loaded on startup
- Custom commands: Users can define new editor commands in scripts
- Custom keybindings: Map keybindings to built-in or custom commands
- Hooks & events: Lifecycle hooks (on-open, on-save, on-buffer-switch, etc.)
- Buffer-local settings: Per-file customization (indentation, line endings, formatters)
- Themes: Customizable color schemes, syntax highlighting rules
- Plugin system (future): If needed, support for compiled plugins or higher-level script extensions

**User experience:**
- Default config provided with sensible defaults
- Extensive documentation with examples
- Community-shared configurations

---

### Layer 6: Project & File Management
**Purpose**: Context-aware file and project operations

**What needs to be achieved:**
- Project root detection: Identify project boundaries (.git, Cargo.toml, package.json, etc.)
- File tree/explorer: Browse project files, open/create/delete
- Recent files: Quick access to frequently used files
- File search: Fast file finding (via ripgrep or similar)
- Fuzzy finding: Fzf-style selection UI for files, buffers, commands
- Ignore patterns: Respect .gitignore, .editorignore
- Project-specific settings: Load per-project configurations

---

## Implementation Phases

### Phase 1: Foundation (Weeks 1-3)
**Deliverable**: Functional text editor with basic keybindings

**Must achieve:**
- Core buffer model and text operations
- Terminal rendering (using Ratatui or Crossterm)
- Basic keybinding system (navigation, insert, delete, undo/redo)
- File loading/saving
- Syntax highlighting (basic, tree-sitter integration)
- Status line showing mode and file info
- Simple command mode (`:q`, `:w`, `:e`)

**Not included:**
- AI features
- LSP integration
- Customization scripting
- Spacemacs-style UI

---

### Phase 2: UI/UX Polish (Weeks 4-5)
**Deliverable**: Spacemacs-inspired interface with discoverable keybindings

**Must achieve:**
- Leader key system (space as leader)
- Which-key style help popups
- Transient menus for command groups
- Modal system refinement (normal/insert/command/visual modes)
- Buffer management UI (switch, list, delete buffers)
- Better status line (show AI connectivity, LSP status)
- File/buffer navigation (`SPC f f`, `SPC b b`)
- Search and navigation commands

---

### Phase 3: Language Server Integration (Weeks 6-8) — ✅ COMPLETED
**Deliverable**: IDE-like features for code understanding

**Status (as of Feb 22, 2026):** Core features complete
- ✅ LSP client architecture and async integration
- ✅ Core LSP methods (hover, goto-definition, completion, rename, symbols, references)
- ✅ Diagnostics storage infrastructure
- ✅ Editor integration (did_open, did_change, did_save notifications)
- ✅ Diagnostics display with visual indicators (gutter markers, status bar counts)
- ✅ Diagnostic navigation (]d / [d for next/previous)
- ✅ LSP server auto-start based on file extension
- ✅ Document version tracking and real-time sync
- ✅ LSP keybindings under SPC l (hover, definition, references, rename, symbols)
- 📄 See PHASE3_PLAN.md and PHASE3_TESTING.md for details

**Completed features:**
- LSP client implementation ✅
- Diagnostics display and navigation ✅
- Language server auto-spawn with configuration ✅
- did_change notifications on every edit ✅
- Keybinding infrastructure for all LSP operations ✅

**Partially implemented (infrastructure ready, needs UI completion):**
- Hover information (request sent, needs popup rendering)
- Go-to-definition (request sent, needs file navigation)
- Find references (request sent, needs results display)
- Document symbols (request sent, needs picker UI)
- Rename (infrastructure ready, needs input prompt)

---

### Phase 4: GitHub Copilot Integration (Weeks 9-12)
**Deliverable**: Enterprise Copilot integration with basic agent support

**Must achieve:**
- GitHub Enterprise authentication flow
- In-line suggestion system with streaming
- Agent mode with basic tool access (file create/edit)
- Diff parsing and application
- Change preview and approval UI
- Context gathering (current buffer, related files, project structure)
- Simple multi-turn agent interaction

---

### Phase 5: Advanced Agent Features (Weeks 13-16)
**Deliverable**: Rich agent capabilities for real development tasks

**Must achieve:**
- Codebase search tool for agent (grep, symbol search)
- Test generation agent
- Code refactoring agent
- Documentation generation
- Error fixing agent (LSP diagnostics as input)
- File creation/management by agent
- Complex multi-step workflows

---

### Phase 6: Customization & Scripting (Weeks 17-19)
**Deliverable**: User customization and extensibility

**Must achieve:**
- Lua scripting engine integration
- Configuration file loading and execution
- Custom command definition
- Custom keybinding system
- Hooks and lifecycle events
- Theme system
- Buffer-local settings

---

### Phase 7: Polish & Testing (Weeks 20+)
**Deliverable**: Production-ready application

**Must achieve:**
- Comprehensive error handling
- Performance optimization
- Documentation (user guide, API docs, examples)
- Test coverage for critical paths
- Edge case handling (large files, binary files, network issues)
- Cross-platform testing (Linux, macOS, potentially Windows)
- Community feedback incorporation

---

## Key Technical Decisions

### Technology Stack
- **Language**: Rust (systems language, memory safety, performance)
- **TUI Library**: Ratatui (modern Rust TUI framework) or Crossterm (lower-level control)
- **LSP Client**: `lsp-types` + `tokio` for async communication
- **Syntax Highlighting**: Tree-sitter for parsing and highlighting
- **Search**: Integration with ripgrep for file/content search
- **Scripting**: Embedded Lua (mlua crate) for user customization
- **Async Runtime**: Tokio for handling I/O, LSP, API calls concurrently

### Architectural Principles
1. **Separation of concerns**: Core editor, UI, LSP, AI features are independent layers
2. **Event-driven**: Core loop processes user input, LSP notifications, AI responses as events
3. **Buffer as source of truth**: All operations flow through buffer abstraction
4. **Async throughout**: Long-running operations (LSP, AI) don't block UI
5. **Composable commands**: Commands can be combined, scripted, extended
6. **Stateless rendering**: Render state computed fresh each frame from buffer state

### Concurrency Model
- Main loop: Handles input and rendering (single-threaded TUI)
- Background tasks: LSP client, Copilot API calls run on Tokio runtime
- Channel-based communication: Background tasks send updates to main loop via channels
- No shared mutable state: Events passed between components, not state mutation

---

## Success Criteria

### MVP (Minimal Viable Product)
1. **Functional text editor** that can open, edit, and save files smoothly
2. **GitHub Enterprise Copilot** authentication and basic in-line suggestions working
3. **Agent mode** where user can ask AI to perform a task (create file, refactor code) and review changes
4. **Spacemacs-inspired UI** with discoverable keybindings and which-key help
5. **Basic LSP integration** showing diagnostics and autocomplete
6. **No crashes or data loss** on normal usage (robust error handling)

### Post-MVP (Enhancement Goals)
1. Advanced agent capabilities (multi-step workflows, codebase search, test generation)
2. Full customization scripting system
3. Performance optimizations for large files and projects
4. Community plugins/extensions
5. Optional GUI layer (Tauri) for users who prefer graphical interface

---

## Risk Mitigation

### Risk 1: Scope Creep
**Mitigation**: Strict phase-based approach. Each phase has clear deliverable. Post-MVP features explicitly separated.

### Risk 2: Performance Issues with TUI Rendering
**Mitigation**: Early prototyping with real-world workloads. Incremental rendering strategies. Profile and optimize before moving to next phase.

### Risk 3: Copilot API Changes
**Mitigation**: Monitor Copilot API updates. Abstract API layer allows quick pivots. Fallback to alternative AI providers researched.

### Risk 4: User Customization Complexity
**Mitigation**: Lua chosen for simplicity and embeddability. Extensive examples provided. Gradual feature expansion based on community feedback.

### Risk 5: Cross-Platform Issues
**Mitigation**: Regular testing on Linux, macOS. Windows support considered but not required for MVP.

---

## Dependencies & Resources

### External Services
- GitHub Copilot API (requires GitHub Enterprise account for testing)
- Language servers (Rust-analyzer, Pyright, TypeScript LSP, etc.)

### Open Source Libraries (Rust Ecosystem)
- `ratatui` or `crossterm`: TUI rendering
- `tokio`: Async runtime
- `tree-sitter`: Syntax parsing
- `lsp-types` + `lsp-client`: Language Server Protocol
- `mlua`: Lua scripting
- `serde`: Serialization
- `regex` or `fancy-regex`: Pattern matching
- `clap`: CLI argument parsing

### Documentation & Learning
- Spacemacs documentation (keybinding conventions)
- LSP specification
- GitHub Copilot API documentation
- Rust async/await patterns
- Tree-sitter documentation

---

## Post-Launch Considerations---

## 🔧 Technical Debt & Code Organization

### Current Status (Feb 2026)
- **Large mod.rs files**: `editor/mod.rs` (~870 lines), `lsp/mod.rs` (~600 lines)
- **Partial separation achieved**: LSP now has `config.rs` module for server configurations
- **Editor monolith persists**: Rendering, input, file ops, LSP integration still in one file

### Recommended Refactoring (Before Phase 4)
```
src/editor/
  mod.rs           # Public API, Editor struct
  render.rs        # UI rendering logic
  input.rs         # Key handling, actions
  file_ops.rs      # File opening/saving/scanning
  lsp_integration.rs  # LSP methods and handlers
  picker.rs        # Buffer/file pickers

src/lsp/
  mod.rs           # Public API, LspManager
  client.rs        # LspClient implementation
  config.rs        # Server configs ✅ (done)
  handlers.rs      # Notification handlers
```

**Priority**: MEDIUM - Current structure is maintainable but will benefit from cleanup
**Recommendation**: Refactor during Phase 4 when adding more complex AI features

### What's Working Well
- ✅ Clean separation between buffer, cursor, history modules
- ✅ LSP client and manager responsibilities are clear
- ✅ UI module is focused and handles rendering only
- ✅ Keymap module is well-organized with leader key tree

---

## 📈 Current Project Status (Feb 22, 2026)

### Completed Phases
- ✅ **Phase 1**: Foundation (text editor, buffer model, basic UI)
- ✅ **Phase 2**: Spacemacs-inspired UX (leader keys, which-key, visual mode, buffer management)
- ✅ **Phase 3**: Language Server Integration (diagnostics, LSP client, server auto-start, document sync)
  - Core features complete and testable
  - Some advanced features have infrastructure ready but need UI completion
  - See `PHASE3_TESTING.md` for comprehensive testing guide

### Ready for Next Phase
- 🚀 **Phase 4**: GitHub Copilot Integration
  - Foundation is solid with LSP integration complete
  - Buffer model, keybinding system, and async infrastructure ready for AI features
  - Can begin implementing authentication, suggestion system, and agent mode

### Upcoming
- ⏳ **Phase 5**: Advanced Agent Features
- ⏳ **Phase 6**: Customization & Scripting
- ⏳ **Phase 7**: Polish & Testing

### Key Achievements This Sprint
1. **Diagnostics System**: Visual error/warning indicators in gutter, status bar counts, navigation
2. **LSP Auto-Start**: Language servers spawn automatically based on file type
3. **Document Sync**: Real-time did_change notifications keep LSP in sync with edits
4. **Version Tracking**: LSP document version increments with each buffer modification
5. **Keybinding Integration**: Full LSP command set under `SPC l` prefix
6. **Testing Documentation**: Comprehensive `PHASE3_TESTING.md` created

### Technical Highlights
- Resolved complex borrow checker issues in diagnostic navigation
- Implemented clean separation between LSP client, manager, and editor
- Created extensible configuration system for language servers
- Added graceful degradation when language servers aren't installed

---

## Future Directions
1. **GUI variant**: Tauri-based graphical interface sharing core engine
2. **Extension marketplace**: Community-contributed scripts and themes
3. **Remote development**: SSH support, remote file editing
4. **Collaborative features**: Shared buffers, real-time collaboration
5. **Mobile client**: Potential lightweight mobile interface
6. **Alternative AI providers**: Support for Claude, Llama, local LLMs
7. **Advanced agent types**: Specialized agents for specific tasks (DevOps, ML, etc.)

### Community Building
- Open source the project (appropriate license)
- Documentation and tutorials
- Community forums or Discord
- Regular releases and updates
- User feedback loops

---

## Glossary & Terminology

- **Buffer**: In-memory representation of content (file or virtual)
- **Agent mode**: AI system that can perform multi-step operations, use tools, and modify code
- **Match & apply**: Parsing AI-generated diffs and intelligently applying them to buffers
- **LSP**: Language Server Protocol for IDE-like features
- **Copilot agent**: GitHub Copilot's agentic capabilities (vs. simple autocomplete)
- **Transient menu**: Temporary menu that disappears after selection or timeout
- **Which-key**: Keybinding help system showing available commands after partial input
- **TUI**: Text User Interface (terminal-based)
- **Spacemacs**: Popular Emacs configuration emphasizing mnemonic keybindings

---

## Conclusion

This plan outlines a realistic, phased approach to building an AI-first editor that captures the strengths of Emacs (buffer model, customization, composability) while adding modern AI capabilities and a polished, integrated experience. The Rust + terminal architecture provides a solid foundation for performance, reliability, and long-term maintainability. By prioritizing GitHub Copilot agent integration from the start, we ensure AI isn't bolted on but genuinely integrated into the editing experience.

Success depends on maintaining focus during phases 1-4 (reaching MVP), then iterating based on real user feedback in phases 5-7.