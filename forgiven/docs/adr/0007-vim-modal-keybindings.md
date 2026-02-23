# ADR 0007 — Vim-style Modal Editing and Spacemacs Leader Keys

**Date:** 2026-02-23
**Status:** Accepted

---

## Context

forgiven targets developers already comfortable with keyboard-driven editors. Two
dominant keybinding philosophies exist in that space:

- **Vim** — modal editing (Normal / Insert / Visual / Command)
- **Spacemacs/Doom Emacs** — mnemonic leader-key trees (`SPC b b`, `SPC f f`, etc.)

The editor needs a keybinding system that:
- Routes keys differently depending on current mode
- Supports multi-key sequences (leader + category + action)
- Is extensible without a combinatorial match table
- Shows available keys after a short pause (which-key)

## Decision

Implement **Vim-style modal editing** for text manipulation, combined with a
**Spacemacs-inspired `SPC`-prefixed leader tree** for editor commands.

### Modes

```rust
pub enum Mode {
    Normal,      // default; motion + command entry
    Insert,      // text entry
    Visual,      // selection
    Command,     // : command line
    PickBuffer,  // buffer switcher overlay
    PickFile,    // file finder overlay
    Agent,       // Copilot Chat panel focused
}
```

### Leader tree

The `SPC` key in Normal mode starts a leader sequence. A recursive `HashMap<char, KeyNode>`
tree resolves multi-key sequences to `Action` variants:

```
SPC b b  →  BufferList
SPC b n  →  BufferNext
SPC b p  →  BufferPrevious
SPC b d  →  BufferClose
SPC f f  →  FileFind
SPC f s  →  FileSave
SPC q q  →  Quit
SPC l h  →  LspHover
SPC l d  →  LspGoToDefinition
SPC l r  →  LspRename
SPC l f  →  LspReferences
SPC l s  →  LspDocumentSymbols
SPC a a  →  AgentToggle
SPC a f  →  AgentFocus
```

### Which-key

After 500 ms of holding a partial sequence the `show_which_key` flag is set and the
renderer draws a popup listing available next-key options with descriptions.
Typing the next character immediately hides the popup and resolves the sequence.

### KeyHandler

`KeyHandler` is a pure state machine:
- `handle_normal(key: KeyEvent) -> Action` — returns an `Action` for the editor to execute
- Sequence state (`Vec<char>`, `Option<Instant>`) is entirely internal
- `clear_sequence()` resets state on Esc, completion, or invalid input

The editor match-dispatches on `Action` variants in a single place, making it easy to
add or remap actions without touching the keyhandler.

## Consequences

- Insert mode key handling is done directly in `handle_insert_mode()` in
  `editor/mod.rs` (crossterm `KeyCode` match) rather than through `KeyHandler`,
  because insert mode has fewer combinatorial bindings and needs direct access to
  editor state (e.g. triggering LSP `didChange`)
- Visual mode currently only records the selection anchor; operations on the selection
  (yank, delete, etc.) are not yet implemented
- The `]d` / `[d` diagnostic navigation bindings are stubs — they are recognized as
  prefixes but the second key handler is not yet wired
- Agent mode key routing is handled separately in `handle_agent_mode()` rather than
  through `KeyHandler`, keeping agent-specific logic self-contained
- No support for user-remapping at runtime; keybindings are compile-time constants
  (a TOML keybinding config layer is a planned future addition)
