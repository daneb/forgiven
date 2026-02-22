use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod buffer;
mod config;
mod editor;
mod keymap;
mod ui;

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
        .with(EnvFilter::from_default_env())
        .with(fmt::layer().with_writer(log_file))
        .init();

    let cli = Cli::parse();

    tracing::info!("Starting forgiven");

    let mut editor = Editor::new()?;

    // Open any files passed on the command line
    for path in &cli.files {
        editor.open_file(path)?;
    }

    // If no file was given, start with a scratch buffer
    if cli.files.is_empty() {
        editor.open_scratch();
    }

    editor.run().await?;

    Ok(())
}
