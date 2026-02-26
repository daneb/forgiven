use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::process::Command;

// ── Result types ───────────────────────────────────────────────────────────────

/// One line matched by ripgrep.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Absolute path to the matching file.
    pub path: PathBuf,
    /// Path relative to the search root (for display).
    pub rel_path: String,
    /// 0-indexed line number.
    pub line: usize,
    /// 0-indexed column.
    pub col: usize,
    /// Content of the matched line (trailing whitespace stripped).
    pub text: String,
}

// ── Panel state ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SearchFocus {
    Query,
    Glob,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchStatus {
    Idle,
    Running,
    Done,
    Error(String),
}

#[derive(Debug)]
pub struct SearchState {
    pub query: String,
    /// Optional glob pattern, e.g. `*.rs` or `src/**/*.ts`.
    pub glob: String,
    pub results: Vec<SearchResult>,
    pub selected: usize,
    pub focus: SearchFocus,
    pub status: SearchStatus,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            glob: String::new(),
            results: Vec::new(),
            selected: 0,
            focus: SearchFocus::Query,
            status: SearchStatus::Idle,
        }
    }

    pub fn select_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_down(&mut self) {
        if self.selected + 1 < self.results.len() {
            self.selected += 1;
        }
    }

    pub fn selected_result(&self) -> Option<&SearchResult> {
        self.results.get(self.selected)
    }

    pub fn set_results(&mut self, results: Vec<SearchResult>) {
        // Clamp selection so it remains valid after a new search.
        if self.selected >= results.len() {
            self.selected = 0;
        }
        self.results = results;
        self.status = SearchStatus::Done;
    }
}

// ── ripgrep invocation ─────────────────────────────────────────────────────────

/// Run ripgrep in `cwd` for `query`, optionally filtered by `glob`.
/// Returns up to 500 results.  Uses the login shell so npm/brew paths work.
pub async fn run_search(query: &str, glob: &str, cwd: &Path) -> Result<Vec<SearchResult>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    // Build the rg command string.
    //
    // IMPORTANT: all --glob=... values are wrapped in single quotes so that the
    // login shell (typically zsh on macOS) does NOT expand the `*` wildcards or
    // treat `!` as a history or extended-glob prefix before the arguments reach
    // rg.  Without quotes, `--glob=!.git/**` would fail in zsh with
    // "no matches found: --glob=!.git/**" causing exit code 1 — silently
    // swallowed as "rg found no matches".
    let mut parts: Vec<String> = vec![
        "rg".to_string(),
        "--line-number".to_string(),
        "--column".to_string(),
        "--no-heading".to_string(),
        "--color=never".to_string(),
        "--smart-case".to_string(),
        "--max-filesize=1M".to_string(),
        // Noise-dir exclusions — single-quoted so the shell never expands them.
        "--glob='!.git/**'".to_string(),
        "--glob='!target/**'".to_string(),       // Rust / Maven
        "--glob='!node_modules/**'".to_string(),
        "--glob='!dist/**'".to_string(),
        "--glob='!build/**'".to_string(),
        "--glob='!obj/**'".to_string(),          // .NET intermediate output
        "--glob='!bin/**'".to_string(),          // .NET / generic build output
        "--glob='!*.lock'".to_string(),
    ];

    // User-supplied file filter — single-quote the value to prevent shell expansion.
    if !glob.trim().is_empty() {
        let g = glob.trim().replace('\'', "'\\''");
        parts.push(format!("--glob='{}'", g));
    }

    // Shell-escape the query.
    let q = query.replace('\'', "'\\''");
    parts.push(format!("'{}'", q));
    parts.push(".".to_string());

    let cmd = parts.join(" ");
    tracing::info!("search: {} -l -c \"{}\"", shell, cmd);

    let out = Command::new(&shell)
        .args(["-l", "-c", &cmd])
        .current_dir(cwd)
        .output()
        .await?;

    // rg exit codes:
    //   0  — one or more matches found
    //   1  — no matches (not an error)
    //   2  — usage/pattern error
    //   127 — command not found (rg not installed, or not in login shell PATH)
    //   other — shell failed before rg ran (e.g. zsh glob expansion error)
    //
    // Only 0 and 1 are "normal" outcomes.  Everything else is surfaced as an
    // error so the user sees it in the results panel title.
    match out.status.code() {
        Some(0) | Some(1) => {} // normal rg outcomes
        code => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!(
                "rg failed (exit {}): {}",
                code.map(|c| c.to_string()).unwrap_or_else(|| "signal".to_string()),
                stderr.trim()
            );
        }
    }

    let raw = String::from_utf8_lossy(&out.stdout);
    let results = raw
        .lines()
        .take(500)
        .filter_map(|l| parse_rg_line(l, cwd))
        .collect();

    Ok(results)
}

/// Parse one line of `rg --line-number --column --no-heading` output.
/// Format: `path:line:col:content`
fn parse_rg_line(line: &str, cwd: &Path) -> Option<SearchResult> {
    let mut parts = line.splitn(4, ':');
    let path_str = parts.next()?;
    let line_num: usize = parts.next()?.trim().parse().ok()?;
    let col: usize = parts.next()?.trim().parse().ok()?;
    let text = parts.next().unwrap_or("").trim_end().to_string();

    Some(SearchResult {
        path: cwd.join(path_str),
        rel_path: path_str.to_string(),
        line: line_num.saturating_sub(1), // convert to 0-indexed
        col: col.saturating_sub(1),
        text,
    })
}
