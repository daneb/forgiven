//! Phase 2 of the context-optimisation roadmap: the Spec Slicer.
//!
//! Instead of letting the model read full spec files on every
//! `/speckit.implement` round, [`SpecSlicer`] pre-extracts:
//! - the **active task** — the first unchecked `- [ ]` entry in `TASKS.md`
//! - **relevant spec sections** — sections of `SPEC.md` whose headings or
//!   opening content contain keywords from the active task title
//!
//! The result is a compact "virtual context" block (~100–1 300 t) injected into
//! the user turn before the template text, saving ~2 000–7 000 t of file reads
//! per implement round.
//!
//! See ADR 0100 for design rationale.

use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// The first unchecked task found in `TASKS.md`, with its surrounding context.
#[derive(Debug, Clone)]
pub struct ActiveTask {
    /// The `## Phase N — ...` heading immediately above this task, if any.
    pub phase_heading: Option<String>,
    /// The task title (text after `- [ ] `).
    pub title: String,
    /// Indented detail lines (inputs, outputs, acceptance condition).
    pub body: String,
}

/// A single `## Section` extracted from `SPEC.md`.
#[derive(Debug, Clone)]
pub struct SpecSection {
    pub heading: String,
    pub content: String,
}

/// Pre-extracted context to inject into the implement template.
#[derive(Debug, Clone)]
pub struct VirtualContext {
    pub active_task: ActiveTask,
    /// Up to three spec sections relevant to the active task (may be empty if
    /// `SPEC.md` is absent or no keyword matches were found).
    pub spec_sections: Vec<SpecSection>,
}

// ─────────────────────────────────────────────────────────────────────────────
// SpecSlicer
// ─────────────────────────────────────────────────────────────────────────────

pub struct SpecSlicer;

impl SpecSlicer {
    /// Parse `TASKS.md` text and return the first unchecked `- [ ]` task.
    ///
    /// Tracks `## ` phase headings so the caller can show which phase the
    /// active task belongs to. Body lines are all subsequent lines indented
    /// by at least two spaces (or a tab), up to the next task marker or heading.
    pub fn parse_active_task(tasks_md: &str) -> Option<ActiveTask> {
        let mut current_phase: Option<String> = None;
        let mut active: Option<ActiveTask> = None;
        let mut in_active_body = false;

        for line in tasks_md.lines() {
            if active.is_some() && in_active_body {
                let is_body =
                    line.starts_with("  ") || line.starts_with('\t') || line.trim().is_empty();
                let is_next_task = line.trim_start().starts_with("- [");
                let is_heading = line.starts_with("## ") || line.starts_with("# ");
                if is_body && !is_next_task && !is_heading {
                    if let Some(ref mut task) = active {
                        if !task.body.is_empty() || !line.trim().is_empty() {
                            task.body.push_str(line);
                            task.body.push('\n');
                        }
                    }
                    continue;
                } else {
                    // Reached the next task or heading — body collection done.
                    break;
                }
            }

            if line.starts_with("## ") || (line.starts_with("# ") && !line.starts_with("## ")) {
                current_phase = Some(line.trim_start_matches('#').trim().to_string());
                continue;
            }

            // Look for unchecked task marker (also handles leading whitespace lists).
            let trimmed = line.trim_start();
            if trimmed.starts_with("- [ ] ") || trimmed.starts_with("* [ ] ") {
                let title =
                    trimmed.trim_start_matches("- [ ] ").trim_start_matches("* [ ] ").to_string();
                active = Some(ActiveTask {
                    phase_heading: current_phase.clone(),
                    title,
                    body: String::new(),
                });
                in_active_body = true;
                continue;
            }
        }

        // Trim trailing whitespace from body.
        if let Some(ref mut task) = active {
            let trimmed = task.body.trim_end().to_string();
            task.body = trimmed;
        }

        active
    }

    /// Extract spec sections from `spec_md` that are relevant to `task`.
    ///
    /// Splits by `## ` headings. Scores each section by keyword overlap with
    /// the task title. Returns the three highest-scoring matches (minimum score
    /// of 1 required — at least one keyword must match).
    pub fn slice_spec(spec_md: &str, task: &ActiveTask) -> Vec<SpecSection> {
        let keywords = extract_keywords(&task.title);
        if keywords.is_empty() {
            return vec![];
        }

        let sections = split_sections(spec_md);
        let mut scored: Vec<(usize, SpecSection)> = sections
            .into_iter()
            .filter_map(|(heading, content)| {
                let score = score_section(&heading, &content, &keywords);
                if score > 0 {
                    Some((score, SpecSection { heading, content }))
                } else {
                    None
                }
            })
            .collect();

        // Highest score first, then stable original order for ties.
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(3).map(|(_, s)| s).collect()
    }

    /// Build a [`VirtualContext`] from the feature directory on disk.
    ///
    /// Reads `<feature_dir>/TASKS.md` and `<feature_dir>/SPEC.md`.
    /// Returns `None` if `TASKS.md` is missing or has no unchecked tasks.
    pub fn build(feature_dir: &Path) -> Option<VirtualContext> {
        let tasks_path = feature_dir.join("TASKS.md");
        let tasks_md = std::fs::read_to_string(&tasks_path).ok()?;

        let active_task = Self::parse_active_task(&tasks_md)?;

        let spec_sections =
            if let Ok(spec_md) = std::fs::read_to_string(feature_dir.join("SPEC.md")) {
                Self::slice_spec(&spec_md, &active_task)
            } else {
                vec![]
            };

        Some(VirtualContext { active_task, spec_sections })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VirtualContext formatting
// ─────────────────────────────────────────────────────────────────────────────

impl VirtualContext {
    /// Format as a markdown block suitable for injection into the user turn.
    pub fn to_prompt_block(&self) -> String {
        let mut out = String::from(
            "<!-- SpecSlicer: pre-extracted virtual context — saves full-file read overhead -->\n",
        );
        out.push_str("## Active Task\n\n");

        if let Some(ref phase) = self.active_task.phase_heading {
            out.push_str(&format!("> {phase}\n\n"));
        }

        out.push_str(&format!("- [ ] {}\n", self.active_task.title));
        if !self.active_task.body.is_empty() {
            for line in self.active_task.body.lines() {
                out.push_str(line);
                out.push('\n');
            }
        }

        if !self.spec_sections.is_empty() {
            out.push_str("\n## Relevant Spec Sections\n");
            for section in &self.spec_sections {
                out.push_str(&format!("\n### {}\n", section.heading));
                // Cap section content at 400 characters to bound token cost.
                let content = section.content.trim();
                if content.len() > 400 {
                    out.push_str(&content[..400]);
                    out.push_str("\n… *(truncated — call read_file for full content)*");
                } else {
                    out.push_str(content);
                }
                out.push('\n');
            }
        }

        out.push_str("\n<!-- End SpecSlicer block -->\n");
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

const STOPWORDS: &[&str] = &[
    "with", "from", "into", "that", "this", "have", "will", "when", "each", "then", "than", "also",
    "been", "does", "should", "which", "where", "their", "there", "create", "update", "delete",
    "remove", "change", "using",
];

fn extract_keywords(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4)
        .filter(|w| !STOPWORDS.contains(&w.to_lowercase().as_str()))
        .map(|w| w.to_lowercase())
        .collect()
}

/// Split a markdown string into `(heading_text, section_body)` pairs.
/// Sections delimited by `## ` lines. Content before the first heading is
/// assigned heading `""` and is included only if non-empty.
fn split_sections(md: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_heading = String::new();
    let mut current_body = String::new();

    for line in md.lines() {
        if line.starts_with("## ") {
            if !current_body.trim().is_empty() || !current_heading.is_empty() {
                sections.push((current_heading.clone(), current_body.trim().to_string()));
            }
            current_heading = line.trim_start_matches('#').trim().to_string();
            current_body = String::new();
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }
    if !current_body.trim().is_empty() || !current_heading.is_empty() {
        sections.push((current_heading, current_body.trim().to_string()));
    }
    sections
}

fn score_section(heading: &str, content: &str, keywords: &[String]) -> usize {
    let haystack =
        format!("{} {}", heading.to_lowercase(), &content[..content.len().min(400)].to_lowercase());
    keywords.iter().filter(|kw| haystack.contains(kw.as_str())).count()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const TASKS_MD: &str = "\
## Phase 1 — Scaffolding

- [x] Create project layout
  Inputs: nothing
  Outputs: `src/` directory
  Acceptance: `cargo build` succeeds.

## Phase 2 — Database Layer

- [x] Write migration script
  Inputs: schema notes
  Outputs: `migrations/001.sql`
  Acceptance: migration applies cleanly.

- [ ] Implement user repository
  Inputs: `migrations/001.sql`, `PLAN.md`
  Outputs: `src/repo/user.rs`
  Acceptance: unit tests pass.

- [ ] Add index on email column
  Inputs: `migrations/001.sql`
  Outputs: `migrations/002.sql`
  Acceptance: query plan uses index.
";

    const SPEC_MD: &str = "\
## Overview

General project description.

## User Repository

Users are stored in a PostgreSQL table. The repository layer must expose
CRUD operations and must not leak database details to the service layer.

## Authentication

JWT tokens are issued on login with a 1-hour TTL.
";

    #[test]
    fn parse_active_task_finds_first_unchecked() {
        let task = SpecSlicer::parse_active_task(TASKS_MD).unwrap();
        assert_eq!(task.title, "Implement user repository");
        assert_eq!(task.phase_heading.as_deref(), Some("Phase 2 — Database Layer"));
        assert!(task.body.contains("src/repo/user.rs"));
    }

    #[test]
    fn parse_active_task_captures_body() {
        let task = SpecSlicer::parse_active_task(TASKS_MD).unwrap();
        assert!(task.body.contains("Inputs:"));
        assert!(task.body.contains("Acceptance:"));
        // Should NOT include next task's title or body
        assert!(!task.body.contains("index on email"));
    }

    #[test]
    fn parse_active_task_all_done_returns_none() {
        let all_done = TASKS_MD.replace("- [ ]", "- [x]");
        assert!(SpecSlicer::parse_active_task(&all_done).is_none());
    }

    #[test]
    fn parse_active_task_no_phase_heading() {
        let no_phase = "- [x] Done\n- [ ] Active task\n  body line\n";
        let task = SpecSlicer::parse_active_task(no_phase).unwrap();
        assert_eq!(task.title, "Active task");
        assert!(task.phase_heading.is_none());
    }

    #[test]
    fn slice_spec_matches_relevant_sections() {
        let task = SpecSlicer::parse_active_task(TASKS_MD).unwrap();
        let sections = SpecSlicer::slice_spec(SPEC_MD, &task);
        // "User Repository" section should match keywords "user", "repository"
        assert!(!sections.is_empty(), "expected at least one matching section");
        let headings: Vec<&str> = sections.iter().map(|s| s.heading.as_str()).collect();
        assert!(headings.contains(&"User Repository"), "expected 'User Repository' section");
    }

    #[test]
    fn slice_spec_caps_at_three_sections() {
        let many_sections = (0..10)
            .map(|i| format!("## Section {i}\nUser repository content here.\n"))
            .collect::<Vec<_>>()
            .join("\n");
        let task = ActiveTask {
            phase_heading: None,
            title: "Implement user repository".into(),
            body: String::new(),
        };
        let sections = SpecSlicer::slice_spec(&many_sections, &task);
        assert!(sections.len() <= 3);
    }

    #[test]
    fn virtual_context_block_contains_task_and_spec() {
        let task = SpecSlicer::parse_active_task(TASKS_MD).unwrap();
        let sections = SpecSlicer::slice_spec(SPEC_MD, &task);
        let vctx = VirtualContext { active_task: task, spec_sections: sections };
        let block = vctx.to_prompt_block();
        assert!(block.contains("Implement user repository"));
        assert!(block.contains("Phase 2 — Database Layer"));
        assert!(block.contains("User Repository"));
        assert!(block.contains("<!-- SpecSlicer"));
        assert!(block.contains("<!-- End SpecSlicer block -->"));
    }

    #[test]
    fn virtual_context_block_no_spec_sections() {
        let vctx = VirtualContext {
            active_task: ActiveTask {
                phase_heading: None,
                title: "Build the thing".into(),
                body: "  Inputs: nothing\n  Acceptance: it works.".into(),
            },
            spec_sections: vec![],
        };
        let block = vctx.to_prompt_block();
        assert!(!block.contains("Relevant Spec Sections"));
        assert!(block.contains("Build the thing"));
    }

    #[test]
    fn extract_keywords_filters_short_and_stopwords() {
        let kws = extract_keywords("Create user table with index");
        assert!(!kws.contains(&"with".to_string()));
        assert!(!kws.contains(&"cre".to_string())); // too short
        assert!(kws.contains(&"user".to_string()));
        assert!(kws.contains(&"table".to_string()));
        assert!(kws.contains(&"index".to_string()));
    }
}
