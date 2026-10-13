//! CLI subcommand implementations and shared helpers.

pub mod devices;
pub mod doctor;
pub mod export;
pub mod library;
pub mod models;
pub mod polish;
pub mod transcribe;

use std::path::PathBuf;

use anyhow::bail;
use clap::ValueEnum;
use suramak_core::export as core_export;
use suramak_core::models as manifest;
use suramak_core::paths;
use suramak_core::store::Store;
use suramak_core::transcript::Transcript;

/// Output formats shared by `transcribe` and `export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Md,
    Txt,
    Json,
    Srt,
    Vtt,
}

impl ExportFormat {
    /// File extension for this format.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Md => "md",
            ExportFormat::Txt => "txt",
            ExportFormat::Json => "json",
            ExportFormat::Srt => "srt",
            ExportFormat::Vtt => "vtt",
        }
    }

    /// Renders a transcript in this format via the core exporters.
    pub fn render(self, transcript: &Transcript) -> anyhow::Result<String> {
        let text = match self {
            ExportFormat::Md => core_export::to_markdown(transcript)?,
            ExportFormat::Txt => core_export::to_txt(transcript)?,
            ExportFormat::Json => core_export::to_json(transcript)?,
            ExportFormat::Srt => core_export::to_srt(transcript)?,
            ExportFormat::Vtt => core_export::to_vtt(transcript)?,
        };
        Ok(text)
    }
}

/// Parses a comma-separated format list such as `md,json`.
pub fn parse_formats(spec: &str) -> anyhow::Result<Vec<ExportFormat>> {
    let mut formats = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let format = ExportFormat::from_str(part, true).map_err(|_| {
            anyhow::anyhow!("unknown format '{part}' (expected one of md,txt,json,srt,vtt)")
        })?;
        if !formats.contains(&format) {
            formats.push(format);
        }
    }
    if formats.is_empty() {
        bail!("no export formats requested — pass e.g. --formats md,json");
    }
    Ok(formats)
}

/// Resolves a `--model` spec (manifest id or path to a `.bin` file) to a model path.
pub fn resolve_model(spec: &str) -> anyhow::Result<PathBuf> {
    let as_path = PathBuf::from(spec);
    if as_path.exists() {
        return Ok(as_path);
    }
    if let Some(info) = manifest::find_model(spec) {
        let path = paths::models_dir()?.join(info.hf_file);
        if !path.exists() {
            bail!(
                "model '{spec}' is not installed at {} — run `suramak-cli models download {spec}`",
                path.display()
            );
        }
        return Ok(path);
    }
    bail!(
        "unknown model '{spec}': not a manifest id ({}) and not an existing file",
        model_ids()
    );
}

/// Comma-separated list of manifest model ids.
pub fn model_ids() -> String {
    manifest::MODELS
        .iter()
        .map(|m| m.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Loads a stored transcript by its uuid string (as printed by `library list`).
///
/// Matching is case-insensitive on the uuid text, so the CLI needs no direct uuid
/// dependency (kept to the workorder's dependency list).
pub fn load_transcript(store: &Store, id_spec: &str) -> anyhow::Result<Transcript> {
    let spec = id_spec.trim();
    let id = store
        .list_transcripts()?
        .into_iter()
        .find(|t| t.id.to_string().eq_ignore_ascii_case(spec))
        .map(|t| t.id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no transcript with id '{spec}' in the library (see `suramak-cli library list`)"
            )
        })?;
    let transcript = store.get_transcript(id)?;
    Ok(transcript)
}

/// Human-readable decimal size, e.g. `77.7 MB`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}
