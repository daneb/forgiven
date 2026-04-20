//! Codified Context three-tier loader (docs/codified-context.md).
//!
//! Tier 1 — Constitution (.forgiven/constitution.md): always injected.
//! Tier 2 — Specialists (.forgiven/agents/*.md): injected when triggered.
//! Tier 3 — Knowledge  (.forgiven/knowledge/*.md): retrieved via fetch_knowledge().

use std::path::{Path, PathBuf};

use crate::config::CodifiedContextConfig;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

pub struct Constitution {
    pub text: String,
    /// Rough estimate: chars / 4.
    pub token_estimate: usize,
}

pub struct Specialist {
    pub name: String,
    /// Glob patterns from `trigger.paths`.
    pub path_patterns: Vec<String>,
    /// Keywords from `trigger.keywords`.
    pub keywords: Vec<String>,
    pub content: String,
}

pub struct KnowledgeDoc {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

#[allow(dead_code)]
pub struct CodifiedContext {
    pub constitution: Option<Constitution>,
    pub specialists: Vec<Specialist>,
    pub knowledge_docs: Vec<KnowledgeDoc>,
    pub forgiven_dir: PathBuf,
    pub constitution_max_tokens: usize,
    pub max_specialists_per_turn: usize,
    pub knowledge_fetch_max_bytes: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// Loader
// ─────────────────────────────────────────────────────────────────────────────

impl CodifiedContext {
    /// Load from `project_root / config.directory`. Returns `None` when the
    /// directory does not exist (caller should show the tip-line).
    pub fn load(project_root: &Path, config: &CodifiedContextConfig) -> Option<Self> {
        let dir = if Path::new(&config.directory).is_absolute() {
            PathBuf::from(&config.directory)
        } else {
            project_root.join(&config.directory)
        };

        if !dir.is_dir() {
            return None;
        }

        let constitution = load_constitution(&dir);
        let specialists = load_specialists(&dir);
        let knowledge_docs = load_knowledge_docs(&dir);

        Some(CodifiedContext {
            constitution,
            specialists,
            knowledge_docs,
            forgiven_dir: dir,
            constitution_max_tokens: config.constitution_max_tokens,
            max_specialists_per_turn: config.max_specialists_per_turn,
            knowledge_fetch_max_bytes: config.knowledge_fetch_max_bytes,
        })
    }

    /// Build the block to inject into the system prompt.
    ///
    /// `open_file` is the project-relative path of the active buffer (may be empty).
    /// `user_msg` is the raw user message for keyword matching.
    pub fn system_prompt_block(&self, open_file: &str, user_msg: &str) -> String {
        let mut parts: Vec<String> = Vec::new();

        if let Some(ref c) = self.constitution {
            parts.push(format!("## Project Constitution\n\n{}", c.text.trim()));
        }

        let triggered = self.triggered_specialists(open_file, user_msg);
        for spec in triggered {
            parts.push(format!("## {} Specialist\n\n{}", spec.name, spec.content.trim()));
        }

        if !self.knowledge_docs.is_empty() {
            let catalogue = self
                .knowledge_docs
                .iter()
                .map(|d| format!("- {} ({})", d.name, d.description))
                .collect::<Vec<_>>()
                .join("\n");
            parts.push(format!(
                "Knowledge base (call fetch_knowledge(name) to retrieve):\n{catalogue}"
            ));
        }

        if parts.is_empty() {
            return String::new();
        }

        format!("{}\n\n", parts.join("\n\n"))
    }

    /// Return the subset of specialists whose triggers match.
    pub fn triggered_specialists<'a>(
        &'a self,
        open_file: &str,
        user_msg: &str,
    ) -> Vec<&'a Specialist> {
        let user_lower = user_msg.to_lowercase();
        let mut matched: Vec<&Specialist> = self
            .specialists
            .iter()
            .filter(|s| {
                let path_match = !s.path_patterns.is_empty()
                    && s.path_patterns.iter().any(|pat| glob_match(pat, open_file));
                let keyword_count =
                    s.keywords.iter().filter(|kw| user_lower.contains(kw.as_str())).count();
                path_match || keyword_count >= 2
            })
            .collect();
        matched.truncate(self.max_specialists_per_turn);
        matched
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────────

fn load_constitution(dir: &Path) -> Option<Constitution> {
    let path = dir.join("constitution.md");
    let text = std::fs::read_to_string(&path).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let token_estimate = text.len() / 4;
    Some(Constitution { text, token_estimate })
}

fn load_specialists(dir: &Path) -> Vec<Specialist> {
    let agents_dir = dir.join("agents");
    if !agents_dir.is_dir() {
        return Vec::new();
    }
    let mut specialists = Vec::new();
    let Ok(entries) = std::fs::read_dir(&agents_dir) else { return specialists };
    let mut paths: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
        .map(|e| e.path())
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let (path_patterns, keywords, content) = parse_specialist(&raw);
        specialists.push(Specialist { name, path_patterns, keywords, content });
    }
    specialists
}

fn load_knowledge_docs(dir: &Path) -> Vec<KnowledgeDoc> {
    let knowledge_dir = dir.join("knowledge");
    if !knowledge_dir.is_dir() {
        return Vec::new();
    }
    let mut docs = Vec::new();
    let Ok(entries) = std::fs::read_dir(&knowledge_dir) else { return docs };
    let mut paths: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
        .map(|e| e.path())
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
        let description = extract_description(&path);
        docs.push(KnowledgeDoc { name, description, path });
    }
    docs
}

/// Parse a specialist Markdown file: extract path_patterns, keywords, body.
///
/// Frontmatter format (optional):
/// ```
/// ---
/// trigger:
///   paths: ["src/agent/**", "src/mcp_servers/**"]
///   keywords: ["agent", "tool call", "MCP"]
/// ---
/// ```
fn parse_specialist(raw: &str) -> (Vec<String>, Vec<String>, String) {
    if !raw.starts_with("---") {
        return (Vec::new(), Vec::new(), raw.to_string());
    }
    // Find closing ---
    let after_open = &raw[3..];
    let Some(end_pos) = after_open.find("\n---") else {
        return (Vec::new(), Vec::new(), raw.to_string());
    };
    let frontmatter = &after_open[..end_pos];
    let content = after_open[end_pos + 4..].trim_start_matches('\n').to_string();

    let mut path_patterns = Vec::new();
    let mut keywords = Vec::new();
    let mut in_paths = false;
    let mut in_keywords = false;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed == "paths:" || trimmed.starts_with("paths:") {
            in_paths = true;
            in_keywords = false;
            // inline: paths: ["a", "b"]
            if let Some(bracket) = trimmed.find('[') {
                path_patterns.extend(parse_inline_list(&trimmed[bracket..]));
                in_paths = false;
            }
        } else if trimmed == "keywords:" || trimmed.starts_with("keywords:") {
            in_keywords = true;
            in_paths = false;
            if let Some(bracket) = trimmed.find('[') {
                keywords.extend(parse_inline_list(&trimmed[bracket..]));
                in_keywords = false;
            }
        } else if trimmed.starts_with('-') {
            let val = trimmed.trim_start_matches('-').trim().trim_matches('"').to_string();
            if in_paths {
                path_patterns.push(val);
            } else if in_keywords {
                keywords.push(val.to_lowercase());
            }
        } else if !trimmed.starts_with(' ') && !trimmed.is_empty() {
            in_paths = false;
            in_keywords = false;
        }
    }

    (path_patterns, keywords, content)
}

fn parse_inline_list(s: &str) -> Vec<String> {
    let inner = s.trim_matches(|c| c == '[' || c == ']');
    inner
        .split(',')
        .map(|part| part.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Extract a one-line description from a Markdown file.
/// Prefers a `## Summary` section's first line, falling back to the first
/// non-heading, non-empty paragraph line.
fn extract_description(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut in_summary = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("## Summary") || t.starts_with("# Summary") {
            in_summary = true;
            continue;
        }
        if in_summary && !t.is_empty() && !t.starts_with('#') {
            return t.chars().take(100).collect();
        }
        if in_summary && t.starts_with('#') {
            break;
        }
    }
    // Fallback: first non-heading non-empty line
    for line in text.lines() {
        let t = line.trim();
        if !t.is_empty() && !t.starts_with('#') && !t.starts_with("---") {
            return t.chars().take(100).collect();
        }
    }
    String::new()
}

/// Minimal glob matching supporting `*` (any chars, no `/`) and `**` (any chars including `/`).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    glob_match_inner(pattern, path)
}

fn glob_match_inner(pattern: &str, text: &str) -> bool {
    // Double star shortcut first.
    if let Some(pos) = pattern.find("**") {
        let prefix = &pattern[..pos];
        let suffix = &pattern[pos + 2..];
        // prefix must match start of text
        if !text.starts_with(prefix) {
            return false;
        }
        let rest = &text[prefix.len()..];
        // suffix may be empty or start with /
        let suffix = suffix.trim_start_matches('/');
        if suffix.is_empty() {
            return true;
        }
        // Try matching suffix at every position in rest
        for i in 0..=rest.len() {
            if glob_match_inner(suffix, &rest[i..]) {
                return true;
            }
        }
        return false;
    }

    // Single-level matching: split on * within a single path component.
    let mut pat_parts = pattern.splitn(2, '*');
    match (pat_parts.next(), pat_parts.next()) {
        (Some(pre), Some(suf)) => {
            // * matches anything except /
            if !text.starts_with(pre) {
                return false;
            }
            let rest = &text[pre.len()..];
            // Find a non-/ region that ends with suf
            for i in 0..=rest.len() {
                if rest[..i].contains('/') {
                    break;
                }
                if rest[i..].starts_with(suf) && !rest[i + suf.len()..].contains('/') {
                    return true;
                }
            }
            false
        },
        (Some(_literal), None) => pattern == text,
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_star() {
        assert!(glob_match("src/agent/**", "src/agent/panel.rs"));
        assert!(glob_match("src/agent/**", "src/agent/sub/mod.rs"));
        assert!(!glob_match("src/agent/**", "src/editor/mod.rs"));
    }

    #[test]
    fn glob_single_star() {
        assert!(glob_match("src/agent/*.rs", "src/agent/panel.rs"));
        assert!(!glob_match("src/agent/*.rs", "src/agent/sub/panel.rs"));
    }

    #[test]
    fn glob_literal() {
        assert!(glob_match("src/config/mod.rs", "src/config/mod.rs"));
        assert!(!glob_match("src/config/mod.rs", "src/config/other.rs"));
    }

    #[test]
    fn parse_specialist_frontmatter() {
        let raw = "---\ntrigger:\n  paths: [\"src/agent/**\"]\n  keywords: [\"agent\", \"mcp\"]\n---\n# Agent Specialist\n\nContent here.\n";
        let (paths, kws, content) = parse_specialist(raw);
        assert_eq!(paths, vec!["src/agent/**"]);
        assert_eq!(kws, vec!["agent", "mcp"]);
        assert!(content.contains("Content here"));
    }

    #[test]
    fn parse_specialist_no_frontmatter() {
        let raw = "# Simple\n\nNo frontmatter.\n";
        let (paths, kws, content) = parse_specialist(raw);
        assert!(paths.is_empty());
        assert!(kws.is_empty());
        assert!(content.contains("No frontmatter"));
    }
}
