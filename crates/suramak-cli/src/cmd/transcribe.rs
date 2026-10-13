//! `transcribe <input> [--model id|path] [--lang auto|en|id] [--initial-prompt S]
//! [--formats md,txt,json,srt,vtt] [--out-dir D] [--db]`

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_core::paths;
use suramak_core::settings::LangMode;
use suramak_core::store::Store;

use crate::cmd::{parse_formats, resolve_model};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LangArg {
    /// Detect the language automatically
    Auto,
    /// Force English
    En,
    /// Force Indonesian
    Id,
}

#[derive(Args)]
pub struct TranscribeArgs {
    /// Input audio file (wav, mp3, flac, ogg/vorbis, m4a)
    pub input: PathBuf,

    /// Model: manifest id (tiny, base, small, turbo-q5-0, turbo-q8-0, turbo) or a .bin path
    #[arg(long, default_value = "turbo-q5-0")]
    pub model: String,

    /// Source language (forcing `en`/`id` is recommended over auto-detect)
    #[arg(long, value_enum, default_value_t = LangArg::Auto)]
    pub lang: LangArg,

    /// Initial prompt forwarded to whisper (helps brand words and names)
    #[arg(long)]
    pub initial_prompt: Option<String>,

    /// Comma-separated export formats: md,txt,json,srt,vtt
    #[arg(long, default_value = "md")]
    pub formats: String,

    /// Directory for the exported files (created if missing)
    #[arg(long, default_value = ".")]
    pub out_dir: PathBuf,

    /// Also save the transcript to the local library database.
    /// OFF by default for privacy: nothing is persisted unless you opt in.
    #[arg(long)]
    pub db: bool,
}

pub fn run(args: TranscribeArgs) -> Result<()> {
    if !args.input.is_file() {
        bail!("input file not found: {}", args.input.display());
    }
    let model_path = resolve_model(&args.model)?;
    let formats = parse_formats(&args.formats)?;
    let lang = match args.lang {
        LangArg::Auto => LangMode::Auto,
        LangArg::En => LangMode::Fixed("en".to_string()),
        LangArg::Id => LangMode::Fixed("id".to_string()),
    };
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("cannot create output dir {}", args.out_dir.display()))?;

    let worker = Worker::spawn();
    worker.submit(Job {
        id: 1,
        input: args.input.clone(),
        model_path,
        lang,
        initial_prompt: args.initial_prompt,
    })?;

    let mut next_pct = 0u8;
    let transcript = loop {
        match worker.recv() {
            Some(WorkerEvent::Progress { pct, .. }) => {
                if pct >= next_pct {
                    eprint!("\rtranscribing… {pct:3}%");
                    next_pct = pct + 10;
                }
            }
            // Emitted just before Finished; the CLI writes files from the transcript.
            Some(WorkerEvent::Segments { .. }) => {}
            Some(WorkerEvent::Finished { transcript, .. }) => break transcript,
            Some(WorkerEvent::Error { message, .. }) => {
                eprintln!();
                bail!("transcription failed: {message}");
            }
            None => bail!("suramak-worker exited unexpectedly"),
        }
    };
    eprintln!("\rtranscribing… done  ");

    if transcript.segments.is_empty() {
        eprintln!("warning: no speech segments were detected");
    }

    let stem = args
        .input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("transcript")
        .to_string();
    for format in &formats {
        let text = format.render(&transcript)?;
        let path = args.out_dir.join(format!("{stem}.{}", format.extension()));
        std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
        println!(
            "wrote {} ({} segments, {})",
            path.display(),
            transcript.segments.len(),
            format.extension()
        );
    }

    if args.db {
        let store = Store::open(&paths::db_path()?)?;
        store.insert_transcript(&transcript)?;
        println!("saved to library: id {}", transcript.id);
    } else {
        eprintln!("library save off by default (privacy) — pass --db to opt in");
    }
    Ok(())
}
