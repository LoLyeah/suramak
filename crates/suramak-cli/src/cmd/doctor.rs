//! `doctor` — prints data dir, db path, models dir (with installed models), settings
//! path and the engine stack (compiled backends, Core ML availability, encoder
//! pairing, and the M10A Parakeet fast engine); creates missing directories; always
//! exits 0 on success.

use anyhow::{Context, Result};
use suramak_asr::{coreml_compiled, metal_compiled, system_info};
use suramak_core::models;
use suramak_core::paths;
use suramak_core::settings::Settings;

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

/// The engine stack: compiled whisper.cpp backends, the Core ML tier, encoder
/// pairing, and the M10A Parakeet fast engine (compiled in / installed / coverage).
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
            "coreml (when the model's encoder package is installed) → metal → cpu"
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

    print_parakeet(models_dir);
    print_speech_transcriber();
}

/// The "Fast engine (experimental)" setting (OFF when settings are missing).
fn fast_engine_setting() -> bool {
    paths::settings_path()
        .ok()
        .and_then(|path| Settings::load(&path).ok())
        .map(|settings| settings.asr.fast_engine)
        .unwrap_or(false)
}

/// The configured default language code (for the SpeechTranscriber asset probe;
/// `auto` falls back to `en`).
fn default_language_code() -> String {
    paths::settings_path()
        .ok()
        .and_then(|path| Settings::load(&path).ok())
        .map(|settings| match settings.asr.language {
            suramak_core::settings::LangMode::Fixed(code) => code,
            suramak_core::settings::LangMode::Auto => "en".to_string(),
        })
        .unwrap_or_else(|| "en".to_string())
}

/// M10A: the Parakeet fast engine — compiled in? model installed + layout ok?
/// language coverage; and whether the settings toggle is on.
fn print_parakeet(models_dir: &std::path::Path) {
    let info = &models::PARAKEET;
    let dir = models::parakeet_dir(models_dir);
    let installed = models::parakeet_installed(models_dir);
    let total: u64 = info.files.iter().map(|f| f.size_bytes).sum();
    let compiled = suramak_asr::parakeet::compiled();

    println!("  parakeet (fast engine, experimental):");
    println!(
        "    build:      {}",
        if compiled {
            "compiled in (sherpa-onnx linked; `parakeet` feature on)".to_string()
        } else {
            "not compiled into this build (feature `parakeet` is off — rebuild with it to use the fast engine)"
                .to_string()
        }
    );
    println!(
        "    provider:   {}",
        if cfg!(target_os = "macos") {
            "coreml (Core ML execution provider on Apple silicon; sherpa-onnx falls back to cpu when a node cannot run there)"
        } else {
            "cpu"
        }
    );
    if installed {
        println!(
            "    model:      installed at {} ({} files, {}, layout ok)",
            dir.display(),
            info.files.len(),
            human_size(total)
        );
    } else if dir.exists() {
        let present = info
            .files
            .iter()
            .filter(|f| dir.join(f.name).is_file())
            .count();
        println!(
            "    model:      PARTIAL at {} ({} of {} files) — run `suramak-cli models download parakeet`",
            dir.display(),
            present,
            info.files.len()
        );
    } else {
        println!(
            "    model:      not installed — run `suramak-cli models download parakeet` ({}, {} files)",
            human_size(total),
            info.files.len()
        );
    }
    println!(
        "    coverage:   {} languages ({})",
        info.languages.len(),
        info.languages.join(", ")
    );
    println!("    invariant:  Indonesian (id) never routes to Parakeet — whisper only");

    let fast_setting = fast_engine_setting();
    println!(
        "    setting:    fast_engine = {} ({})",
        fast_setting,
        if fast_setting {
            "covered languages use Parakeet when installed; everything else stays whisper"
        } else {
            "OFF (default) — today's whisper routing for every language"
        }
    );
}

/// M10B: Apple SpeechTranscriber — the macOS fast path. Build / OS availability /
/// locales / authorization / the locale asset / the routing order. The assets are
/// **OS-managed**: there are deliberately no `models` entries (nothing is pinned,
/// downloaded or hosted by this app — the system ships and auto-updates them).
fn print_speech_transcriber() {
    use suramak_asr::speech_transcriber as st;

    let macos = cfg!(target_os = "macos");
    let compiled = st::compiled();
    let probed = macos && compiled;

    println!("  speech-transcriber (fast engine, experimental):");
    println!(
        "    build:      {}",
        if !macos {
            "not available on this platform (Apple SpeechTranscriber is macOS-only; \
             the `speech` crate is a Swift bridge)"
                .to_string()
        } else if !compiled {
            "not compiled into this build (feature `speech-transcriber` is off — \
             rebuild with it on macOS 26+)"
                .to_string()
        } else {
            "compiled in (feature `speech-transcriber` on; `speech` crate / Swift bridge linked)"
                .to_string()
        }
    );
    if probed {
        println!(
            "    os:         {}",
            if st::os_available() {
                "macOS 26+ SpeechTranscriber available".to_string()
            } else {
                "unavailable on this macOS version (needs macOS 26+)".to_string()
            }
        );
        if let Some(auth) = st::authorization() {
            println!("    auth:       {}", auth.label());
        }
    } else {
        println!(
            "    os:         not probed ({})",
            if macos { "feature off" } else { "macOS-only" }
        );
    }
    match st::supported_locales() {
        Ok(locales) => println!(
            "    locales:    {} supported by this OS ({})",
            locales.len(),
            locales.join(", ")
        ),
        Err(_) if !probed => println!(
            "    locales:    not probed ({})",
            if macos { "feature off" } else { "macOS-only" }
        ),
        Err(err) => println!("    locales:    unavailable ({err})"),
    }
    println!(
        "    coverage:   {} languages pinned ({})",
        models::SPEECH_TRANSCRIBER_LANGUAGES.len(),
        models::SPEECH_TRANSCRIBER_LANGUAGES.join(", ")
    );
    println!("    invariant:  Indonesian (id) never routes to SpeechTranscriber — whisper only");

    let language = default_language_code();
    match st::asset_status(&language) {
        Ok(state) => println!(
            "    asset:      {} (locale for `{language}`)",
            state.label()
        ),
        Err(_) if !probed => println!(
            "    asset:      not probed ({})",
            if macos { "feature off" } else { "macOS-only" }
        ),
        Err(err) => println!("    asset:      unavailable ({err})"),
    }
    println!(
        "    assets:     OS-managed — no `models` entries (macOS ships and auto-updates \
         the locale models via AssetInventory; nothing is pinned or downloaded here)"
    );
    println!(
        "    routing:    Fast engine ON + Auto: macOS + covered + available → \
         speech-transcriber, else parakeet, else whisper (the warning names the \
         blocker); explicit coreml/metal/cpu stays whisper"
    );

    let fast_setting = fast_engine_setting();
    println!(
        "    setting:    fast_engine = {} ({})",
        fast_setting,
        if fast_setting {
            "covered languages use the fast engines in order (macOS: SpeechTranscriber → \
             Parakeet → Whisper); everything else stays whisper"
        } else {
            "OFF (default) — today's whisper routing for every language"
        }
    );
}
