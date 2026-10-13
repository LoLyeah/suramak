//! `models list | download <id> | remove <id>`

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use suramak_core::models;
use suramak_core::{download, paths};

use crate::cmd::{human_size, model_ids};

#[derive(Subcommand)]
pub enum ModelsCmd {
    /// List known models and show which ones are installed
    List,
    /// Download a model from HuggingFace and verify its pinned sha256
    Download {
        /// Model id: tiny, base, small, turbo-q5-0, turbo-q8-0, turbo
        id: String,
    },
    /// Delete an installed model file
    Remove {
        /// Model id: tiny, base, small, turbo-q5-0, turbo-q8-0, turbo
        id: String,
    },
}

pub fn run(cmd: ModelsCmd) -> Result<()> {
    match cmd {
        ModelsCmd::List => list(),
        ModelsCmd::Download { id } => download_model(&id),
        ModelsCmd::Remove { id } => remove(&id),
    }
}

fn list() -> Result<()> {
    let dir = paths::models_dir()?;
    println!("models dir: {}", dir.display());
    println!("{:<12} {:>10}  {:<10} ROLE", "ID", "SIZE", "INSTALLED");
    for model in models::MODELS {
        let installed = dir.join(model.hf_file).exists();
        println!(
            "{:<12} {:>10}  {:<10} {}",
            model.id,
            human_size(model.size_bytes),
            if installed { "yes" } else { "-" },
            model.role
        );
    }
    Ok(())
}

fn download_model(id: &str) -> Result<()> {
    let info = find_model(id)?;
    paths::ensure_dirs()?;
    let dest = paths::models_dir()?.join(info.hf_file);

    if dest.exists() {
        download::verify_sha256(&dest, info.sha256).with_context(|| {
            format!(
                "existing file {} failed sha256 verification — delete it and retry",
                dest.display()
            )
        })?;
        println!(
            "{id} already installed (sha256 {}… verified): {}",
            &info.sha256[..12],
            dest.display()
        );
        return Ok(());
    }

    let url = models::model_url(info);
    println!("downloading {id} ({})", human_size(info.size_bytes));
    println!("  from {}", url);
    println!("  to   {}", dest.display());
    download::download(&url, &dest, info.sha256)?;
    // Re-verify the final file (post-rename) so the success claim below is airtight.
    download::verify_sha256(&dest, info.sha256)?;
    println!(
        "{id} downloaded, sha256 verified ({}): {}",
        human_size(info.size_bytes),
        dest.display()
    );
    Ok(())
}

fn remove(id: &str) -> Result<()> {
    let info = find_model(id)?;
    let dest = paths::models_dir()?.join(info.hf_file);

    let mut removed_any = false;
    for path in [dest.clone(), part_path(&dest)] {
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("cannot remove {}", path.display()))?;
            println!("removed {}", path.display());
            removed_any = true;
        }
    }
    if !removed_any {
        println!("{id} is not installed ({})", dest.display());
    }
    Ok(())
}

fn find_model(id: &str) -> Result<&'static models::ModelInfo> {
    models::find_model(id)
        .ok_or_else(|| anyhow::anyhow!("unknown model '{id}' (available: {})", model_ids()))
}

/// `<dest>.part` — the same temporary path the core downloader uses.
fn part_path(dest: &Path) -> PathBuf {
    let mut raw = dest.as_os_str().to_owned();
    raw.push(".part");
    PathBuf::from(raw)
}
