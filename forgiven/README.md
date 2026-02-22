# Forgiven Editor

An AI-first, terminal-based code editor with GitHub Copilot agent integration, inspired by Emacs and Spacemacs.

## Project Status: Phase 1 Complete ✓

We've successfully completed the foundation phase with a fully functional text editor.

### ✅ What's Working

**Core Editor Features:**
- Buffer-centric architecture (all content lives in buffers)
- Multiple buffer support with buffer management
- Full cursor movement and text editing
- Undo/redo history tracking (recording operational)
- File loading and saving
- Horizontal and vertical scrolling

**Modes:**
- **Normal Mode**: Vim-like navigation (h/j/k/l, arrows, 0/$)
- **Insert Mode**: Full text insertion and deletion
- **Command Mode**: Colon commands (:w, :q, :wq, :q!)

**UI:**
- Clean terminal interface using Ratatui
- Status line showing:
  - Current mode (color-coded)
  - Buffer name and modified indicator
  - Cursor position
  - Status messages
- Command buffer display

**Keybindings (Vim-style for now):**
- `i` - insert at cursor
- `a` - append after cursor
- `I` - insert at line start
- `A` - append at line end
- `o` - open new line below
- `O` - open new line above
- `h/j/k/l` or arrows - navigation
- `0` or Home - line start
- `$` or End - line end
- `:` - command mode
- `Esc` - back to normal mode

## Running the Editor

### Build
```bash
cargo build --release
```

### Run
```bash
# Open a file
./target/release/forgiven test.txt

# Start with scratch buffer
./target/release/forgiven

# Open multiple files
./target/release/forgiven file1.txt file2.txt
```

### Basic Usage
1. Start in Normal mode (blue status bar)
2. Press `i` to enter Insert mode (green status bar)
3. Type your text
4. Press `Esc` to return to Normal mode
5. Press `:w` to save
6. Press `:q` to quit (or `:wq` to save and quit)

## Project Structure

```
forgiven/
├── src/
│   ├── main.rs              # Entry point and CLI
│   ├── buffer/              # Buffer management
│   │   ├── buffer.rs        # Core buffer with text operations
│   │   ├── cursor.rs        # Cursor position tracking
│   │   └── history.rs       # Undo/redo history
│   ├── editor/              # Main editor loop and state
│   │   └── mod.rs
│   ├── ui/                  # Terminal rendering
│   │   └── mod.rs
│   ├── keymap/              # Keybinding system
│   │   └── mod.rs
│   └── config/              # Configuration (placeholder)
│       └── mod.rs
└── Cargo.toml
```

## Architecture

**Buffer-Centric Design:**
- All content (files, virtual buffers) are managed as Buffer objects
- Buffers are the single source of truth for content
- UI renders from buffer state, never modifies directly

**Event-Driven:**
- Main loop polls for keyboard events
- Key events are processed based on current mode
- UI is redrawn after each event

**Async-Ready:**
- Built on Tokio runtime for future LSP and AI integration
- Terminal rendering is synchronous but doesn't block async operations

## Next Steps (Phase 2-4)

### Phase 2: UI/UX Polish
- [ ] Spacemacs-style leader key (SPC)
- [ ] Which-key popup menus
- [ ] Transient menus for command groups
- [ ] Multiple buffer switching (SPC b b)
- [ ] File explorer/fuzzy finder

### Phase 3: Language Server Protocol
- [ ] LSP client implementation
- [ ] Diagnostics display
- [ ] Hover information
- [ ] Go-to-definition
- [ ] Autocomplete
- [ ] Symbol navigation

### Phase 4: GitHub Copilot Integration
- [ ] GitHub Enterprise authentication
- [ ] Inline AI suggestions
- [ ] Agent mode for multi-step operations
- [ ] Match & apply functionality
- [ ] Context-aware code generation

### Phase 5+: Advanced Features
- [ ] Codebase search for AI agent
- [ ] Test generation
- [ ] Code refactoring agent
- [ ] Lua scripting engine
- [ ] Custom keybindings
- [ ] Theme system

## Technology Stack

- **Language**: Rust 🦀
- **TUI**: Ratatui + Crossterm
- **Async Runtime**: Tokio
- **CLI**: Clap
- **Logging**: Tracing

## Development

Logs are written to `/tmp/forgiven.log` to avoid interfering with the TUI.

```bash
# Watch logs while testing
tail -f /tmp/forgiven.log
```

## License

[To be determined]

## Contributing

This project is in early development. Contributions welcome once we reach Phase 2!
