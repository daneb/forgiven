//! Agent hooks — event-driven automation (ADR 0114).
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
use crate::agent::AgentStatus;

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
    /// Called after a successful `FileSave`.  Checks all configured `on_save`
    /// hooks against the saved file path and fires the first matching one if
    /// the agent is idle and the hook's cooldown has elapsed.
    pub(super) fn fire_hooks_for_save(&mut self, saved_path: &std::path::Path) -> Result<()> {
        // Skip if no hooks defined.
        if self.config.agent.hooks.is_empty() {
            return Ok(());
        }

        // Skip if the agent is already running.
        if self.agent_panel.status != AgentStatus::Idle {
            return Ok(());
        }

        // Compute project-relative path for glob matching.
        let cwd =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let rel_path = saved_path
            .strip_prefix(&cwd)
            .unwrap_or(saved_path)
            .to_string_lossy()
            .replace('\\', "/");

        let now = std::time::Instant::now();
        const COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5);

        // Collect config data first to avoid holding borrows during submit.
        let mut matched: Option<(usize, String)> = None; // (hook_index, prompt)
        for (i, hook) in self.config.agent.hooks.iter().enumerate() {
            if !hook.enabled || hook.trigger != "on_save" {
                continue;
            }
            if !glob_matches(&hook.glob, &rel_path) {
                continue;
            }
            // Cooldown check.
            if let Some(last) = self.hook_cooldowns.get(&i) {
                if now.duration_since(*last) < COOLDOWN {
                    continue;
                }
            }
            let prompt = hook.prompt.replace("{file}", &rel_path);
            matched = Some((i, prompt));
            break; // first match wins
        }

        let Some((hook_idx, prompt)) = matched else {
            return Ok(());
        };

        // Record cooldown before submitting.
        self.hook_cooldowns.insert(hook_idx, now);

        // Make the agent panel visible so the user can see the hook running.
        self.agent_panel.visible = true;

        // Add a system banner to the chat so the trigger is always visible.
        self.agent_panel.messages.push(crate::agent::ChatMessage {
            role: crate::agent::Role::System,
            content: format!("── Hook: on_save → {rel_path} ──"),
            images: Vec::new(),
        });

        // Inject the hook prompt as the user input and submit.
        self.agent_panel.input = prompt;

        let project_root = cwd;
        let max_rounds = self.config.max_agent_rounds;
        let warning_threshold = self.config.agent_warning_threshold;
        let preferred_model = self.config.active_default_model().to_string();
        let auto_compress = self.config.agent.auto_compress_tool_results;

        let fut = self.agent_panel.submit(
            None,
            project_root,
            max_rounds,
            warning_threshold,
            &preferred_model,
            auto_compress,
        );
        let submit_err = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match fut.await {
                    Ok(()) => None,
                    Err(e) => {
                        tracing::warn!("Hook submit error: {e}");
                        Some(e.to_string())
                    },
                }
            })
        });
        if let Some(e) = submit_err {
            self.set_status(format!("Hook error: {e}"));
        }
        Ok(())
    }

    /// Run the configured (or auto-detected) test command if `run_on_save` is
    /// enabled and at least one `on_test_fail` hook is configured.  On a
    /// pass→fail transition, fires the first matching `on_test_fail` hook.
    ///
    /// This is a no-op when `hooks_firing` is set (prevents hook re-entry).
    pub(super) fn run_tests_if_configured(&mut self, saved_path: &std::path::Path) -> Result<()> {
        // Re-entry guard: don't run tests while a hook-triggered agent is active.
        if self.hooks_firing {
            return Ok(());
        }

        // Only run if `run_on_save` is on and an on_test_fail hook exists.
        if !self.config.agent.test.run_on_save {
            return Ok(());
        }
        let has_test_fail_hook =
            self.config.agent.hooks.iter().any(|h| h.enabled && h.trigger == "on_test_fail");
        if !has_test_fail_hook {
            return Ok(());
        }

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let cmd_str = if self.config.agent.test.command.is_empty() {
            match detect_test_command(&cwd) {
                Some(c) => c,
                None => return Ok(()), // no framework detected
            }
        } else {
            self.config.agent.test.command.clone()
        };

        // Run the test command synchronously (blocks the UI briefly).
        // Timeout: 60 seconds.
        let mut parts = cmd_str.split_whitespace();
        let program = match parts.next() {
            Some(p) => p.to_string(),
            None => return Ok(()),
        };
        let args: Vec<String> = parts.map(|s| s.to_string()).collect();

        let output = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    tokio::process::Command::new(&program)
                        .args(&args)
                        .current_dir(&cwd)
                        .output(),
                )
                .await
            })
        });

        let (passed, combined_output) = match output {
            Ok(Ok(out)) => {
                let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
                combined.push_str(&String::from_utf8_lossy(&out.stderr));
                (out.status.success(), combined)
            },
            Ok(Err(e)) => {
                tracing::warn!("[hooks] test command failed to run: {e}");
                return Ok(());
            },
            Err(_) => {
                tracing::warn!("[hooks] test command timed out after 60 s");
                return Ok(());
            },
        };

        let prev = self.last_test_passed;
        self.last_test_passed = Some(passed);

        // Only fire on a pass→fail transition (not on repeated failures).
        let newly_failing = !passed && prev != Some(false);
        if newly_failing {
            self.fire_hooks_for_test_fail(saved_path, &combined_output)?;
        }

        Ok(())
    }

    /// Fire the first matching `on_test_fail` hook.
    fn fire_hooks_for_test_fail(
        &mut self,
        saved_path: &std::path::Path,
        test_output: &str,
    ) -> Result<()> {
        if self.agent_panel.status != AgentStatus::Idle {
            return Ok(());
        }

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let rel_path = saved_path
            .strip_prefix(&cwd)
            .unwrap_or(saved_path)
            .to_string_lossy()
            .replace('\\', "/");

        let now = std::time::Instant::now();
        const COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

        // Truncate output to ~2000 chars to keep prompt size reasonable.
        const MAX_OUTPUT: usize = 2000;
        let truncated_output = if test_output.len() > MAX_OUTPUT {
            format!("{}… (truncated)", &test_output[..MAX_OUTPUT])
        } else {
            test_output.to_string()
        };

        let mut matched: Option<(usize, String)> = None;
        for (i, hook) in self.config.agent.hooks.iter().enumerate() {
            if !hook.enabled || hook.trigger != "on_test_fail" {
                continue;
            }
            if !glob_matches(&hook.glob, &rel_path) {
                continue;
            }
            if let Some(last) = self.hook_cooldowns.get(&i) {
                if now.duration_since(*last) < COOLDOWN {
                    continue;
                }
            }
            let prompt = hook
                .prompt
                .replace("{file}", &rel_path)
                .replace("{output}", &truncated_output);
            matched = Some((i, prompt));
            break;
        }

        let Some((hook_idx, prompt)) = matched else {
            return Ok(());
        };

        self.hook_cooldowns.insert(hook_idx, now);
        self.hooks_firing = true;

        self.agent_panel.visible = true;
        self.agent_panel.messages.push(crate::agent::ChatMessage {
            role: crate::agent::Role::System,
            content: format!("── Hook: on_test_fail → {rel_path} ──"),
            images: Vec::new(),
        });
        self.agent_panel.input = prompt;

        let project_root = cwd;
        let max_rounds = self.config.max_agent_rounds;
        let warning_threshold = self.config.agent_warning_threshold;
        let preferred_model = self.config.active_default_model().to_string();
        let auto_compress = self.config.agent.auto_compress_tool_results;

        let fut = self.agent_panel.submit(
            None,
            project_root,
            max_rounds,
            warning_threshold,
            &preferred_model,
            auto_compress,
        );
        let submit_err = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match fut.await {
                    Ok(()) => None,
                    Err(e) => {
                        tracing::warn!("on_test_fail hook submit error: {e}");
                        Some(e.to_string())
                    },
                }
            })
        });
        if let Some(e) = submit_err {
            self.set_status(format!("Hook error: {e}"));
            self.hooks_firing = false; // reset on submit failure
        }
        // hooks_firing is reset to false when the agent stream completes (Done/Error event).

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
}
