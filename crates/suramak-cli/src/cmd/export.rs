//! `export <id> --format F --out FILE`

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use suramak_core::paths;
use suramak_core::store::Store;

use crate::cmd::{load_transcript, ExportFormat};

#[derive(Args)]
pub struct ExportArgs {
    /// Transcript id (uuid) from the library — see `suramak-cli library list`
    pub id: String,

    /// Output format
    #[arg(long, value_enum)]
    pub format: ExportFormat,

    /// Output file path
    #[arg(long)]
    pub out: PathBuf,
}

pub fn run(args: ExportArgs) -> Result<()> {
    let store = Store::open(&paths::db_path()?)?;
    let transcript = load_transcript(&store, &args.id)?;

    let text = args.format.render(&transcript)?;
    if let Some(parent) = args.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
    }
    std::fs::write(&args.out, text)
        .with_context(|| format!("cannot write {}", args.out.display()))?;
    println!(
        "wrote {} ({} segments, {})",
        args.out.display(),
        transcript.segments.len(),
        args.format.extension()
    );
    Ok(())
}
