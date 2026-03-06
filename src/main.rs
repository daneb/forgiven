use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod agent;
mod buffer;
mod config;
mod editor;
mod explorer;
mod highlight;
mod keymap;
mod lsp;
mod markdown;
mod mcp;
mod search;
mod ui;

use crate::config::Config;
use crate::editor::Editor;

/// Forgiven — an AI-first terminal code editor
#[derive(Parser, Debug)]
#[command(
    name = "forgiven",
    version,
    about,
    after_help = "EXAMPLES:\n    forgiven                    Open current directory\n    forgiven /path/to/project   Open a project folder\n    forgiven -C ~/work/myapp    Open a project folder (explicit flag)\n    forgiven src/main.rs        Open a specific file"
)]
struct Cli {
    /// Project folder to open (overrides the current directory).
    /// You can also just pass a directory path as a positional argument.
    #[arg(short = 'C', long = "dir", value_name = "DIR")]
    dir: Option<std::path::PathBuf>,

    /// File(s) or directory to open on startup.
    /// If the first positional argument is a directory it is used as the
    /// project root (equivalent to -C).
    files: Vec<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up logging to a file so it doesn't interfere with the TUI
    let log_file = std::fs::File::create("/tmp/forgiven.log")?;
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().with_writer(log_file))
        .init();

    let cli = Cli::parse();

    // -----------------------------------------------------------------------
    // Resolve project root and separate directory args from file args.
    //
    // Supported invocation styles:
    //   forgiven -C /path/to/project          explicit flag
    //   forgiven /path/to/project             positional directory
    //   forgiven /path/to/project file.rs     directory + files
    //   forgiven file.rs                      file(s) in current dir
    // -----------------------------------------------------------------------
    let mut project_dir: Option<std::path::PathBuf> = cli.dir;
    let mut files_to_open: Vec<std::path::PathBuf> = Vec::new();

    for path in cli.files {
        if path.is_dir() {
            // First directory-shaped positional arg becomes the project root.
            // Subsequent directory args are ignored (unusual to pass multiples).
            if project_dir.is_none() {
                project_dir = Some(path);
            }
        } else {
            files_to_open.push(path);
        }
    }

    // Change the process working directory so that every subsequent
    // current_dir() call (LSP root, agent project_root, FileExplorer, etc.)
    // automatically reflects the chosen project.
    if let Some(ref dir) = project_dir {
        let canonical = dir
            .canonicalize()
            .with_context(|| format!("Cannot open directory: {}", dir.display()))?;
        std::env::set_current_dir(&canonical)
            .with_context(|| format!("Cannot change into directory: {}", canonical.display()))?;
        tracing::info!("Project root set to {}", canonical.display());
    }

    tracing::info!("Starting forgiven");

    let config = Config::load();
    let mut editor = Editor::new(config)?;

    // Open any files passed on the command line
    for path in &files_to_open {
        editor.open_file(path)?;
    }

    // Start configured LSP servers (non-fatal if any fail)
    editor.setup_lsp().await;

    // Connect to configured MCP servers (non-fatal if any fail)
    editor.setup_mcp().await;

    editor.run().await?;

    Ok(())
}
