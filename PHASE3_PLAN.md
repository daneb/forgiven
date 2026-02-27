# Phase 3: Language Server Integration — Progress & Plan

## Overview
Phase 3 goal: Add IDE-like features through LSP integration (diagnostics, hover, go-to-definition, autocomplete, symbol navigation, basic refactoring).

---

## ✅ Completed So Far

### 1. LSP Client Architecture (DONE)
- ✅ Created async LSP client in `src/lsp/mod.rs`
- ✅ Implemented request/response handling with oneshot channels
- ✅ Added notification system for diagnostics
- ✅ Proper error handling and connection management
- ✅ Support for multiple language servers via `LspManager`
- ✅ Helper methods: `path_to_uri()`, `language_from_path()`

### 2. Core LSP Methods (DONE)
- ✅ `did_open()` - Notify server when file opens
- ✅ `did_change()` - Notify server of content changes  
- ✅ `did_save()` - Notify server when file saves
- ✅ `hover()` - Request hover information (returns oneshot receiver)
- ✅ `goto_definition()` - Request definition location
- ✅ `completion()` - Request completions
- ✅ `references()` - Find references
- ✅ `rename()` - Rename symbol
- ✅ `document_symbols()` - Get document outline

### 3. Diagnostics Storage (DONE)
- ✅ `LspManager` stores diagnostics in Arc<RwLock<HashMap>>
- ✅ Diagnostics are collected from LSP notifications
- ✅ `get_diagnostics(uri)` method to fetch for specific file
- ✅ `get_all_diagnostics()` method

### 4. Editor Integration (PARTIAL)
- ✅ Added `LspManager` field to `Editor` struct
- ✅ Added `current_diagnostics` field to `Editor`
- ✅ Main loop processes LSP messages
- ✅ Main loop updates diagnostics for current buffer
- ✅ `did_open` sent when files are opened
- ✅ `did_save` sent when files are saved
- ⚠️ NOT YET: `did_change` notifications on buffer edits
- ⚠️ NOT YET: Language server spawning/initialization

---

## 🔨 TODO: Remaining Phase 3 Work

### Step 1: Complete Diagnostics Display
**Goal:** Show errors/warnings in the editor UI

**Tasks:**
- [ ] Update UI module to accept diagnostics parameter
- [ ] Render diagnostics in gutter (line numbers with indicators)
- [ ] Render diagnostics inline or in separate panel
- [ ] Add keybinding to jump to next/prev diagnostic (e.g., `]d`, `[d`)
- [ ] Show diagnostic count in status line

**Files to modify:**
- `src/ui/mod.rs` - Add diagnostics rendering
- `src/editor/mod.rs` - Pass diagnostics to UI
- `src/keymap/mod.rs` - Add diagnostic navigation actions

---

### Step 2: Hover Information
**Goal:** Show type info and documentation on hover/command

**Tasks:**
- [ ] Add keybinding for hover (e.g., `SPC l h` or `K`)
- [ ] Request hover from LSP client
- [ ] Display hover content in popup/overlay
- [ ] Handle markdown formatting in hover text
- [ ] Timeout and error handling

**Files to modify:**
- `src/keymap/mod.rs` - Add hover action
- `src/editor/mod.rs` - Handle hover requests/responses
- `src/ui/mod.rs` - Render hover popup

---

### Step 3: Go-to-Definition
**Goal:** Navigate to symbol definitions

**Tasks:**
- [ ] Add keybinding for goto-definition (e.g., `g d` or `SPC l d`)
- [ ] Request definition location from LSP
- [ ] Parse response (could be single location, multiple, or links)
- [ ] Open file at target location if different file
- [ ] Jump to location in current file
- [ ] Maintain jump list for going back (`Ctrl-O`)

**Files to modify:**
- `src/keymap/mod.rs` - Add goto-definition action
- `src/editor/mod.rs` - Handle goto requests, file jumping
- `src/buffer/mod.rs` - May need cursor position helpers

---

### Step 4: Autocomplete from LSP
**Goal:** Show completion suggestions while typing

**Tasks:**
- [ ] Detect when to trigger completion (after `.`, `::`, etc.)
- [ ] Request completions from LSP at cursor position
- [ ] Display completion menu (similar to buffer picker)
- [ ] Navigate completion items with `j/k` or arrows
- [ ] Apply selected completion
- [ ] Handle additional text edits from completion

**Files to modify:**
- `src/editor/mod.rs` - Completion trigger, request/response
- `src/ui/mod.rs` - Completion menu widget
- `src/keymap/mod.rs` - Completion navigation keys

---

### Step 5: Symbol Navigation
**Goal:** Browse document outline and workspace symbols

**Tasks:**
- [ ] Add keybinding for document symbols (e.g., `SPC l s`)
- [ ] Request document symbols from LSP
- [ ] Display symbols in picker (similar to buffer picker)
- [ ] Jump to selected symbol
- [ ] Add workspace symbol search (future)

**Files to modify:**
- `src/keymap/mod.rs` - Symbol navigation actions
- `src/editor/mod.rs` - Symbol requests, picker mode
- `src/ui/mod.rs` - Symbol picker rendering

---

### Step 6: Basic Refactoring (Rename)
**Goal:** Rename symbols across project

**Tasks:**
- [ ] Add keybinding for rename (e.g., `SPC l r`)
- [ ] Prompt user for new name
- [ ] Request rename from LSP
- [ ] Apply workspace edit (multiple files)
- [ ] Show preview before applying (optional)

**Files to modify:**
- `src/keymap/mod.rs` - Rename action
- `src/editor/mod.rs` - Rename requests, workspace edits
- `src/ui/mod.rs` - Rename prompt

---

### Step 7: LSP Server Configuration & Spawning
**Goal:** Automatically start language servers for opened files

**Tasks:**
- [ ] Create language server config file or struct
- [ ] Define server commands per language (rust-analyzer, pyright, etc.)
- [ ] Spawn server when first file of language is opened
- [ ] Initialize server with workspace root
- [ ] Handle server failures gracefully
- [ ] Add command to restart LSP server

**Files to create/modify:**
- `src/lsp/config.rs` - Server configuration
- `src/editor/mod.rs` - Auto-spawn logic

---

### Step 8: Send did_change Notifications
**Goal:** Keep LSP in sync with buffer edits

**Tasks:**
- [ ] Track document version in buffer
- [ ] Send `did_change` after insertions/deletions
- [ ] Batch changes or debounce for performance
- [ ] Handle full document sync vs incremental

**Files to modify:**
- `src/buffer/buffer.rs` - Add version tracking
- `src/editor/mod.rs` - Send did_change after edits

---

### Step 9: Keybindings & Commands
**Goal:** Spacemacs-style LSP commands under `SPC l` prefix

**Keybinding plan:**
- `SPC l h` - Hover/documentation
- `SPC l d` - Go to definition
- `SPC l r` - Rename
- `SPC l s` - Document symbols
- `SPC l f` - Find references
- `SPC l a` - Code actions (future)
- `]d` / `[d` - Next/prev diagnostic

**Files to modify:**
- `src/keymap/mod.rs` - Add LSP action enum variants
- `src/keymap/mod.rs` - Add leader key submenu for LSP

---

### Step 10: Documentation & Testing
**Goal:** Create Phase 3 testing guide

**Tasks:**
- [ ] Create `PHASE3_TESTING.md`
- [ ] Document how to install language servers
- [ ] Test with rust-analyzer
- [ ] Test diagnostics display
- [ ] Test hover, goto-definition
- [ ] Test symbol navigation
- [ ] Document known issues/limitations

---

## 🏗️ Architecture Improvements Needed

### Refactor Monolithic Files

**Current issue:** `src/editor/mod.rs` is 656+ lines and growing

**Proposed structure:**
```
src/editor/
  mod.rs           # Public API, Editor struct definition
  render.rs        # Rendering logic
  input.rs         # Key handling, action execution
  file_ops.rs      # File opening, saving, scanning
  lsp_integration.rs  # LSP-specific editor methods
  picker.rs        # Buffer/file picker modes
```

**Also consider:**
```
src/lsp/
  mod.rs           # Public API, LspManager
  client.rs        # LspClient implementation
  config.rs        # Language server configuration
  handlers.rs      # Notification/request handlers
```

---

## 📊 Estimated Completion

### Critical Path (MVP):
1. ✅ Diagnostics storage (DONE)
2. 🔨 Diagnostics display (2-3 hours)
3. 🔨 Hover information (1-2 hours)
4. 🔨 Go-to-definition (2-3 hours)
5. 🔨 LSP server spawning (1 hour)
6. 🔨 did_change notifications (1 hour)

**Total remaining: ~8-12 hours of work**

### Nice-to-have (can defer):
- Autocomplete UI (complex, can do in Phase 4/5)
- Symbol navigation (useful but not critical)
- Rename refactoring (useful but not critical)

---

## 🚀 Recommended Next Steps

1. **Reorganize code first** - Break up editor/mod.rs and lsp/mod.rs
2. **Complete diagnostics display** - Most visible feature
3. **Add hover + goto-definition** - Most useful for developers
4. **Configure and spawn rust-analyzer** - Make it actually work
5. **Test thoroughly** - Write PHASE3_TESTING.md
6. **Move to Phase 4** - GitHub Copilot integration

---

## 📝 Notes

- Current LSP implementation is solid foundation, well-architected
- Async/await integration is clean
- Borrow checker issues resolved
- Ready to build features on top of this base

**Status:** Phase 3 is ~30-40% complete. Core infrastructure done, features need implementation.
