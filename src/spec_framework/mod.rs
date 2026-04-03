//! Pluggable prompt-framework layer for the agent panel.
//!
//! Slash commands typed in the agent panel (e.g. `/speckit.specify`) are intercepted
//! in [`AgentPanel::submit`] and resolved to a structured prompt template that is
//! prepended to the **user turn** of the next API call (not the system prompt).
//!
//! # Built-in framework: `spec-kit`
//! A port of GitHub's Spec-Driven Development workflow (github/spec-kit).
//! Commands: `speckit.constitution`, `speckit.specify`, `speckit.plan`,
//!            `speckit.tasks`, `speckit.implement`, `speckit.clarify`, `speckit.analyze`
//!
//! # Custom framework
//! Point `[agent] spec_framework` at a directory of `.md` files.  Each file's
//! stem (without the `.md` extension) becomes a slash-command name.
//!
//! # Config (`~/.config/forgiven/config.toml`)
//! ```toml
//! [agent]
//! spec_framework = "spec-kit"                    # built-in (default when set)
//! # spec_framework = "/path/to/my/framework"     # custom directory
//! # spec_framework = "none"                      # disabled (default when absent)
//! ```

use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::warn;

// ─────────────────────────────────────────────────────────────────────────────
// Built-in spec-kit templates (embedded at compile time)
// ─────────────────────────────────────────────────────────────────────────────

const SPECKIT_CONSTITUTION: &str = include_str!("templates/speckit/constitution.md");
const SPECKIT_SPECIFY: &str = include_str!("templates/speckit/specify.md");
const SPECKIT_PLAN: &str = include_str!("templates/speckit/plan.md");
const SPECKIT_TASKS: &str = include_str!("templates/speckit/tasks.md");
const SPECKIT_IMPLEMENT: &str = include_str!("templates/speckit/implement.md");
const SPECKIT_CLARIFY: &str = include_str!("templates/speckit/clarify.md");
const SPECKIT_ANALYZE: &str = include_str!("templates/speckit/analyze.md");

fn speckit_templates() -> HashMap<String, &'static str> {
    [
        ("speckit.constitution", SPECKIT_CONSTITUTION),
        ("speckit.specify", SPECKIT_SPECIFY),
        ("speckit.plan", SPECKIT_PLAN),
        ("speckit.tasks", SPECKIT_TASKS),
        ("speckit.implement", SPECKIT_IMPLEMENT),
        ("speckit.clarify", SPECKIT_CLARIFY),
        ("speckit.analyze", SPECKIT_ANALYZE),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// SpecFramework — a loaded set of slash-command → template mappings
// ─────────────────────────────────────────────────────────────────────────────

/// A loaded prompt framework ready for slash-command resolution.
#[allow(dead_code)] // `name` and `commands` are public API used by future UI
pub struct SpecFramework {
    /// Command name → template text (front-matter already stripped).
    templates: HashMap<String, String>,
    /// Command name → short description shown in the slash-menu hint line.
    descriptions: HashMap<String, String>,
    /// Human-readable name shown in the agent panel status line.
    pub name: String,
    /// Commands that should automatically start a new conversation before
    /// injecting their template.  All built-in spec-kit commands set this;
    /// custom-framework templates opt in via `clears_context: true` front-matter.
    clears_context: HashSet<String>,
}

fn speckit_descriptions() -> HashMap<String, String> {
    [
        ("speckit.constitution", "Step 1 · Define project principles & constraints"),
        ("speckit.specify", "Step 2 · /speckit.specify <feature-name> [context]"),
        ("speckit.plan", "Step 3 · /speckit.plan <feature-name>"),
        ("speckit.tasks", "Step 4 · /speckit.tasks <feature-name>"),
        ("speckit.implement", "Step 5 · /speckit.implement <feature-name>"),
        ("speckit.clarify", "Step 6 · /speckit.clarify [feature-name]"),
        ("speckit.analyze", "/speckit.analyze [feature-name]"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Parse optional YAML-lite front-matter from a template file.
///
/// Recognises a leading `---\n … \n---\n` block. The only supported key is:
/// `clears_context: true` — when present the command will automatically start
/// a new conversation before injecting the template.
///
/// Returns `(clears_context, body)` where `body` is the template text with
/// the front-matter block removed.
fn parse_front_matter(content: &str) -> (bool, &str) {
    let Some(after_open) = content.strip_prefix("---\n") else {
        return (false, content);
    };
    let Some(close_pos) = after_open.find("\n---\n") else {
        return (false, content);
    };
    let front_matter = &after_open[..close_pos];
    let body_raw = &after_open[close_pos + 5..]; // skip "\n---\n"
                                                 // Strip one leading newline — the conventional blank line after the closing "---".
    let body = body_raw.strip_prefix('\n').unwrap_or(body_raw);
    let clears =
        front_matter.lines().any(|line| line.trim().eq_ignore_ascii_case("clears_context: true"));
    (clears, body)
}

impl SpecFramework {
    /// Load the built-in spec-kit framework.
    ///
    /// All seven spec-kit commands automatically clear the conversation context
    /// before injecting their template — each phase should start with a clean
    /// slate to avoid carrying expensive prior-phase history into the new turn.
    pub fn spec_kit() -> Self {
        // All built-in commands are phase boundaries; every one clears context.
        let clears_context: HashSet<String> = [
            "speckit.constitution",
            "speckit.specify",
            "speckit.plan",
            "speckit.tasks",
            "speckit.implement",
            "speckit.clarify",
            "speckit.analyze",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        Self {
            templates: speckit_templates().into_iter().map(|(k, v)| (k, v.to_string())).collect(),
            descriptions: speckit_descriptions(),
            name: "spec-kit".to_string(),
            clears_context,
        }
    }

    /// Load a custom framework from a directory.
    ///
    /// Each `.md` file becomes a command whose name equals the file stem.
    /// Templates may opt in to automatic context-clearing via front-matter:
    ///
    /// ```markdown
    /// ---
    /// clears_context: true
    /// ---
    ///
    /// Your template content here…
    /// ```
    pub fn from_directory(path: &Path) -> Self {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("custom").to_string();

        let mut templates = HashMap::new();
        let mut clears_context = HashSet::new();
        let descriptions = HashMap::new();

        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) != Some("md") {
                        continue;
                    }
                    let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    match std::fs::read_to_string(&p) {
                        Ok(content) => {
                            let (clears, body) = parse_front_matter(&content);
                            if clears {
                                clears_context.insert(stem.to_string());
                            }
                            // Store the body with front-matter stripped.
                            templates.insert(stem.to_string(), body.to_string());
                        },
                        Err(e) => warn!("spec_framework: could not read {:?}: {e}", p),
                    }
                }
            },
            Err(e) => warn!("spec_framework: could not read directory {:?}: {e}", path),
        }

        Self { templates, descriptions, name, clears_context }
    }

    /// Try to resolve a slash command from the start of `input`.
    ///
    /// Returns `Some((template, remaining, clears_context))` when `input` starts
    /// with `/<known-command>`:
    /// - `template` — the prompt text to prepend to the user turn.
    /// - `remaining` — everything after the command name, left-trimmed.
    /// - `clears_context` — when `true` the caller should start a new conversation
    ///   before injecting the template (automatic for all spec-kit phase commands).
    ///
    /// Returns `None` if the input doesn't start with `/` or the command is unknown.
    pub fn resolve<'a>(&self, input: &'a str) -> Option<(&str, &'a str, bool)> {
        let without_slash = input.trim_start().strip_prefix('/')?;
        let (cmd, rest) = without_slash
            .split_once(char::is_whitespace)
            .map(|(c, r)| (c, r.trim_start()))
            .unwrap_or((without_slash, ""));
        let template = self.templates.get(cmd)?;
        let clears = self.clears_context.contains(cmd);
        Some((template.as_str(), rest, clears))
    }

    /// Short description for a command, shown as a hint in the slash-menu popup.
    pub fn describe(&self, cmd: &str) -> Option<&str> {
        self.descriptions.get(cmd).map(String::as_str)
    }

    /// All registered command names, sorted (useful for help text or autocomplete).
    #[allow(dead_code)]
    pub fn commands(&self) -> Vec<&str> {
        let mut cmds: Vec<&str> = self.templates.keys().map(String::as_str).collect();
        cmds.sort_unstable();
        cmds
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Factory
// ─────────────────────────────────────────────────────────────────────────────

/// Construct a [`SpecFramework`] from the `[agent] spec_framework` config value.
///
/// | config value      | result                                    |
/// |-------------------|-------------------------------------------|
/// | `"none"` / `""`  | `None` — framework disabled               |
/// | `"spec-kit"`     | `Some(SpecFramework::spec_kit())`         |
/// | any other string  | treated as a filesystem path to a dir of `.md` files |
pub fn load_from_config(cfg: &str) -> Option<SpecFramework> {
    match cfg.trim() {
        "" | "none" => None,
        "spec-kit" => Some(SpecFramework::spec_kit()),
        path => {
            let p = Path::new(path);
            if p.is_dir() {
                Some(SpecFramework::from_directory(p))
            } else {
                warn!(
                    "spec_framework: '{}' is not 'none', 'spec-kit', or a valid directory — \
                     framework disabled",
                    path
                );
                None
            }
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_kit_resolves_known_command() {
        let fw = SpecFramework::spec_kit();
        let (template, rest, clears) = fw.resolve("/speckit.specify describe a todo app").unwrap();
        assert!(template.contains("Phase 2"));
        assert_eq!(rest, "describe a todo app");
        assert!(clears, "spec-kit commands should always clear context");
    }

    #[test]
    fn spec_kit_command_only_no_args() {
        let fw = SpecFramework::spec_kit();
        let (_, rest, _) = fw.resolve("/speckit.plan").unwrap();
        assert_eq!(rest, "");
    }

    #[test]
    fn all_spec_kit_commands_clear_context() {
        let fw = SpecFramework::spec_kit();
        for cmd in &[
            "speckit.constitution",
            "speckit.specify",
            "speckit.plan",
            "speckit.tasks",
            "speckit.implement",
            "speckit.clarify",
            "speckit.analyze",
        ] {
            let input = format!("/{cmd}");
            let (_, _, clears) = fw.resolve(&input).unwrap();
            assert!(clears, "{cmd} should have clears_context = true");
        }
    }

    #[test]
    fn front_matter_clears_context_parsed() {
        let (clears, body) = parse_front_matter("---\nclears_context: true\n---\n\nHello world");
        assert!(clears);
        assert_eq!(body, "Hello world");
    }

    #[test]
    fn front_matter_absent_defaults_false() {
        let (clears, body) = parse_front_matter("No front matter here");
        assert!(!clears);
        assert_eq!(body, "No front matter here");
    }

    #[test]
    fn unknown_command_returns_none() {
        let fw = SpecFramework::spec_kit();
        assert!(fw.resolve("/nonexistent").is_none());
    }

    #[test]
    fn non_slash_input_returns_none() {
        let fw = SpecFramework::spec_kit();
        assert!(fw.resolve("just a normal message").is_none());
    }

    #[test]
    fn load_from_config_none_variants() {
        assert!(load_from_config("none").is_none());
        assert!(load_from_config("").is_none());
        assert!(load_from_config("  ").is_none());
    }

    #[test]
    fn load_from_config_spec_kit() {
        let fw = load_from_config("spec-kit").unwrap();
        assert_eq!(fw.name, "spec-kit");
        assert!(!fw.commands().is_empty());
    }

    #[test]
    fn all_seven_commands_present() {
        let fw = SpecFramework::spec_kit();
        let cmds = fw.commands();
        for name in &[
            "speckit.constitution",
            "speckit.specify",
            "speckit.plan",
            "speckit.tasks",
            "speckit.implement",
            "speckit.clarify",
            "speckit.analyze",
        ] {
            assert!(cmds.contains(name), "missing command: {name}");
        }
    }
}
