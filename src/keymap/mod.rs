use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Editor modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Visual,          // character-wise visual selection (v)
    VisualLine,      // line-wise visual selection (V)
    PickBuffer,      // For buffer selection UI
    PickFile,        // For file finder UI
    Agent,           // Copilot Chat / agent panel focused
    Explorer,        // File explorer tree focused
    MarkdownPreview, // Read-only rendered markdown view (SPC m p toggle)
    Search,          // Project-wide ripgrep search overlay (SPC s g)
    InFileSearch,    // In-file search mode (/)
    RenameFile,      // Rename popup: user edits a filename from the explorer
    DeleteFile,      // Confirmation popup: y=delete, n/Esc=cancel
    NewFolder,       // New folder popup: user types a folder name from the explorer
    ApplyDiff,       // Full-screen diff preview before applying agent code block
    CommitMsg,       // Editable commit message popup (SPC g s / SPC g l)
    ReleaseNotes,    // Release notes generation popup (SPC g n)
    Diagnostics,     // Read-only diagnostics overlay (SPC d)
    BinaryFile,      // Unsupported binary file popup: o=open default app, Esc=dismiss
}

/// An editor action to be executed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Noop,
    Insert,
    InsertAppend,
    InsertLineStart,
    InsertLineEnd,
    InsertNewlineBelow,
    InsertNewlineAbove,
    // Normal-mode movement (no line-wrap for h/l)
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveFirstNonBlank, // ^ — first non-whitespace char on the line
    #[allow(dead_code)]
    MoveLineEnd, // A / InsertLineEnd motion (past last char)
    MoveLineEndNormal, // $ in Normal mode (lands ON last char)
    MoveWordForward,
    MoveWordBackward,
    // Navigation
    GotoFileTop,    // gg
    GotoFileBottom, // G
    Command,
    Visual,
    // Edit operations
    DeleteChar,      // x — delete char at cursor
    DeleteLine,      // dd — delete current line into clipboard
    DeleteToLineEnd, // D  — delete from cursor to EOL
    DeleteWord,      // dw — delete from cursor to end of word
    DeleteToChar {
        ch: char,
        inclusive: bool,
    }, // dt{c}/df{c}
    YankToChar {
        ch: char,
        inclusive: bool,
    }, // yt{c}/yf{c}
    ChangeToChar {
        ch: char,
        inclusive: bool,
    }, // ct{c}/cf{c}
    FindCharForward {
        ch: char,
        inclusive: bool,
    }, // f{c}/t{c}
    FindCharBackward {
        ch: char,
        inclusive: bool,
    }, // F{c}/T{c}
    YankLine,        // yy — yank whole line
    YankWord,        // yw — yank to end of word
    YankToLineEnd,   // y$ — yank to end of line
    YankSelection,   // y in Visual mode — yank selection
    DeleteSelection, // d/x in Visual mode — delete selection into clipboard
    ChangeLine,      // cc — delete line + enter Insert
    ChangeWord,      // cw — delete word + enter Insert
    PasteAfter,      // p
    PasteBefore,     // P
    Undo,            // u
    Redo,            // Ctrl+R
    // Leader key actions
    BufferList,
    BufferNext,
    BufferPrevious,
    BufferClose,
    BufferForceClose,
    FileFind,
    FileNew,
    FileSave,
    FileEditConfig,
    Quit,
    // LSP actions
    LspHover,
    LspGoToDefinition,
    LspReferences,
    LspRename,
    LspDocumentSymbols,
    #[allow(dead_code)]
    LspNextDiagnostic,
    #[allow(dead_code)]
    LspPrevDiagnostic,
    // Visual modes
    VisualLine,
    // Agent panel
    AgentToggle,
    AgentFocus,
    // Explorer panel
    ExplorerToggle,
    ExplorerFocus,
    ExplorerToggleHidden,
    // Git
    GitOpen,         // SPC g g — open lazygit
    GitCommitStaged, // SPC g s — generate commit msg from staged diff
    GitCommitLast,   // SPC g l — generate commit msg from last commit
    GitReleaseNotes, // SPC g n — generate release notes from last N commits
    // Markdown preview
    MarkdownPreviewToggle, // SPC m p — toggle markdown preview for .md buffers
    MarkdownOpenBrowser,   // SPC m b — render current buffer to HTML and open in browser
    // Project-wide text search
    SearchOpen, // SPC s g — open the project search overlay
    // In-file search
    InFileSearchStart, // / — start search in current buffer
    InFileSearchNext,  // n — jump to next match
    InFileSearchPrev,  // N — jump to previous match
    // Window / split
    WindowSplit,     // SPC w v — open vertical split
    WindowFocusNext, // SPC w w — cycle focus between panes
    WindowClose,     // SPC w c — close split
    // Diagnostics
    DiagnosticsOpen,    // SPC d d — open diagnostics overlay
    DiagnosticsOpenLog, // SPC d l — open /tmp/forgiven.log in editor
}

/// Represents a keybinding tree node
#[derive(Debug, Clone)]
pub struct KeyNode {
    pub description: String,
    pub action: Option<Action>,
    pub children: BTreeMap<char, KeyNode>,
}

impl KeyNode {
    fn new(desc: impl Into<String>) -> Self {
        Self { description: desc.into(), action: None, children: BTreeMap::new() }
    }

    fn leaf(desc: impl Into<String>, action: Action) -> Self {
        Self { description: desc.into(), action: Some(action), children: BTreeMap::new() }
    }
}

/// Handles key events and maps them to editor actions with leader key support
pub struct KeyHandler {
    /// Current leader key sequence being built (SPC …)
    sequence: Vec<char>,
    /// When the sequence started (for which-key timeout)
    sequence_start: Option<Instant>,
    /// Leader key bindings tree
    leader_tree: BTreeMap<char, KeyNode>,
    /// Which-key popup should be shown
    show_which_key: bool,
    /// Pending prefix key for two-key Normal-mode commands (d, g, y, c …)
    pending_key: Option<char>,
    /// Second pending key for three-key sequences (e.g. `t` in `dt"`)
    pending_second_key: Option<char>,
    /// Accumulated numeric count prefix (e.g. `3` before `dd` → delete 3 lines).
    pending_count: Option<usize>,
}

impl KeyHandler {
    pub fn new() -> Self {
        let leader_tree = Self::build_leader_tree();
        Self {
            sequence: Vec::new(),
            sequence_start: None,
            leader_tree,
            show_which_key: false,
            pending_key: None,
            pending_second_key: None,
            pending_count: None,
        }
    }

    /// Consume and return the pending count, defaulting to 1.
    /// Should be called once at the start of `execute_action`.
    pub fn take_count(&mut self) -> usize {
        self.pending_count.take().unwrap_or(1)
    }

    /// Build the Spacemacs-inspired leader key tree
    fn build_leader_tree() -> BTreeMap<char, KeyNode> {
        let mut tree = BTreeMap::new();

        // SPC b - Buffer commands
        let mut buffer_node = KeyNode::new("buffer");
        buffer_node.children.insert('b', KeyNode::leaf("list buffers", Action::BufferList));
        buffer_node.children.insert('n', KeyNode::leaf("next buffer", Action::BufferNext));
        buffer_node.children.insert('p', KeyNode::leaf("previous buffer", Action::BufferPrevious));
        buffer_node.children.insert('d', KeyNode::leaf("delete buffer", Action::BufferClose));
        buffer_node
            .children
            .insert('D', KeyNode::leaf("delete buffer (discard)", Action::BufferForceClose));
        tree.insert('b', buffer_node);

        // SPC f - File commands
        let mut file_node = KeyNode::new("file");
        file_node.children.insert('f', KeyNode::leaf("find file", Action::FileFind));
        file_node.children.insert('n', KeyNode::leaf("new file", Action::FileNew));
        file_node.children.insert('s', KeyNode::leaf("save file", Action::FileSave));
        file_node.children.insert('e', KeyNode::leaf("edit config", Action::FileEditConfig));
        tree.insert('f', file_node);

        // SPC q - Quit commands
        let mut quit_node = KeyNode::new("quit");
        quit_node.children.insert('q', KeyNode::leaf("quit", Action::Quit));
        tree.insert('q', quit_node);

        // SPC l - LSP commands
        let mut lsp_node = KeyNode::new("lsp");
        lsp_node.children.insert('h', KeyNode::leaf("hover", Action::LspHover));
        lsp_node.children.insert('d', KeyNode::leaf("definition", Action::LspGoToDefinition));
        lsp_node.children.insert('r', KeyNode::leaf("rename", Action::LspRename));
        lsp_node.children.insert('f', KeyNode::leaf("references", Action::LspReferences));
        lsp_node.children.insert('s', KeyNode::leaf("symbols", Action::LspDocumentSymbols));
        tree.insert('l', lsp_node);

        // SPC a - Agent / Copilot Chat panel
        let mut agent_node = KeyNode::new("agent");
        agent_node.children.insert('a', KeyNode::leaf("toggle agent panel", Action::AgentToggle));
        agent_node.children.insert('f', KeyNode::leaf("focus agent panel", Action::AgentFocus));
        tree.insert('a', agent_node);

        // SPC e - Explorer / file tree
        let mut explorer_node = KeyNode::new("explorer");
        explorer_node
            .children
            .insert('e', KeyNode::leaf("toggle file explorer", Action::ExplorerToggle));
        explorer_node
            .children
            .insert('f', KeyNode::leaf("focus file explorer", Action::ExplorerFocus));
        explorer_node
            .children
            .insert('h', KeyNode::leaf("toggle hidden files", Action::ExplorerToggleHidden));
        tree.insert('e', explorer_node);

        // SPC g - Git
        let mut git_node = KeyNode::new("git");
        git_node.children.insert('g', KeyNode::leaf("open lazygit", Action::GitOpen));
        git_node
            .children
            .insert('s', KeyNode::leaf("commit msg from staged", Action::GitCommitStaged));
        git_node
            .children
            .insert('l', KeyNode::leaf("commit msg from last commit", Action::GitCommitLast));
        git_node.children.insert(
            'n',
            KeyNode::leaf("release notes from last N commits", Action::GitReleaseNotes),
        );
        tree.insert('g', git_node);

        // SPC m - Markdown
        let mut md_node = KeyNode::new("markdown");
        md_node
            .children
            .insert('p', KeyNode::leaf("toggle markdown preview", Action::MarkdownPreviewToggle));
        md_node.children.insert('b', KeyNode::leaf("open in browser", Action::MarkdownOpenBrowser));
        tree.insert('m', md_node);

        // SPC s - Search
        let mut search_node = KeyNode::new("search");
        search_node
            .children
            .insert('g', KeyNode::leaf("search in project (ripgrep)", Action::SearchOpen));
        tree.insert('s', search_node);

        // SPC w - Window / split
        let mut window_node = KeyNode::new("window");
        window_node.children.insert('v', KeyNode::leaf("vertical split", Action::WindowSplit));
        window_node.children.insert('w', KeyNode::leaf("focus next pane", Action::WindowFocusNext));
        window_node.children.insert('c', KeyNode::leaf("close split", Action::WindowClose));
        tree.insert('w', window_node);

        // SPC d - Diagnostics
        let mut diag_node = KeyNode::new("diagnostics");
        diag_node
            .children
            .insert('d', KeyNode::leaf("diagnostics overlay", Action::DiagnosticsOpen));
        diag_node.children.insert('l', KeyNode::leaf("open log file", Action::DiagnosticsOpenLog));
        tree.insert('d', diag_node);

        tree
    }

    /// Get the current key sequence (for display in status line).
    /// Shows accumulated count + pending key/leader so the user has feedback.
    pub fn sequence(&self) -> String {
        let count_prefix = self.pending_count.map(|n| n.to_string()).unwrap_or_default();
        if let Some(pk) = self.pending_key {
            if let Some(sk) = self.pending_second_key {
                return format!("{}{}{}", count_prefix, pk, sk);
            }
            return format!("{}{}", count_prefix, pk);
        }
        if self.sequence.is_empty() {
            count_prefix
        } else {
            format!("SPC {}", self.sequence.iter().collect::<String>())
        }
    }

    /// Returns true when a leader sequence is in-flight and the which-key
    /// popup has not yet been shown — used by the event loop to force a render
    /// tick so the popup appears exactly when the 500 ms timer fires, without
    /// waiting for the next key event.
    pub fn which_key_pending(&self) -> bool {
        self.sequence_start.is_some() && !self.show_which_key
    }

    /// Check if which-key should be displayed (also arms the flag on first call
    /// after the 500 ms delay).
    pub fn should_show_which_key(&mut self) -> bool {
        if let Some(start) = self.sequence_start {
            if start.elapsed() > Duration::from_millis(500) && !self.show_which_key {
                self.show_which_key = true;
                return true;
            }
        }
        self.show_which_key
    }

    /// Get which-key options for current sequence
    pub fn which_key_options(&self) -> Vec<(String, String)> {
        if self.sequence.is_empty() {
            // Show top-level leader options
            self.leader_tree
                .iter()
                .map(|(k, node)| (format!("SPC {}", k), node.description.clone()))
                .collect()
        } else {
            // Navigate to current position in tree
            let mut temp_node: Option<&KeyNode> = None;

            for (i, &ch) in self.sequence.iter().enumerate() {
                if i == 0 {
                    temp_node = self.leader_tree.get(&ch);
                } else if let Some(node) = temp_node {
                    temp_node = node.children.get(&ch);
                } else {
                    return Vec::new();
                }
            }

            if let Some(node) = temp_node {
                node.children
                    .iter()
                    .map(|(k, child)| {
                        let key_seq =
                            format!("SPC {}{}", self.sequence.iter().collect::<String>(), k);
                        (key_seq, child.description.clone())
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
    }

    /// Clear the current sequence and any accumulated count.
    pub fn clear_sequence(&mut self) {
        self.sequence.clear();
        self.sequence_start = None;
        self.show_which_key = false;
        self.pending_count = None;
    }

    /// Handle a key in Normal mode, returning an action
    pub fn handle_normal(&mut self, key: KeyEvent) -> Action {
        // ── Numeric count prefix (e.g. 3 in 3dd / 5j) ────────────────────────
        // `0` alone = MoveLineStart; `0` after digits = part of count.
        if let KeyCode::Char(ch) = key.code {
            if ch.is_ascii_digit() && (ch != '0' || self.pending_count.is_some()) {
                let digit = (ch as usize) - ('0' as usize);
                self.pending_count = Some(self.pending_count.unwrap_or(0) * 10 + digit);
                return Action::Noop; // accumulating — don't act yet
            }
        }

        // ── Resolve three-key sequences (dt/df + char, yt/yf, ct/cf) ─────────
        if self.pending_second_key.is_some() {
            let pk = self.pending_key.take().unwrap_or(' ');
            let sk = self.pending_second_key.take().unwrap_or(' ');
            if let KeyCode::Char(ch) = key.code {
                return match (pk, sk) {
                    ('d', 't') => Action::DeleteToChar { ch, inclusive: false },
                    ('d', 'f') => Action::DeleteToChar { ch, inclusive: true },
                    ('y', 't') => Action::YankToChar { ch, inclusive: false },
                    ('y', 'f') => Action::YankToChar { ch, inclusive: true },
                    ('c', 't') => Action::ChangeToChar { ch, inclusive: false },
                    ('c', 'f') => Action::ChangeToChar { ch, inclusive: true },
                    _ => Action::Noop,
                };
            }
            return Action::Noop; // non-char key cancels
        }

        // ── Resolve pending double-key prefixes (dd, gg, yy, ft, …) ──────────
        if let Some(pk) = self.pending_key.take() {
            if let KeyCode::Char(ch) = key.code {
                // Check if this needs a third key (char argument)
                if matches!((pk, ch), ('d' | 'y' | 'c', 'f' | 't')) {
                    self.pending_key = Some(pk);
                    self.pending_second_key = Some(ch);
                    return Action::Noop;
                }
                return match (pk, ch) {
                    // d — delete into clipboard
                    ('d', 'd') => Action::DeleteLine,
                    ('d', 'w') => Action::DeleteWord,
                    ('d', '$') => Action::DeleteToLineEnd,
                    // g — goto
                    ('g', 'g') => Action::GotoFileTop,
                    // y — yank into clipboard
                    ('y', 'y') => Action::YankLine,
                    ('y', 'w') => Action::YankWord,
                    ('y', '$') => Action::YankToLineEnd,
                    // c — change (delete + Insert)
                    ('c', 'c') => Action::ChangeLine,
                    ('c', 'w') => Action::ChangeWord,
                    ('c', '$') => Action::DeleteToLineEnd, // same as D, then insert
                    // f/t/F/T — find char (standalone)
                    ('f', _) => Action::FindCharForward { ch, inclusive: true },
                    ('t', _) => Action::FindCharForward { ch, inclusive: false },
                    ('F', _) => Action::FindCharBackward { ch, inclusive: true },
                    ('T', _) => Action::FindCharBackward { ch, inclusive: false },
                    _ => Action::Noop, // unknown combo — discard
                };
            }
            // Non-char key after a prefix — cancel
            return Action::Noop;
        }

        // ── Leader key sequence (SPC …) ───────────────────────────────────────
        if self.sequence_start.is_some() {
            if let KeyCode::Char(ch) = key.code {
                self.sequence.push(ch);
                self.show_which_key = false;
                return self.resolve_leader_sequence();
            } else if key.code == KeyCode::Esc {
                self.clear_sequence();
                return Action::Noop;
            }
        }

        // Check for leader key (Space)
        if let KeyCode::Char(' ') = key.code {
            self.sequence.clear();
            self.sequence_start = Some(Instant::now());
            self.show_which_key = false;
            return Action::Noop;
        }

        // ── Direct Normal-mode key bindings ───────────────────────────────────
        match key.code {
            // Insert mode entry
            KeyCode::Char('i') => Action::Insert,
            KeyCode::Char('a') => Action::InsertAppend,
            KeyCode::Char('I') => Action::InsertLineStart,
            KeyCode::Char('A') => Action::InsertLineEnd,
            KeyCode::Char('o') => Action::InsertNewlineBelow,
            KeyCode::Char('O') => Action::InsertNewlineAbove,

            // Movement — h/l do NOT wrap across lines (vim behaviour)
            KeyCode::Char('h') => Action::MoveLeft,
            KeyCode::Char('l') => Action::MoveRight,
            KeyCode::Left => Action::MoveLeft,
            KeyCode::Right => Action::MoveRight,
            KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
            KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
            KeyCode::Char('0') | KeyCode::Home => Action::MoveLineStart,
            // ^ — first non-whitespace character on the line
            KeyCode::Char('^') => Action::MoveFirstNonBlank,
            // $ lands ON the last character in Normal mode
            KeyCode::Char('$') | KeyCode::End => Action::MoveLineEndNormal,
            KeyCode::Char('w') => Action::MoveWordForward,
            KeyCode::Char('b') => Action::MoveWordBackward,
            KeyCode::Char('G') => Action::GotoFileBottom,

            // Delete / edit
            KeyCode::Char('x') => Action::DeleteChar,
            KeyCode::Char('D') => Action::DeleteToLineEnd,
            KeyCode::Char('u') => Action::Undo,

            // Yank / paste
            KeyCode::Char('p') => Action::PasteAfter,
            KeyCode::Char('P') => Action::PasteBefore,

            // Double-key prefixes: store first key, resolve on next keypress
            // d(d/w/$)  g(g)  y(y/w/$)  c(c/w/$)  f/t/F/T(char)
            KeyCode::Char('d')
            | KeyCode::Char('g')
            | KeyCode::Char('y')
            | KeyCode::Char('c')
            | KeyCode::Char('f')
            | KeyCode::Char('t')
            | KeyCode::Char('F')
            | KeyCode::Char('T') => {
                if let KeyCode::Char(ch) = key.code {
                    self.pending_key = Some(ch);
                }
                Action::Noop
            },

            // Ctrl+R — redo
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Redo,

            // Visual modes
            KeyCode::Char('v') => Action::Visual,
            KeyCode::Char('V') => Action::VisualLine,

            // Command mode
            KeyCode::Char(':') => Action::Command,

            // In-file search
            KeyCode::Char('/') => Action::InFileSearchStart,
            KeyCode::Char('n') => Action::InFileSearchNext,
            KeyCode::Char('N') => Action::InFileSearchPrev,

            _ => Action::Noop,
        }
    }

    /// Resolve the current leader key sequence
    fn resolve_leader_sequence(&mut self) -> Action {
        let mut temp_node: Option<&KeyNode> = None;

        for (i, &ch) in self.sequence.iter().enumerate() {
            if i == 0 {
                temp_node = self.leader_tree.get(&ch);
            } else if let Some(node) = temp_node {
                temp_node = node.children.get(&ch);
            } else {
                // Invalid sequence
                self.clear_sequence();
                return Action::Noop;
            }
        }

        if let Some(node) = temp_node {
            if let Some(action) = &node.action {
                // Complete action found
                let result = action.clone();
                self.clear_sequence();
                return result;
            } else if node.children.is_empty() {
                // Dead end
                self.clear_sequence();
                return Action::Noop;
            }
            // Still building sequence
            return Action::Noop;
        }

        // Invalid sequence
        self.clear_sequence();
        Action::Noop
    }
}
