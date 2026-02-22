use crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Editor modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Visual,
    PickBuffer,  // For buffer selection UI
    PickFile,    // For file finder UI
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
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
    MoveWordForward,
    MoveWordBackward,
    Command,
    Visual,
    // Leader key actions
    BufferList,
    BufferNext,
    BufferPrevious,
    BufferClose,
    FileFind,
    FileSave,
    Quit,
}

/// Represents a keybinding tree node
#[derive(Debug, Clone)]
pub struct KeyNode {
    pub description: String,
    pub action: Option<Action>,
    pub children: HashMap<char, KeyNode>,
}

impl KeyNode {
    fn new(desc: impl Into<String>) -> Self {
        Self {
            description: desc.into(),
            action: None,
            children: HashMap::new(),
        }
    }

    fn leaf(desc: impl Into<String>, action: Action) -> Self {
        Self {
            description: desc.into(),
            action: Some(action),
            children: HashMap::new(),
        }
    }
}

/// Handles key events and maps them to editor actions with leader key support
pub struct KeyHandler {
    /// Current key sequence being built (for leader keys)
    sequence: Vec<char>,
    /// When the sequence started (for which-key timeout)
    sequence_start: Option<Instant>,
    /// Leader key bindings tree
    leader_tree: HashMap<char, KeyNode>,
    /// Which-key popup should be shown
    show_which_key: bool,
}

impl KeyHandler {
    pub fn new() -> Self {
        let leader_tree = Self::build_leader_tree();
        Self {
            sequence: Vec::new(),
            sequence_start: None,
            leader_tree,
            show_which_key: false,
        }
    }

    /// Build the Spacemacs-inspired leader key tree
    fn build_leader_tree() -> HashMap<char, KeyNode> {
        let mut tree = HashMap::new();

        // SPC b - Buffer commands
        let mut buffer_node = KeyNode::new("buffer");
        buffer_node.children.insert('b', KeyNode::leaf("list buffers", Action::BufferList));
        buffer_node.children.insert('n', KeyNode::leaf("next buffer", Action::BufferNext));
        buffer_node.children.insert('p', KeyNode::leaf("previous buffer", Action::BufferPrevious));
        buffer_node.children.insert('d', KeyNode::leaf("delete buffer", Action::BufferClose));
        tree.insert('b', buffer_node);

        // SPC f - File commands
        let mut file_node = KeyNode::new("file");
        file_node.children.insert('f', KeyNode::leaf("find file", Action::FileFind));
        file_node.children.insert('s', KeyNode::leaf("save file", Action::FileSave));
        tree.insert('f', file_node);

        // SPC q - Quit commands
        let mut quit_node = KeyNode::new("quit");
        quit_node.children.insert('q', KeyNode::leaf("quit", Action::Quit));
        tree.insert('q', quit_node);

        tree
    }

    /// Get the current key sequence (for display in status line)
    pub fn sequence(&self) -> String {
        if self.sequence.is_empty() {
            String::new()
        } else {
            format!("SPC {}", self.sequence.iter().collect::<String>())
        }
    }

    /// Check if which-key should be displayed
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
                        let key_seq = format!("SPC {}{}", self.sequence.iter().collect::<String>(), k);
                        (key_seq, child.description.clone())
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
    }

    /// Clear the current sequence
    pub fn clear_sequence(&mut self) {
        self.sequence.clear();
        self.sequence_start = None;
        self.show_which_key = false;
    }

    /// Handle a key in Normal mode, returning an action
    pub fn handle_normal(&mut self, key: KeyEvent) -> Action {
        // If we're building a leader sequence
        if !self.sequence.is_empty() {
            if let KeyCode::Char(ch) = key.code {
                self.sequence.push(ch);
                self.show_which_key = false; // Hide while typing

                // Try to resolve the sequence
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

        // Basic vim-like bindings
        match key.code {
            // Insert mode entry
            KeyCode::Char('i') => Action::Insert,
            KeyCode::Char('a') => Action::InsertAppend,
            KeyCode::Char('I') => Action::InsertLineStart,
            KeyCode::Char('A') => Action::InsertLineEnd,
            KeyCode::Char('o') => Action::InsertNewlineBelow,
            KeyCode::Char('O') => Action::InsertNewlineAbove,

            // Movement
            KeyCode::Char('h') | KeyCode::Left => Action::MoveLeft,
            KeyCode::Char('l') | KeyCode::Right => Action::MoveRight,
            KeyCode::Char('k') | KeyCode::Up => Action::MoveUp,
            KeyCode::Char('j') | KeyCode::Down => Action::MoveDown,
            KeyCode::Char('0') | KeyCode::Home => Action::MoveLineStart,
            KeyCode::Char('$') | KeyCode::End => Action::MoveLineEnd,
            KeyCode::Char('w') => Action::MoveWordForward,
            KeyCode::Char('b') => Action::MoveWordBackward,

            // Visual mode
            KeyCode::Char('v') => Action::Visual,

            // Command mode
            KeyCode::Char(':') => Action::Command,

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
