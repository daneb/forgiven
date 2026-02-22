# Phase 2 Testing Guide — Spacemacs-Inspired UX

## 🎉 Phase 2 Complete!

We've implemented the Spacemacs-inspired interface with discoverable keybindings, leader keys, which-key popups, visual mode, and buffer management!

## New Features to Test

### 1. Leader Key System (SPC)

Press **Space** in Normal mode to enter leader key mode. The status bar will show "SPC" and after 500ms, a which-key popup will appear showing available commands.

**Buffer Commands** (SPC b):
- `SPC b b` - List buffers (interactive picker)
- `SPC b n` - Next buffer
- `SPC b p` - Previous buffer
- `SPC b d` - Delete/close current buffer

**File Commands** (SPC f):
- `SPC f s` - Save file (same as :w)
- `SPC f f` - Find file (placeholder for now)

**Quit Commands** (SPC q):
- `SPC q q` - Quit (checks for unsaved changes)

### 2. Visual Mode

Press **v** in Normal mode to enter Visual mode.
- Move with h/j/k/l or arrows to extend selection
- Selected text is highlighted
- Press **Esc** to exit Visual mode

### 3. Buffer Management

**Buffer List (SPC b b)**:
- Shows all open buffers in a centered picker
- Use j/k or ↑/↓ to navigate
- Press Enter to switch to selected buffer
- Press Esc to the cancel

**Quick Buffer Switching**:
- `SPC b n` - Cycle to next buffer
- `SPC b p` - Cycle to previous buffer
- Modified buffers show [+] indicator

### 4. Which-Key Popup

After pressing Space, wait ~500ms and a popup will appear showing:
- Available key sequences
- Description of each command
- Auto-dismisses when you continue typing

### 5. Enhanced Movement

**Word Movement** (vim-style):
- `w` - Move forward by word
- `b` - Move backward by word

### 6. Enhanced Status Line

The status line now shows:
- Current mode with color coding:
  - NORMAL (blue)
  - INSERT (green)
  - VISUAL (magenta)
  - COMMAND (yellow)
  - PICK (cyan) - for buffer picker
- Current key sequence (e.g., "SPC b")
- Buffer name and modification status
- Cursor position

## Testing Workflow

### Test 1: Leader Keys and Which-Key

```
1. Start the editor: ./target/release/forgiven test.txt
2. Press Space (don't press anything else)
3. Wait ~500ms - which-key popup should appear
4. Press 'b' - popup updates to show buffer commands
5. Press 'b' again - buffer picker appears
6. Press Esc - return to normal mode
```

### Test 2: Multiple Buffers

```
1. Open multiple files: ./target/release/forgiven file1.txt file2.txt file3.txt
2. Press Space → b → b (SPC b b) - buffer picker shows all files
3. Navigate with j/k
4. Press Enter to switch
5. Try SPC b n / SPC b p to cycle through buffers
6. Edit one file (add text)
7. Try SPC b d - should warn about unsaved changes
8. Save with SPC f s
9. Try SPC b d again - buffer closes
```

### Test 3: Visual Mode

```
1. Open any file with text
2. Position cursor at start of a word
3. Press 'v' - enter Visual mode (magenta status)
4. Use h/j/k/l to extend selection
5. Watch text highlight as you move
6. Press Esc - selection clears, back to Normal mode
```

### Test 4: Word Movement

```
1. Type some text: "Hello world this is a test"
2. Press Esc (Normal mode)
3. Move to start with '0'
4. Press 'w' repeatedly - cursor jumps word by word
5. Press 'b' repeatedly - cursor moves backward by word
```

### Test 5: Combined Workflow

```
1. Open multiple files
2. Use visual mode to select text in first file
3. Switch buffers with SPC b b
4. Make edits in multiple buffers
5. Save all with SPC f s (switch and save each)
6. Close buffers with SPC b d
7. Quit with SPC q q
```

## Known Limitations (Phase 2)

- File finder (SPC f f) is not yet implemented
- Visual mode doesn't support copy/paste yet
- No multi-line selection operations
- Which-key popup is basic (no icons, limited styling)

## What's Next (Phase 3)

- LSP integration for diagnostics and autocomplete
- Syntax highlighting with tree-sitter
- Go-to-definition
- Code navigation

## Keybinding Reference

### Normal Mode

**Insert**:
- `i` - insert at cursor
- `a` - append after cursor
- `I` - insert at line start
- `A` - append at line end
- `o` - open line below
- `O` - open line above

**Movement**:
- `h/j/k/l` or arrows - basic movement
- `w` - word forward
- `b` - word backward
- `0` or Home - line start
- `$` or End - line end

**Modes**:
- `v` - visual mode
- `:` - command mode
- `Space` - leader key

**Leader Keys** (Space):
- `b b` - buffer list
- `b n` - next buffer
- `b p` - previous buffer
- `b d` - delete buffer
- `f s` - save file
- `f f` - find file (TODO)
- `q q` - quit

### Insert Mode

- Type normally
- Arrow keys for movement
- Backspace/Delete
- Esc - return to Normal mode

### Visual Mode

- `h/j/k/l` or arrows - extend selection
- Esc - clear selection, return to Normal

### Command Mode

- `:w` - save
- `:q` - quit
- `:wq` - save and quit
- `:q!` - force quit
- Esc - cancel

Enjoy the new Spacemacs-inspired interface! 🚀
