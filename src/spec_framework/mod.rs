//! Pluggable prompt-framework layer for the agent panel.
//!
//! Slash commands typed in the agent panel (e.g. `/openspec.propose`) are intercepted
//! in [`AgentPanel::submit`] and resolved to a structured prompt template that is
//! prepended to the **user turn** of the next API call (not the system prompt).
//!
//! # Built-in framework: `open-spec`
//! A port of the OpenSpec 3-command workflow (Fission-AI/OpenSpec).
//! Commands: `openspec.propose`, `openspec.review`, `openspec.apply`
//!
//! # Custom framework
//! Point `[agent] spec_framework` at a directory of `.md` files.  Each file's
//! stem (without the `.md` extension) becomes a slash-command name.
//!
//! # Config (`~/.config/forgiven/config.toml`)
//! ```toml
//! [agent]
//! spec_framework = "open-spec"                   # built-in (default when set)
//! # spec_framework = "/path/to/my/framework"     # custom directory
//! # spec_framework = "none"                      # disabled (default when absent)
//! ```

pub mod spec_slicer;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::warn;

// ─────────────────────────────────────────────────────────────────────────────
// Built-in openspec templates (embedded at compile time)
// ─────────────────────────────────────────────────────────────────────────────

const OPENSPEC_PROPOSE: &str = include_str!("templates/openspec/propose.md");
const OPENSPEC_REVIEW: &str = include_str!("templates/openspec/review.md");
const OPENSPEC_APPLY: &str = include_str!("templates/openspec/apply.md");

fn openspec_templates() -> HashMap<String, &'static str> {
    [
        ("openspec.propose", OPENSPEC_PROPOSE),
        ("openspec.review", OPENSPEC_REVIEW),
        ("openspec.apply", OPENSPEC_APPLY),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

fn openspec_descriptions() -> HashMap<String, String> {
    [
        (
            "openspec.propose",
            "/openspec.propose <change-name> — elicit requirements, design, tasks",
        ),
        ("openspec.review", "/openspec.review <change-name>  — audit artefacts before implement"),
        ("openspec.apply", "/openspec.apply <change-name>   — implement tasks, archive to specs/"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
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
    /// injecting their template.  All built-in openspec commands set this;
    /// custom-framework templates opt in via `clears_context: true` front-matter.
    clears_context: HashSet<String>,
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
    /// Load the built-in open-spec framework.
    ///
    /// All three openspec commands automatically clear the conversation context
    /// before injecting their template — each phase should start with a clean
    /// slate to avoid carrying expensive prior-phase history into the new turn.
    pub fn open_spec() -> Self {
        let clears_context: HashSet<String> =
            ["openspec.propose", "openspec.review", "openspec.apply"]
                .iter()
                .map(|s| s.to_string())
                .collect();

        Self {
            templates: openspec_templates()
                .into_iter()
                .map(|(k, v)| {
                    let (_, body) = parse_front_matter(v);
                    (k, body.to_string())
                })
                .collect(),
            descriptions: openspec_descriptions(),
            name: "open-spec".to_string(),
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
    ///   before injecting the template (automatic for all openspec phase commands).
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
/// | `"open-spec"`    | `Some(SpecFramework::open_spec())`        |
/// | any other string  | treated as a filesystem path to a dir of `.md` files |
pub fn load_from_config(cfg: &str) -> Option<SpecFramework> {
    match cfg.trim() {
        "" | "none" => None,
        "open-spec" => Some(SpecFramework::open_spec()),
        path => {
            let p = Path::new(path);
            if p.is_dir() {
                Some(SpecFramework::from_directory(p))
            } else {
                warn!(
                    "spec_framework: '{}' is not 'none', 'open-spec', or a valid directory — \
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
    fn open_spec_resolves_propose() {
        let fw = SpecFramework::open_spec();
        let (template, rest, clears) = fw.resolve("/openspec.propose add-dark-mode").unwrap();
        assert!(template.contains("PROPOSE"));
        assert_eq!(rest, "add-dark-mode");
        assert!(clears, "openspec commands should always clear context");
    }

    #[test]
    fn open_spec_command_only_no_args() {
        let fw = SpecFramework::open_spec();
        let (_, rest, _) = fw.resolve("/openspec.review").unwrap();
        assert_eq!(rest, "");
    }

    #[test]
    fn all_openspec_commands_clear_context() {
        let fw = SpecFramework::open_spec();
        for cmd in &["openspec.propose", "openspec.review", "openspec.apply"] {
            let input = format!("/{cmd}");
            let (_, _, clears) = fw.resolve(&input).unwrap();
            assert!(clears, "{cmd} should have clears_context = true");
        }
    }

    #[test]
    fn all_three_commands_present() {
        let fw = SpecFramework::open_spec();
        let cmds = fw.commands();
        for name in &["openspec.propose", "openspec.review", "openspec.apply"] {
            assert!(cmds.contains(name), "missing command: {name}");
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
        let fw = SpecFramework::open_spec();
        assert!(fw.resolve("/nonexistent").is_none());
    }

    #[test]
    fn non_slash_input_returns_none() {
        let fw = SpecFramework::open_spec();
        assert!(fw.resolve("just a normal message").is_none());
    }

    #[test]
    fn load_from_config_none_variants() {
        assert!(load_from_config("none").is_none());
        assert!(load_from_config("").is_none());
        assert!(load_from_config("  ").is_none());
    }

    #[test]
    fn load_from_config_open_spec() {
        let fw = load_from_config("open-spec").unwrap();
        assert_eq!(fw.name, "open-spec");
        assert!(!fw.commands().is_empty());
    }

    #[test]
    fn load_from_config_spec_kit_falls_through() {
        // "spec-kit" is no longer a valid built-in — falls through to dir lookup and returns None.
        assert!(load_from_config("spec-kit").is_none());
    }
}
