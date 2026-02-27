# Phase 3: LSP Integration — Testing Guide

## Overview

This guide helps you test the LSP (Language Server Protocol) integration features in Forgiven. Phase 3 adds IDE-like capabilities including diagnostics, hover information, go-to-definition, and more.

## Prerequisites

### Install Language Servers

For full testing, you'll need language servers installed:

#### Rust (rust-analyzer)
```bash
# Via rustup (recommended)
rustup component add rust-analyzer

# Or via package manager
# macOS
brew install rust-analyzer

# Linux
# Download from https://github.com/rust-lang/rust-analyzer/releases
```

#### Python (pyright)
```bash
npm install -g pyright
```

#### TypeScript/JavaScript
```bash
npm install -g typescript-language-server typescript
```

#### Go
```bash
go install golang.org/x/tools/gopls@latest
```

## Features to Test

### 1. Diagnostics Display

**What it does:** Shows compiler errors and warnings in the editor with visual indicators and counts in the status bar.

**How to test:**
1. Build the editor: `cargo build --release`
2. Create a Rust file with errors:
   ```bash
   cat > test_diagnostics.rs << 'EOF'
   fn main() {
       let x = 5;
       let y: String = x;  // Type error
       println!("Hello {}", z);  // Undefined variable
   }
   EOF
   ```
3. Run the editor: `cargo run -- test_diagnostics.rs`
4. **Expected behavior:**
   - Lines with errors should show a red `●` marker in the gutter
   - Status bar should show error count (e.g., `● 2`)
   - rust-analyzer should start automatically in the background

**Keybindings to test:**
- `SPC l n` or `]d` - Jump to next diagnostic
- `SPC l p` or `[d` - Jump to previous diagnostic
  - Cursor should move to the diagnostic line
  - Status bar should show diagnostic message

**Known limitations:**
- Diagnostics update when LSP sends them (may take a few seconds)
- Color scheme may vary based on terminal

---

### 2. LSP Server Auto-Start

**What it does:** Automatically launches the appropriate language server when you open a file.

**How to test:**
1. Check logs: `tail -f /tmp/forgiven.log`
2. Open a Rust file: `cargo run -- src/main.rs`
3. **Expected behavior in logs:**
   ```
   INFO Spawning LSP server: rust-analyzer []
   INFO LSP server initialized successfully
   INFO LSP client initialized
   ```

**Test different languages:**
- Rust: `cargo run -- test.rs`
- Python: `cargo run -- test.py` (requires pyright installed)
- Go: `cargo run -- test.go` (requires gopls installed)

**Expected:** Different language servers should start for each file type.

---

### 3. Document Sync (did_change notifications)

**What it does:** Keeps the language server in sync with your edits in real-time.

**How to test:**
1. Open a Rust file with an error
2. Enter insert mode: press `i`
3. Fix the error by typing corrections
4. Wait 1-2 seconds
5. **Expected behavior:**
   - Diagnostics should update automatically
   - Error markers should disappear when fixed
   - New errors should appear as you type

**Test did_save:**
1. Make changes to a file
2. Save using `SPC f s` or `:w`
3. Check logs for "Sent LSP notification: textDocument/didSave"

---

### 4. Keybindings & Which-Key

**What it does:** Spacemacs-style leader key system with discoverable keybindings.

**How to test:**
1. In normal mode, press `SPC` (space)
2. Wait ~500ms
3. **Expected:** Which-key popup should appear showing options:
   ```
   Available keys:
   SPC b  buffer
   SPC f  file
   SPC l  lsp
   SPC q  quit
   ```

4. Press `l` to see LSP submenu:
   ```
   SPC l h  hover
   SPC l d  definition
   SPC l r  rename
   SPC l f  references
   SPC l s  symbols
   ```

**Test each LSP command:**
- `SPC l h` - Hover (shows "requested" message for now)
- `SPC l d` - Go to definition (shows "requested" message for now)
- `SPC l f` - Find references (shows "requested" message for now)
- `SPC l r` - Rename (shows "not yet implemented")
- `SPC l s` - Document symbols (shows "not yet implemented")

**Note:** Hover, definition, and references are stubbed for now. Full implementation requires async response handling in the editor loop.

---

### 5. Buffer Integration

**What it does:** Tracks LSP version and notifies server of all changes.

**How to test:**
1. Open a file: `cargo run -- test.rs`
2. Enter insert mode: `i`
3. Type some text
4. Check logs: `tail -f /tmp/forgiven.log`
5. **Expected in logs:**
   - Multiple "Sent LSP notification: textDocument/didChange" entries
   - Version number should increment with each change

---

## Integration Testing

### Full Workflow Test

1. **Create a new Rust file:**
   ```bash
   cat > integration_test.rs << 'EOF'
   fn main() {
       println!("Hello");
   }
   EOF
   ```

2. **Open in editor:**
   ```bash
   cargo run -- integration_test.rs
   ```

3. **Test sequence:**
   - File opens (LSP starts automatically)
   - No errors shown (clean file)
   - Press `i` to enter insert mode
   - Add an error: change `println!` to `printl!`
   - Exit insert mode: press `Esc`
   - Wait 2-3 seconds
   - **Expected:** Red `●` appears on that line, error count in status bar
   - Press `]d` to jump to diagnostic
   - **Expected:** Cursor on error line, message shows in status
   - Press `i` to fix it back to `println!`
   - Press `Esc`
   - Wait 2-3 seconds
   - **Expected:** Error disappears

---

## Troubleshooting

### Language Server Not Starting

**Symptoms:** No diagnostics appear, logs show "Failed to start language server"

**Solutions:**
1. Verify server is installed: `which rust-analyzer`
2. Check server works standalone: `rust-analyzer --version`
3. Check permissions: ensure server is executable
4. Try manual spawn: `rust-analyzer` (should not crash)

### Diagnostics Not Updating

**Symptoms:** Errors don't appear or don't update after fixes

**Possible causes:**
1. Language server not running (check logs)
2. File not saved (some servers only show diagnostics after save)
3. LSP initialization failed (check logs for errors)

**Debug steps:**
1. Check logs: `tail -f /tmp/forgiven.log`
2. Look for: "Diagnostics for <file>: X items"
3. If missing, LSP may not be sending diagnostics
4. Try saving the file: `SPC f s`

### Keybindings Not Working

**Symptoms:** Pressing `SPC l` does nothing

**Solutions:**
1. Ensure you're in Normal mode (press `Esc`)
2. Wait for which-key timeout (500ms)
3. Check if Mode indicator shows "NORMAL" in status bar

---

## Performance Notes

- **Startup:** Language servers may take 1-3 seconds to initialize
- **First diagnostic:** May appear after 2-4 seconds after file open
- **did_change notifications:** Sent on every keystroke in Insert mode
  - Future optimization: debounce to reduce frequency

---

## What's Working

✅ LSP server auto-start based on file extension
✅ Diagnostics collection from LSP
✅ Diagnostic display with gutter markers
✅ Diagnostic navigation (`]d`, `[d`)
✅ did_open, did_change, did_save notifications
✅ LSP document version tracking
✅ Which-key menu system
✅ LSP keybindings under `SPC l`

---

## What's Not Yet Implemented

⚠️ **Hover information** - Feature is stubbed, needs:
  - Async response handling in main loop
  - Popup rendering for hover content
  - Markdown formatting support

⚠️ **Go-to-definition** - Feature is stubbed, needs:
  - Async response handling
  - File switching logic
  - Jump list for back navigation

⚠️ **Find references** - Feature is stubbed
⚠️ **Rename** - Not yet implemented
⚠️ **Document symbols** - Not yet implemented
⚠️ **Autocompletion** - Not yet implemented

---

## Next Steps for Full LSP Support

To complete hover, definition, and other features:

1. **Add LSP response queue in Editor:**
   ```rust
   pending_lsp_requests: HashMap<RequestId, LspRequestType>
   ```

2. **Poll for responses in main loop:**
   ```rust
   // In run() loop
   for (id, request_type) in pending_requests {
       if let Ok(response) = receiver.try_recv() {
           // Handle response based on type
       }
   }
   ```

3. **Implement hover popup:**
   - Create popup widget in UI module
   - Render markdown content
   - Position near cursor

4. **Implement definition jump:**
   - Parse location response
   - If same file: move cursor
   - If different file: open file and move cursor
   - Add to jump stack

---

## Logging

All LSP activity is logged to `/tmp/forgiven.log`:

```bash
# Watch logs in real-time
tail -f /tmp/forgiven.log

# Filter for LSP messages
tail -f /tmp/forgiven.log | grep LSP

# Filter for diagnostics
tail -f /tmp/forgiven.log | grep Diagnostics
```

**Useful log messages:**
- `Spawning LSP server` - Server starting
- `LSP server initialized` - Server ready
- `Diagnostics for <uri>: X items` - Diagnostics received
- `Sent LSP notification: textDocument/didChange` - Document updated

---

## Success Criteria

Phase 3 is successful when:

- ✅ Language servers start automatically for supported languages
- ✅ Diagnostics appear in the editor within 5 seconds of opening a file
- ✅ Diagnostic markers are clearly visible
- ✅ Diagnostic navigation works (]d / [d)
- ✅ Document changes are synced to LSP (in logs)
- ✅ Status bar shows error/warning counts
- ✅ Which-key menu appears and shows LSP commands
- ✅ No crashes when LSP server is missing (graceful degradation)

---

## Known Issues

1. **did_change frequency:** Currently sent on every keystroke, may cause performance issues with large files
2. **Hover/Definition stubbed:** Need async response handling implementation
3. **Terminal color support:** Some terminals may not render diagnostic markers correctly
4. **Workspace root detection:** Currently uses file's parent directory, should detect project root (Cargo.toml, etc.)

---

## Feedback & Bug Reports

When reporting issues:
1. Include relevant logs from `/tmp/forgiven.log`
2. Specify which language server you're using
3. Include steps to reproduce
4. Note your terminal/OS version

---

**Status:** Phase 3 Core Features Complete ✅  
**Date:** February 22, 2026  
**Next Phase:** GitHub Copilot Integration (Phase 4)
