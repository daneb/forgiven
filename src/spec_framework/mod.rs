//! Pluggable prompt-framework layer for the agent panel.
//!
//! Slash commands typed in the agent panel (e.g. `/speckit.specify`) are intercepted
//! in [`AgentPanel::submit`] and resolved to a structured prompt template that is
//! injected into the system prompt of the next API call.
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

use std::collections::HashMap;
use std::path::Path;
use tracing::warn;

// ─────────────────────────────────────────────────────────────────────────────
// Built-in spec-kit templates (embedded at compile time)
// ─────────────────────────────────────────────────────────────────────────────

const SPECKIT_CONSTITUTION: &str =
    include_str!("templates/speckit/constitution.md");
const SPECKIT_SPECIFY: &str =
    include_str!("templates/speckit/specify.md");
const SPECKIT_PLAN: &str =
    include_str!("templates/speckit/plan.md");
const SPECKIT_TASKS: &str =
    include_str!("templates/speckit/tasks.md");
const SPECKIT_IMPLEMENT: &str =
    include_str!("templates/speckit/implement.md");
const SPECKIT_CLARIFY: &str =
    include_str!("templates/speckit/clarify.md");
const SPECKIT_ANALYZE: &str =
    include_str!("templates/speckit/analyze.md");

fn speckit_templates() -> HashMap<String, &'static str> {
    [
        ("speckit.constitution", SPECKIT_CONSTITUTION),
        ("speckit.specify",      SPECKIT_SPECIFY),
        ("speckit.plan",         SPECKIT_PLAN),
        ("speckit.tasks",        SPECKIT_TASKS),
        ("speckit.implement",    SPECKIT_IMPLEMENT),
        ("speckit.clarify",      SPECKIT_CLARIFY),
        ("speckit.analyze",      SPECKIT_ANALYZE),
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
    /// Command name → template text.
    templates: HashMap<String, String>,
    /// Human-readable name shown in the agent panel status line.
    pub name: String,
}

impl SpecFramework {
    /// Load the built-in spec-kit framework.
    pub fn spec_kit() -> Self {
        Self {
            templates: speckit_templates()
                .into_iter()
                .map(|(k, v)| (k, v.to_string()))
                .collect(),
            name: "spec-kit".to_string(),
        }
    }

    /// Load a custom framework from a directory.
    ///
    /// Each `.md` file in the directory becomes a command whose name equals the
    /// file stem (e.g. `my-command.md` → `/my-command`).
    pub fn from_directory(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("custom")
            .to_string();

        let mut templates = HashMap::new();
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
                            templates.insert(stem.to_string(), content);
                        },
                        Err(e) => warn!("spec_framework: could not read {:?}: {e}", p),
                    }
                }
            },
            Err(e) => warn!("spec_framework: could not read directory {:?}: {e}", path),
        }

        Self { templates, name }
    }

    /// Try to resolve a slash command from the start of `input`.
    ///
    /// Returns `Some((template, remaining))` when `input` starts with `/<known-command>`.
    /// `remaining` is everything after the command name, left-trimmed — it becomes the
    /// user's message that is appended to the template.
    ///
    /// Returns `None` if the input doesn't start with `/` or the command is unknown
    /// (in which case the input is forwarded to the model unchanged).
    pub fn resolve<'a>(&self, input: &'a str) -> Option<(&str, &'a str)> {
        let without_slash = input.trim_start().strip_prefix('/')?;
        let (cmd, rest) = without_slash
            .split_once(char::is_whitespace)
            .map(|(c, r)| (c, r.trim_start()))
            .unwrap_or((without_slash, ""));
        let template = self.templates.get(cmd)?;
        Some((template.as_str(), rest))
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
        let (template, rest) = fw.resolve("/speckit.specify describe a todo app").unwrap();
        assert!(template.contains("Phase 2"));
        assert_eq!(rest, "describe a todo app");
    }

    #[test]
    fn spec_kit_command_only_no_args() {
        let fw = SpecFramework::spec_kit();
        let (_, rest) = fw.resolve("/speckit.plan").unwrap();
        assert_eq!(rest, "");
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
