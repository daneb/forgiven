use anyhow::Result;
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
mod ui;

use crate::config::Config;
use crate::editor::Editor;

/// Forgiven — an AI-first terminal code editor
#[derive(Parser, Debug)]
#[command(name = "forgiven", version, about)]
struct Cli {
    /// File(s) to open on startup
    files: Vec<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up logging to a file so it doesn't interfere with the TUI
    let log_file = std::fs::File::create("/tmp/forgiven.log")?;
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with(fmt::layer().with_writer(log_file))
        .init();

    let cli = Cli::parse();

    tracing::info!("Starting forgiven");

    let config = Config::load();
    let mut editor = Editor::new()?;

    // Open any files passed on the command line
    for path in &cli.files {
        editor.open_file(path)?;
    }

    // If no file was given, start with a scratch buffer
    if cli.files.is_empty() {
        editor.open_scratch();
    }

    // Start configured LSP servers (non-fatal if any fail)
    editor.setup_lsp(&config).await;

    editor.run().await?;

    Ok(())
}
