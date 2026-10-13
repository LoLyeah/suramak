//! `transcribe <input> [--model id|path] [--lang auto|en|id] [--engine auto|coreml|metal|cpu]
//! [--initial-prompt S] [--formats md,txt,json,srt,vtt] [--out-dir D] [--db]`

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use suramak_asr::engine::resolve_for;
use suramak_asr::worker::{Job, Worker, WorkerEvent};
use suramak_core::paths;
use suramak_core::settings::{EngineSetting, LangMode, Settings};
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

/// `--engine` values (identical to `EngineSetting`, spelled for the CLI).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum EngineArg {
    /// Pick the best available engine (macOS: coreml → metal → cpu; elsewhere: cpu)
    Auto,
    /// Core ML encoder package on the ANE (macOS + `models download <id> --encoder`)
    Coreml,
    /// Metal GPU backend (macOS)
    Metal,
    /// CPU only
    Cpu,
}

impl From<EngineArg> for EngineSetting {
    fn from(arg: EngineArg) -> Self {
        match arg {
            EngineArg::Auto => EngineSetting::Auto,
            EngineArg::Coreml => EngineSetting::CoreML,
            EngineArg::Metal => EngineSetting::Metal,
            EngineArg::Cpu => EngineSetting::Cpu,
        }
    }
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

    /// Engine: auto (settings.toml default), coreml (ANE), metal, cpu.
    /// Defaults to `settings.asr.engine` (auto when unset).
    #[arg(long, value_enum)]
    pub engine: Option<EngineArg>,

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
    // Flags beat settings.toml, which beats the built-in default (`auto`).
    let settings = Settings::load(&paths::settings_path()?)?;
    let engine = args
        .engine
        .map(EngineSetting::from)
        .unwrap_or(settings.asr.engine);
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("cannot create output dir {}", args.out_dir.display()))?;

    // Resolve the engine up front so a downgraded request is reported before decoding
    // (the worker re-resolves from the same pure function and reports what actually ran).
    let language = match &lang {
        LangMode::Auto => None,
        LangMode::Fixed(code) => Some(code.as_str()),
    };
    let plan = resolve_for(&model_path, engine, language);
    if let Some(warning) = &plan.warning {
        eprintln!("warning: {warning}");
    }
    eprintln!(
        "engine: requested {} → planned {}",
        engine.as_str(),
        plan.engine.as_str()
    );

    let worker = Worker::spawn();
    worker.submit(Job {
        id: 1,
        input: args.input.clone(),
        model_path,
        lang,
        initial_prompt: args.initial_prompt,
        engine,
    })?;

    let mut next_pct = 0u8;
    let (transcript, engine_used, warnings) = loop {
        match worker.recv() {
            Some(WorkerEvent::Progress { pct, .. }) => {
                if pct >= next_pct {
                    eprint!("\rtranscribing… {pct:3}%");
                    next_pct = pct + 10;
                }
            }
            // Emitted just before Finished; the CLI writes files from the transcript.
            Some(WorkerEvent::Segments { .. }) => {}
            Some(WorkerEvent::Finished {
                transcript,
                engine_used,
                warnings,
                ..
            }) => break (transcript, engine_used, warnings),
            Some(WorkerEvent::Error { message, .. }) => {
                eprintln!();
                bail!("transcription failed: {message}");
            }
            None => bail!("suramak-worker exited unexpectedly"),
        }
    };
    eprintln!("\rtranscribing… done  ");

    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    // The engaged engine, always on stderr (stdout stays parseable for scripts).
    eprintln!("engine: {}", engine_used.as_str());

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
