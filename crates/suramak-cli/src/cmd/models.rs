//! `models list | download <id> [--encoder] | remove <id>`
//!
//! `download --encoder` fetches the official Core ML encoder package
//! (`ggml-<model>-encoder.mlmodelc.zip`), sha256-verifies it and extracts it next to the
//! model file as `<model-stem>-encoder.mlmodelc` — the exact directory whisper.cpp looks
//! for. See `docs/ENGINES.md`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use suramak_core::models;
use suramak_core::{download, paths};

use crate::cmd::{human_size, model_ids};

#[derive(Subcommand)]
pub enum ModelsCmd {
    /// List known models and show which ones are installed (and their encoder packages)
    List,
    /// Download a model from HuggingFace and verify its pinned sha256
    Download {
        /// Model id: tiny, base, small, turbo-q5-0, turbo-q8-0, turbo
        id: String,
        /// Also download + extract the official Core ML encoder package (ANE tier)
        #[arg(long)]
        encoder: bool,
    },
    /// Delete an installed model file (and its Core ML encoder package when unshared)
    Remove {
        /// Model id: tiny, base, small, turbo-q5-0, turbo-q8-0, turbo
        id: String,
    },
}

pub fn run(cmd: ModelsCmd) -> Result<()> {
    match cmd {
        ModelsCmd::List => list(),
        ModelsCmd::Download { id, encoder } => download_model(&id, encoder),
        ModelsCmd::Remove { id } => remove(&id),
    }
}

fn list() -> Result<()> {
    let dir = paths::models_dir()?;
    println!("models dir: {}", dir.display());
    println!(
        "{:<12} {:>10}  {:<10} {:<12} ROLE",
        "ID", "SIZE", "INSTALLED", "ENCODER"
    );
    for model in models::MODELS {
        let installed = dir.join(model.hf_file).exists();
        let encoder = match model.encoder {
            Some(package) => {
                let ready =
                    models::encoder_dir_ready(&dir.join(models::encoder_dir_name(model.hf_file)));
                if ready {
                    "yes".to_string()
                } else if dir.join(package.zip_file).exists() {
                    "partial".to_string()
                } else {
                    "-".to_string()
                }
            }
            None => "n/a".to_string(),
        };
        println!(
            "{:<12} {:>10}  {:<10} {:<12} {}",
            model.id,
            human_size(model.size_bytes),
            if installed { "yes" } else { "-" },
            encoder,
            model.role
        );
    }
    println!(
        "encoder packages: `models download <id> --encoder` (shared by the three turbo decoders)"
    );
    Ok(())
}

fn download_model(id: &str, with_encoder: bool) -> Result<()> {
    let info = find_model(id)?;
    paths::ensure_dirs()?;
    let models_dir = paths::models_dir()?;
    let dest = models_dir.join(info.hf_file);

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
    } else {
        let url = models::model_url(info);
        println!("downloading {id} ({})", human_size(info.size_bytes));
        println!("  from {url}");
        println!("  to   {}", dest.display());
        download::download(&url, &dest, info.sha256)?;
        // Re-verify the final file (post-rename) so the success claim below is airtight.
        download::verify_sha256(&dest, info.sha256)?;
        println!(
            "{id} downloaded, sha256 verified ({}): {}",
            human_size(info.size_bytes),
            dest.display()
        );
    }

    if with_encoder {
        download_encoder(info, &models_dir)?;
    }
    Ok(())
}

/// Downloads + extracts the Core ML encoder package for `info` (idempotent).
fn download_encoder(info: &'static models::ModelInfo, models_dir: &Path) -> Result<()> {
    let package = info.encoder.ok_or_else(|| {
        anyhow::anyhow!(
            "model '{}' has no official Core ML encoder package upstream",
            info.id
        )
    })?;
    // whisper.cpp's lookup key: derived from the model file name, not the manifest id.
    let dir_name = models::encoder_dir_name(info.hf_file);
    let dest = models_dir.join(&dir_name);
    if models::encoder_dir_ready(&dest) {
        println!(
            "encoder already installed (sha256 {}… verified): {}",
            &package.sha256[..12],
            dest.display()
        );
        return Ok(());
    }

    let url = models::encoder_url(&package);
    println!(
        "downloading encoder package {} ({})",
        package.zip_file,
        human_size(package.size_bytes)
    );
    println!("  from {url}");
    println!("  to   {}", dest.display());
    let extracted = download::download_zip(
        &url,
        models_dir,
        package.zip_file,
        &dir_name,
        package.sha256,
        models::ENCODER_REQUIRED_FILES,
    )?;
    // Re-check the extracted layout so the success claim below is airtight.
    if !models::encoder_dir_ready(&extracted) {
        anyhow::bail!(
            "extracted {} but the encoder package is incomplete",
            extracted.display()
        );
    }
    println!(
        "encoder downloaded, sha256 verified ({}), extracted to {}",
        human_size(package.size_bytes),
        extracted.display()
    );
    println!("  whisper.cpp will pair it with the model file automatically on macOS");
    Ok(())
}

fn remove(id: &str) -> Result<()> {
    let info = find_model(id)?;
    let models_dir = paths::models_dir()?;
    let dest = models_dir.join(info.hf_file);

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

    remove_encoder_if_unshared(info, &models_dir)?;
    Ok(())
}

/// Removes the model's encoder directory unless another *installed* model pairs with it.
///
/// All three turbo decoders share `ggml-large-v3-turbo-encoder.mlmodelc`, so the package
/// only goes when the last decoder that needs it goes.
fn remove_encoder_if_unshared(info: &'static models::ModelInfo, models_dir: &Path) -> Result<()> {
    let dir_name = models::encoder_dir_name(info.hf_file);
    let dir = models_dir.join(&dir_name);
    if !dir.exists() {
        return Ok(());
    }
    let shared_by: Vec<&str> = models::MODELS
        .iter()
        .filter(|other| other.id != info.id)
        .filter(|other| models::encoder_dir_name(other.hf_file) == dir_name)
        .filter(|other| models_dir.join(other.hf_file).exists())
        .map(|other| other.id)
        .collect();
    if !shared_by.is_empty() {
        println!(
            "kept {} (still used by: {})",
            dir.display(),
            shared_by.join(", ")
        );
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("cannot remove {}", dir.display()))?;
    println!("removed {}", dir.display());
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
