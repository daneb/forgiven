//! Agent hooks — event-driven automation (ADR 0114).
#![allow(dead_code)]
//!
//! Hooks let the user configure the agent to fire automatically when project
//! events occur (e.g. a file is saved).  Each hook is defined in config as a
//! `[[agent.hooks]]` TOML block with a `trigger`, a `glob` filter, and a
//! `prompt`.
//!
//! # Glob dialect
//!
//! Patterns support `*` (any non-separator chars), `**` (any chars including
//! separators), and `?` (any single non-separator char).  All other characters
//! are literal.

use anyhow::Result;

use super::Editor;

// ── Glob matching ─────────────────────────────────────────────────────────────

/// Returns `true` if `path` (project-relative, forward-slash separated)
/// matches `pattern` using the minimal glob dialect described in ADR 0114.
///
/// Patterns that contain no `/` are matched against the **filename component**
/// only (gitignore semantics), so `*.rs` matches `src/editor/mod.rs`.
/// Patterns with `/` (e.g. `src/**/*.rs`, `**/*.ts`) are matched against the
/// full path.
pub(super) fn glob_matches(pattern: &str, path: &str) -> bool {
    let path = path.replace('\\', "/");
    let pat = pattern.replace('\\', "/");
    if !pat.contains('/') {
        // No separator in pattern → match against filename only.
        let filename = path.split('/').next_back().unwrap_or(&path);
        let pat_chars: Vec<char> = pat.chars().collect();
        let fname_chars: Vec<char> = filename.chars().collect();
        return match_glob(&pat_chars, &fname_chars);
    }
    let pat_chars: Vec<char> = pat.chars().collect();
    let path_chars: Vec<char> = path.chars().collect();
    match_glob(&pat_chars, &path_chars)
}

fn match_glob(pat: &[char], text: &[char]) -> bool {
    match pat.first() {
        // Both exhausted — full match.
        None => text.is_empty(),

        // `**` — match zero or more characters including `/`.
        Some('*') if pat.get(1) == Some(&'*') => {
            let rest = &pat[2..];
            // Skip a leading `/` after `**` (e.g. `**/foo` → match `foo`)
            let rest = if rest.first() == Some(&'/') { &rest[1..] } else { rest };
            // Try matching `rest` against every suffix of `text`.
            if match_glob(rest, text) {
                return true;
            }
            for i in 0..text.len() {
                if match_glob(rest, &text[i + 1..]) {
                    return true;
                }
            }
            false
        },

        // `*` — match zero or more non-separator characters.
        Some('*') => {
            let rest = &pat[1..];
            if match_glob(rest, text) {
                return true;
            }
            for i in 0..text.len() {
                if text[i] == '/' {
                    break;
                }
                if match_glob(rest, &text[i + 1..]) {
                    return true;
                }
            }
            false
        },

        // `?` — match any single non-separator character.
        Some('?') => match text.first() {
            Some(c) if *c != '/' => match_glob(&pat[1..], &text[1..]),
            _ => false,
        },

        // Literal character.
        Some(p) => match text.first() {
            Some(t) if t == p => match_glob(&pat[1..], &text[1..]),
            _ => false,
        },
    }
}

// ── Hook firing ───────────────────────────────────────────────────────────────

// ── Test runner ───────────────────────────────────────────────────────────────

/// Detect the test command from project root if none is configured.
/// Precedence: `Cargo.toml` → `cargo test`, `package.json` → `npm test`,
/// `pyproject.toml` / `pytest.ini` → `pytest`.
/// Returns `None` if no recognised framework is found.
fn detect_test_command(project_root: &std::path::Path) -> Option<String> {
    if project_root.join("Cargo.toml").exists() {
        return Some("cargo test".into());
    }
    if project_root.join("package.json").exists() {
        return Some("npm test".into());
    }
    if project_root.join("pyproject.toml").exists()
        || project_root.join("pytest.ini").exists()
        || project_root.join("setup.cfg").exists()
    {
        return Some("pytest".into());
    }
    None
}

impl Editor {
    /// Called after a successful `FileSave`. Agent hooks are removed in slim build.
    pub(super) fn fire_hooks_for_save(&mut self, _saved_path: &std::path::Path) -> Result<()> {
        // TODO: hooks removed in slim build
        Ok(())
    }

    /// Run the configured test command. Agent hooks are removed in slim build.
    pub(super) fn run_tests_if_configured(&mut self, _saved_path: &std::path::Path) -> Result<()> {
        // TODO: hooks removed in slim build
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::glob_matches;

    #[test]
    fn star_ext() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(glob_matches("*.rs", "src/lib.rs"));
        assert!(!glob_matches("*.rs", "main.py"));
    }

    #[test]
    fn double_star_ext() {
        assert!(glob_matches("**/*.rs", "src/editor/mod.rs"));
        assert!(glob_matches("**/*.rs", "main.rs"));
        assert!(!glob_matches("**/*.rs", "main.py"));
    }

    #[test]
    fn prefix_double_star() {
        assert!(glob_matches("src/**/*.rs", "src/editor/mod.rs"));
        assert!(glob_matches("src/**/*.rs", "src/main.rs"));
        assert!(!glob_matches("src/**/*.rs", "tests/foo.rs"));
    }

    #[test]
    fn literal() {
        assert!(glob_matches("config.toml", "config.toml"));
        assert!(!glob_matches("config.toml", "other.toml"));
    }

    #[test]
    fn question_mark() {
        assert!(glob_matches("src/?.rs", "src/a.rs"));
        assert!(!glob_matches("src/?.rs", "src/ab.rs"));
    }

    #[test]
    fn no_separator_deep_path() {
        // Pattern without `/` uses gitignore semantics: matched against filename only.
        assert!(glob_matches("*.rs", "a/b/c/foo.rs"));
    }

    #[test]
    fn anchored_prefix() {
        // Pattern with `/` matched against full path; `*` won't cross separators.
        assert!(!glob_matches("src/*.rs", "src/a/b.rs"));
        assert!(glob_matches("src/*.rs", "src/main.rs"));
    }

    #[test]
    fn double_star_root() {
        // `**` alone matches any path.
        assert!(glob_matches("**", "anything/deep/foo.rs"));
        assert!(glob_matches("**", "top.txt"));
    }

    #[test]
    fn empty_pattern_empty_path() {
        assert!(glob_matches("", ""));
    }

    #[test]
    fn empty_pattern_non_empty_path() {
        assert!(!glob_matches("", "foo"));
    }
}
