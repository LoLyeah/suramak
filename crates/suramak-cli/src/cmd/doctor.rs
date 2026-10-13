//! `doctor` — prints data dir, db path, models dir (with installed models) and
//! settings path; creates missing directories; always exits 0 on success.

use anyhow::{Context, Result};
use suramak_core::models;
use suramak_core::paths;

use crate::cmd::human_size;

pub fn run() -> Result<()> {
    let data = paths::data_dir()?;
    paths::ensure_dirs().context("cannot create the data directory")?;
    let models_dir = paths::models_dir()?;
    let db = paths::db_path()?;
    let settings = paths::settings_path()?;

    println!("suramak doctor");
    println!("  data dir:   {}", data.display());
    println!(
        "  db:         {} {}",
        db.display(),
        if db.exists() {
            "(exists)"
        } else {
            "(not created yet)"
        }
    );
    println!("  models dir: {}", models_dir.display());
    println!(
        "  settings:   {} {}",
        settings.display(),
        if settings.exists() {
            "(exists)"
        } else {
            "(built-in defaults)"
        }
    );

    println!("installed models:");
    let mut installed = 0usize;
    if models_dir.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(&models_dir)
            .with_context(|| format!("cannot read {}", models_dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                continue;
            }
            installed += 1;
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            match models::MODELS.iter().find(|m| m.hf_file == file_name) {
                Some(info) if size == info.size_bytes => {
                    println!(
                        "  - {} ({}, id {}, size ok)",
                        path.display(),
                        human_size(size),
                        info.id
                    );
                }
                Some(info) => {
                    println!(
                        "  - {} ({}, id {}, SIZE MISMATCH: expected {} bytes)",
                        path.display(),
                        human_size(size),
                        info.id,
                        info.size_bytes
                    );
                }
                None => {
                    println!(
                        "  - {} ({}, unrecognized filename)",
                        path.display(),
                        human_size(size)
                    );
                }
            }
        }
    }
    if installed == 0 {
        println!("  (none — run `suramak-cli models download tiny`)");
    }
    println!("ok");
    Ok(())
}
