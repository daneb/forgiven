// Task tracking panel for project TODOs
// Similar to VS Code Copilot's task list

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    NotStarted,
    InProgress,
    Completed,
}

impl TaskStatus {
    pub fn icon(&self) -> &str {
        match self {
            TaskStatus::NotStarted => "○",
            TaskStatus::InProgress => "⦿",
            TaskStatus::Completed => "✓",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            TaskStatus::NotStarted => TaskStatus::InProgress,
            TaskStatus::InProgress => TaskStatus::Completed,
            TaskStatus::Completed => TaskStatus::NotStarted,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: usize,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskStorage {
    tasks: Vec<Task>,
    next_id: usize,
}

pub struct TaskPanel {
    pub visible: bool,
    pub focused: bool,
    pub tasks: Vec<Task>,
    pub selected: usize,
    #[allow(dead_code)]
    pub scroll: usize, // Reserved for future scrolling support
    pub input_buffer: String,
    pub editing: Option<usize>, // Task ID being edited
    next_id: usize,
    project_root: Option<PathBuf>,
}

impl TaskPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            focused: false,
            tasks: Vec::new(),
            selected: 0,
            scroll: 0,
            input_buffer: String::new(),
            editing: None,
            next_id: 1,
            project_root: None,
        }
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
        self.focused = self.visible;
    }

    pub fn focus(&mut self) {
        self.focused = true;
    }

    pub fn blur(&mut self) {
        self.focused = false;
    }

    /// Load tasks from project-local .forgiven/tasks.json
    pub fn load(&mut self, project_root: &PathBuf) {
        self.project_root = Some(project_root.clone());
        let path = project_root.join(".forgiven").join("tasks.json");
        
        if !path.exists() {
            return;
        }

        match std::fs::read_to_string(&path) {
            Ok(content) => {
                match serde_json::from_str::<TaskStorage>(&content) {
                    Ok(storage) => {
                        self.tasks = storage.tasks;
                        self.next_id = storage.next_id;
                    }
                    Err(e) => {
                        warn!("Failed to parse tasks.json: {}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read tasks.json: {}", e);
            }
        }
    }

    /// Save tasks to project-local .forgiven/tasks.json
    pub fn save(&self) {
        let Some(ref root) = self.project_root else { return };
        
        let dir = root.join(".forgiven");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("Failed to create .forgiven directory: {}", e);
            return;
        }

        let storage = TaskStorage {
            tasks: self.tasks.clone(),
            next_id: self.next_id,
        };

        let path = dir.join("tasks.json");
        match serde_json::to_string_pretty(&storage) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    warn!("Failed to write tasks.json: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to serialize tasks: {}", e);
            }
        }
    }

    /// Add a new task with the given title
    pub fn add_task(&mut self, title: String) {
        if title.trim().is_empty() {
            return;
        }
        
        let task = Task {
            id: self.next_id,
            title,
            status: TaskStatus::NotStarted,
        };
        self.tasks.push(task);
        self.next_id += 1;
        self.selected = self.tasks.len().saturating_sub(1);
        self.save();
    }

    /// Toggle the completion status of the selected task
    pub fn toggle_selected(&mut self) {
        if let Some(task) = self.tasks.get_mut(self.selected) {
            task.status = task.status.next();
            self.save();
        }
    }

    /// Delete the selected task
    pub fn delete_selected(&mut self) {
        if !self.tasks.is_empty() && self.selected < self.tasks.len() {
            self.tasks.remove(self.selected);
            if self.selected >= self.tasks.len() && self.selected > 0 {
                self.selected -= 1;
            }
            self.save();
        }
    }

    /// Start editing the selected task
    pub fn start_edit(&mut self) {
        if let Some(task) = self.tasks.get(self.selected) {
            self.editing = Some(task.id);
            self.input_buffer = task.title.clone();
        }
    }

    /// Finish editing and save changes
    pub fn finish_edit(&mut self) {
        if let Some(id) = self.editing {
            if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                if !self.input_buffer.trim().is_empty() {
                    task.title = self.input_buffer.clone();
                    self.save();
                }
            }
            self.editing = None;
            self.input_buffer.clear();
        }
    }

    /// Cancel editing without saving
    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.input_buffer.clear();
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.tasks.len() {
            self.selected += 1;
        }
    }

    /// Get completed task count
    pub fn completed_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.status == TaskStatus::Completed).count()
    }

    /// Get total task count
    pub fn total_count(&self) -> usize {
        self.tasks.len()
    }

    /// Handle character input (when editing or adding)
    pub fn input_char(&mut self, ch: char) {
        self.input_buffer.push(ch);
    }

    /// Handle backspace (when editing or adding)
    pub fn input_backspace(&mut self) {
        self.input_buffer.pop();
    }

    /// Check if we're in adding mode (no editing, but have input)
    pub fn is_adding(&self) -> bool {
        self.editing.is_none() && !self.input_buffer.is_empty()
    }
}
