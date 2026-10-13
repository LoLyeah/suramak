//! `doctor` — prints data dir, db path, models dir (with installed models), settings
//! path and the engine stack (compiled backends, Core ML availability, encoder
//! pairing); creates missing directories; always exits 0 on success.

use anyhow::{Context, Result};
use suramak_asr::{coreml_compiled, metal_compiled, system_info};
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

    print_engines(&models_dir);
    println!("ok");
    Ok(())
}

/// The engine stack: compiled whisper.cpp backends, the Core ML tier and encoder pairing.
fn print_engines(models_dir: &std::path::Path) {
    let macos = cfg!(target_os = "macos");
    println!("engines:");
    let info = system_info();
    println!(
        "  whisper.cpp: {}",
        if info.is_empty() {
            "(unavailable)"
        } else {
            info.trim()
        }
    );
    println!(
        "  coreml:      {}",
        if !macos {
            "not available on this platform (Core ML is macOS-only)".to_string()
        } else if !coreml_compiled() {
            "not available (built without the `coreml` feature)".to_string()
        } else {
            "available (whisper.cpp compiled with Core ML; ANE encoder packages pair per model)"
                .to_string()
        }
    );
    println!(
        "  metal:       {}",
        if !macos {
            "not available on this platform (Metal is macOS-only)".to_string()
        } else if !metal_compiled() {
            "not available (built without the `metal` feature)".to_string()
        } else {
            "available (whisper.cpp compiled with Metal)".to_string()
        }
    );
    println!(
        "  auto order:  {}",
        if macos {
            "coreml (encoder installed) → metal → cpu"
        } else {
            "cpu"
        }
    );

    println!("  encoder pairing:");
    let mut paired_any = false;
    let mut known_dirs: Vec<String> = Vec::new();
    if models_dir.exists() {
        for info in models::MODELS {
            let model_path = models_dir.join(info.hf_file);
            if !model_path.exists() {
                continue;
            }
            paired_any = true;
            let dir_name = models::encoder_dir_name(info.hf_file);
            known_dirs.push(dir_name.clone());
            let dir = models_dir.join(&dir_name);
            if models::encoder_dir_ready(&dir) {
                println!("    - {} (id {}): ok ({})", info.hf_file, info.id, dir_name);
            } else if let Some(package) = info.encoder {
                if models_dir.join(package.zip_file).exists() {
                    println!(
                        "    - {} (id {}): archive present but not extracted ({})",
                        info.hf_file, info.id, package.zip_file
                    );
                } else {
                    println!(
                        "    - {} (id {}): MISSING — run `suramak-cli models download {} --encoder`",
                        info.hf_file, info.id, info.id
                    );
                }
            } else {
                println!(
                    "    - {} (id {}): no official encoder package upstream",
                    info.hf_file, info.id
                );
            }
        }
    }
    if !paired_any {
        println!("    (no models installed)");
    }

    // Encoder directories that no installed model pairs with (leftovers, renamed files).
    if let Ok(entries) = std::fs::read_dir(models_dir) {
        let mut orphans: Vec<String> = Vec::new();
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with("-encoder.mlmodelc") {
                continue;
            }
            if !known_dirs.contains(&name) {
                orphans.push(name);
            }
        }
        orphans.sort();
        for orphan in orphans {
            println!("    - {orphan}: unpaired (no installed model looks for this directory)");
        }
    }
}
