//! suramak-cli — headless CLI for the Suramak transcription pipeline.

mod cmd;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "suramak-cli",
    version,
    about = "Suramak — fully-local audio transcription (CLI)",
    long_about = "Suramak CLI: transcribe audio files locally with whisper.cpp.\n\
                  Audio never leaves the machine; saving transcripts to the library is \
                  OFF by default and enabled per-run with --db."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage whisper models: list, download (sha256-verified), remove
    #[command(subcommand)]
    Models(cmd::models::ModelsCmd),
    /// Transcribe an audio file and write export files
    Transcribe(cmd::transcribe::TranscribeArgs),
    /// Library operations
    #[command(subcommand)]
    Library(cmd::library::LibraryCmd),
    /// Export a stored transcript to a file
    Export(cmd::export::ExportArgs),
    /// Polish a stored transcript with an OpenAI-compatible LLM
    Polish(cmd::polish::PolishArgs),
    /// Check data dir, database, models and settings (creates missing dirs)
    Doctor,
    /// List input audio devices (requires a build with the `capture` feature)
    Devices,
}

fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Models(cmd) => cmd::models::run(cmd),
        Command::Transcribe(args) => cmd::transcribe::run(args),
        Command::Library(cmd) => cmd::library::run(cmd),
        Command::Export(args) => cmd::export::run(args),
        Command::Polish(args) => cmd::polish::run(args),
        Command::Doctor => cmd::doctor::run(),
        Command::Devices => cmd::devices::run(),
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
