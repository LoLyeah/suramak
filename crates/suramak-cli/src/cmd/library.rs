//! `library list`

use anyhow::Result;
use clap::Subcommand;
use suramak_core::paths;
use suramak_core::store::Store;

#[derive(Subcommand)]
pub enum LibraryCmd {
    /// List stored transcripts (newest first)
    List,
}

pub fn run(cmd: LibraryCmd) -> Result<()> {
    match cmd {
        LibraryCmd::List => list(),
    }
}

fn list() -> Result<()> {
    let db = paths::db_path()?;
    let store = Store::open(&db)?;
    let transcripts = store.list_transcripts()?;

    if transcripts.is_empty() {
        println!("no transcripts in the library ({})", db.display());
        return Ok(());
    }
    println!("{} transcript(s) in {}", transcripts.len(), db.display());
    println!(
        "{:<38} {:<17} {:<14} {:<6} TITLE",
        "ID", "CREATED (UTC)", "MODEL", "LANG"
    );
    for t in transcripts {
        println!(
            "{:<38} {:<17} {:<14} {:<6} {}",
            t.id,
            t.created_at.format("%Y-%m-%d %H:%M"),
            t.model,
            t.language.as_deref().unwrap_or("auto"),
            t.title
        );
    }
    Ok(())
}
