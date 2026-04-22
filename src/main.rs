use anyhow::{Context, Result};
use clap::Parser;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::Level;
use tracing_subscriber::{fmt, prelude::*, EnvFilter, Layer};

/// Tracing layer that captures WARN/ERROR events into a shared ring buffer
/// so the in-app diagnostics panel can display them without the user leaving
/// the editor.
struct RingBufLayer {
    buf: Arc<Mutex<VecDeque<(String, String)>>>,
    capacity: usize,
}

impl<S> Layer<S> for RingBufLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() > Level::WARN {
            return;
        }
        let level = event.metadata().level().to_string();
        // Extract the message field from the event.
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if let Ok(mut buf) = self.buf.lock() {
            if buf.len() >= self.capacity {
                buf.pop_front();
            }
            buf.push_back((level, visitor.0));
        }
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

mod agent;
mod buffer;
mod config;
mod csv_preview;
mod editor;
mod explorer;
mod highlight;
mod insights;
mod json_preview;
mod keymap;
mod lsp;
mod markdown;
mod mcp;
mod search;
mod spec_framework;
mod treesitter;
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
    // Set up logging to a persistent file so it doesn't interfere with the TUI
    // and survives across restarts.  Fall back to /tmp if HOME is unavailable.
    // Also install a ring-buffer layer so the in-app diagnostics panel can
    // display recent WARN/ERROR events without the user leaving the editor.
    let log_path =
        Config::log_path().unwrap_or_else(|| std::path::PathBuf::from("/tmp/forgiven.log"));
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let log_file = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;
    let log_buf: Arc<Mutex<VecDeque<(String, String)>>> = Arc::new(Mutex::new(VecDeque::new()));
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().with_writer(log_file))
        .with(RingBufLayer { buf: Arc::clone(&log_buf), capacity: 50 })
        .init();

    // Log panics to the tracing file so crashes leave a trace even though the
    // Drop impl restores the terminal (making panics look like clean exits).
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };
        tracing::error!("PANIC at {location}: {msg}");
    }));

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

    let t0 = Instant::now();

    let config = Config::load();
    let mut editor = Editor::new(config.clone())?;
    editor.log_buffer = log_buf;

    // Open any files passed on the command line
    for path in &files_to_open {
        editor.open_file(path)?;
    }

    // Start LSP + MCP servers concurrently (each group also starts its members in parallel).
    editor.render_loading("starting services…")?;
    editor.setup_services().await;

    // Pre-warm the Ollama model into RAM so the user's first message is fast.
    // Runs in the background — startup is not blocked on model load completion.
    // Pre-warm the Ollama model into RAM so the user's first message is fast.
    // Runs in the background — startup is not blocked on model load completion.
    if config.provider.active == "ollama" {
        let base_url = config.provider.ollama.base_url.clone();
        let model = config.provider.ollama.default_model.clone();
        tracing::info!("[ollama] background warmup started for model={model:?}");
        tokio::spawn(crate::agent::provider::warmup_ollama(base_url, model));
    }

    editor.startup_elapsed = Some(t0.elapsed());
    tracing::info!("startup: total ready in {}ms", t0.elapsed().as_millis());

    editor.run().await?;

    Ok(())
}
