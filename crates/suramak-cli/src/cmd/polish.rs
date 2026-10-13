//! `polish <id> [--base-url U] [--model M]`
//!
//! Sends the stored segments to an OpenAI-compatible `/chat/completions` endpoint,
//! validates the response and upserts the polished texts back into the library.
//! Requires `SURAMAK_OPENAI_API_KEY` on Linux (keychain on macOS/Windows).

use anyhow::{bail, Result};
use clap::Args;
use suramak_core::paths;
use suramak_core::polish::{OpenAiCompat, PolishBackend};
use suramak_core::settings::{PolishSettings, Settings};
use suramak_core::store::Store;

use crate::cmd::load_transcript;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-4o-mini";

#[derive(Args)]
pub struct PolishArgs {
    /// Transcript id (uuid) from the library — see `suramak-cli library list`
    pub id: String,

    /// OpenAI-compatible base URL (default: settings.toml, else OpenAI)
    #[arg(long)]
    pub base_url: Option<String>,

    /// Model name (default: settings.toml, else gpt-4o-mini)
    #[arg(long)]
    pub model: Option<String>,
}

pub fn run(args: PolishArgs) -> Result<()> {
    let store = Store::open(&paths::db_path()?)?;
    let transcript = load_transcript(&store, &args.id)?;
    if transcript.segments.is_empty() {
        bail!("transcript {} has no segments to polish", transcript.id);
    }

    // Flags beat settings.toml, which beats the built-in defaults.
    let settings = Settings::load(&paths::settings_path()?)?;
    let base_url = args
        .base_url
        .or_else(|| settings.polish.as_ref().map(|p| p.base_url.clone()))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let model = args
        .model
        .or_else(|| settings.polish.as_ref().map(|p| p.model.clone()))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let cfg = PolishSettings {
        base_url: base_url.clone(),
        model: model.clone(),
    };
    let backend = OpenAiCompat::new()?;

    println!(
        "polishing {} segment(s) with '{}' at {}",
        transcript.segments.len(),
        model,
        base_url
    );
    let polished = backend.polish(&transcript.segments, &cfg)?;
    store.upsert_segments(transcript.id, &polished)?;
    println!(
        "polished {} segment(s) and saved to the library",
        polished.len()
    );
    Ok(())
}
