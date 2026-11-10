//! `transcribe <input>... [--model id|path] [--lang auto|en|id] [--engine auto|coreml|metal|cpu]
//! [--initial-prompt S] [--formats md,txt,json,srt,vtt] [--out-dir D] [--db] [--cancel N[,N]]`
//!
//! ## Multi-file batches (M16.1)
//!
//! Every path is enqueued in command-line order on the shared bounded queue
//! ([`suramak_asr::queue`]): **one file transcribes at a time** while the next
//! two may be decoding ahead (the pool is capped at 96 MiB of decoded audio —
//! see the crate docs). Progress and per-file completion lines go to **stderr**
//! so stdout stays parseable; each file prints exactly one terminal line:
//!
//! ```text
//! [1/3] done: a.wav — 42 segments, engine cpu
//! [2/3] failed: b.wav — decode: unsupported codec
//! [3/3] cancelled: c.wav
//! ```
//!
//! `--cancel N` cancels the file at 1-based position `N`:
//!
//! * a waiting file is removed from the queue before the batch starts;
//! * position 1 is the file that runs first, so its cancel is applied right
//!   after it takes the slot: its cooperative token is set and it stops at the
//!   next stage boundary (the worker discards its partial result) — this
//!   requires **at least two paths** to be meaningful, because a single-path
//!   batch has nothing left to continue with;
//! * a failed file never stops the batch; the exit status is non-zero only when
//!   at least one file failed (cancellations are requested outcomes).
//!
//! Output file names are the input stems; two inputs with the same stem are
//! disambiguated with `-2`, `-3`, … in command-line order.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use suramak_asr::engine::resolve_for;
use suramak_asr::queue::driver::{
    FileDecoder, JobOptions, QueueDriver, QueueEvent, ThreadedPrefetch,
};
use suramak_asr::worker::Worker;
use suramak_core::paths;
use suramak_core::settings::{EngineSetting, LangMode, Settings};
use suramak_core::store::Store;
use suramak_core::transcript::Transcript;

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
    /// Input files, transcribed in this order (audio: wav, mp3, flac, ogg/vorbis,
    /// m4a, opus; video: mp4, mov, m4v, mkv, webm — the audio track is used).
    /// One file runs exactly as before; two or more run as a bounded queue.
    #[arg(required = true, num_args = 1..)]
    pub input: Vec<PathBuf>,

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

    /// Enable the experimental fast engine (Parakeet TDT v3 via sherpa-onnx) for
    /// this run — covered languages only; Indonesian always stays on whisper.
    /// Overrides `settings.asr.fast_engine` (which defaults to OFF).
    #[arg(long)]
    pub fast: bool,

    /// Initial prompt forwarded to whisper (helps brand words and names)
    #[arg(long)]
    pub initial_prompt: Option<String>,

    /// Comma-separated export formats: md,txt,json,srt,vtt
    #[arg(long, default_value = "md")]
    pub formats: String,

    /// Directory for the exported files (created if missing)
    #[arg(long, default_value = ".")]
    pub out_dir: PathBuf,

    /// Also save every finished transcript to the local library database.
    /// OFF by default for privacy: nothing is persisted unless you opt in.
    #[arg(long)]
    pub db: bool,

    /// Cancel the file at this 1-based position (repeatable / comma-separated,
    /// e.g. `--cancel 2,4`). A queued file is removed; the running file stops at
    /// its next stage boundary. Meaningful with two or more paths.
    #[arg(long, value_delimiter = ',', num_args = 1..)]
    pub cancel: Vec<usize>,
}

/// Validates `--cancel` positions: 1-based, within the batch, duplicates are
/// harmless (deduped in input order).
fn cancel_positions(positions: &[usize], total: usize) -> Result<Vec<usize>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &position in positions {
        if position == 0 || position > total {
            bail!(
                "--cancel {position} is out of range: the batch has {}",
                if total == 1 {
                    "1 file".to_string()
                } else {
                    format!("{total} files (positions 1..={total})")
                }
            );
        }
        if seen.insert(position) {
            out.push(position);
        }
    }
    Ok(out)
}

/// The export stem for each input, in order. A duplicate stem gets its 1-based
/// position appended, so a batch never overwrites an earlier file's output.
fn output_stems(inputs: &[PathBuf]) -> Vec<String> {
    let stems: Vec<String> = inputs
        .iter()
        .map(|input| {
            input
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("transcript")
                .to_string()
        })
        .collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut duplicates: HashSet<&str> = HashSet::new();
    for stem in &stems {
        if !seen.insert(stem.as_str()) {
            duplicates.insert(stem.as_str());
        }
    }
    stems
        .iter()
        .enumerate()
        .map(|(index, stem)| {
            if duplicates.contains(stem.as_str()) {
                format!("{stem}-{}", index + 1)
            } else {
                stem.clone()
            }
        })
        .collect()
}

/// The display name of one input (the same label the queue rows use).
fn file_label(input: &Path) -> String {
    input
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.display().to_string())
}

/// One finished file's export/save step.
fn finish_file(
    transcript: &Transcript,
    words: &[suramak_core::transcript::WordTiming],
    stem: &str,
    formats: &[crate::cmd::ExportFormat],
    out_dir: &Path,
    db: bool,
) -> Result<()> {
    for format in formats {
        let text = format.render(transcript)?;
        let path = out_dir.join(format!("{stem}.{}", format.extension()));
        std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
        println!(
            "wrote {} ({} segments, {})",
            path.display(),
            transcript.segments.len(),
            format.extension()
        );
    }
    if db {
        let store = Store::open(&paths::db_path()?)?;
        // M15.6: the word timing this run produced lands with the transcript.
        store.insert_transcript_with_words(transcript, words)?;
        println!("saved to library: id {}", transcript.id);
    }
    Ok(())
}

pub fn run(args: TranscribeArgs) -> Result<()> {
    // Validate the whole batch up front: a typo in path 3 must not start a
    // three-file run whose first two files already wrote output.
    if args.input.is_empty() {
        bail!("no input files");
    }
    for input in &args.input {
        if !input.is_file() {
            bail!("input file not found: {}", input.display());
        }
    }
    let total = args.input.len();
    let cancel = cancel_positions(&args.cancel, total)?;
    if !cancel.is_empty() && total == 1 {
        eprintln!(
            "note: --cancel with a single input has nothing to continue with; the file is still cancelled"
        );
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
    // M10A: `--fast` forces the fast engine on for this run; otherwise the
    // settings toggle (default OFF) decides.
    let fast_engine = args.fast || settings.asr.fast_engine;
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("cannot create output dir {}", args.out_dir.display()))?;

    // Resolve the engine up front so a downgraded request is reported before
    // decoding (the worker re-resolves from the same pure function and reports
    // what actually ran).
    let language = match &lang {
        LangMode::Auto => None,
        LangMode::Fixed(code) => Some(code.as_str()),
    };
    let plan = resolve_for(&model_path, engine, fast_engine, language);
    if let Some(warning) = &plan.warning {
        eprintln!("warning: {warning}");
    }
    eprintln!(
        "engine: requested {} → planned {}",
        engine.as_str(),
        plan.engine.as_str()
    );
    if total > 1 {
        eprintln!(
            "queue: {total} files, one at a time (up to 2 decoding ahead, capped at {} MiB decoded)",
            suramak_asr::MAX_PREFETCH_BYTES / (1024 * 1024)
        );
    }

    let options = JobOptions {
        model_path,
        lang,
        initial_prompt: args.initial_prompt.clone(),
        engine,
        fast_engine,
    };
    let mut driver = QueueDriver::new(
        Worker::spawn(),
        ThreadedPrefetch::new(Box::new(FileDecoder)),
        options,
    );
    driver.enqueue_all(args.input.clone());

    // Waiting positions are removed before the batch starts (deterministic:
    // they can never reach the engine). Position 1 is the file that runs first,
    // so its cancel is applied right after it starts — that exercises the
    // running-job seam (the cooperative token), not just queue removal.
    for position in cancel.iter().filter(|position| **position != 1) {
        driver.cancel_seq(*position);
    }
    driver.run_once();
    if cancel.contains(&1) {
        driver.cancel_seq(1);
    }

    let stems = output_stems(&args.input);
    let mut done = 0usize;
    let mut failed = 0usize;
    let mut cancelled = 0usize;

    // Drain after every pump, **including the last one**: `is_idle` can become
    // true in the same `run_once` that produced the batch's terminal events,
    // and a `while !is_idle` head check would then exit without ever draining
    // them (the batch reported 0 outcomes; M13g2's full run caught it).
    loop {
        driver.run_once();
        for event in driver.drain_events() {
            match event {
                QueueEvent::Started { seq, input, .. } => {
                    eprint!(
                        "\r[{seq}/{total}] {}: transcribing… {:>3}%   ",
                        file_label(&input),
                        0
                    );
                }
                QueueEvent::Progress { seq, pct, .. } => {
                    eprint!("\r[{seq}/{total}] transcribing… {pct:>3}%   ");
                }
                QueueEvent::Stage { .. } => {}
                QueueEvent::Finished {
                    seq,
                    input,
                    transcript,
                    words,
                    engine,
                    ..
                } => {
                    eprintln!(
                        "\r[{seq}/{total}] done: {} — {} segments, engine {}{}",
                        file_label(&input),
                        transcript.segments.len(),
                        engine.as_str(),
                        " ".repeat(20)
                    );
                    let stem = &stems[seq - 1];
                    if let Err(err) =
                        finish_file(&transcript, &words, stem, &formats, &args.out_dir, args.db)
                    {
                        // An export failure does not invalidate the transcript
                        // or stop the batch; it is reported per file.
                        eprintln!("\r[{seq}/{total}] failed to write output: {err}");
                        failed += 1;
                        continue;
                    }
                    done += 1;
                }
                QueueEvent::Failed {
                    seq, input, reason, ..
                } => {
                    eprintln!(
                        "\r[{seq}/{total}] failed: {} — {reason}{}",
                        file_label(&input),
                        " ".repeat(20)
                    );
                    failed += 1;
                }
                QueueEvent::Cancelled { seq, input, .. } => {
                    eprintln!(
                        "\r[{seq}/{total}] cancelled: {}{}",
                        file_label(&input),
                        " ".repeat(20)
                    );
                    cancelled += 1;
                }
            }
        }
        if driver.is_idle() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // Every position must have produced exactly one terminal line (a defensive
    // check: a queue that lost an item would otherwise end quietly short).
    debug_assert_eq!(
        done + failed + cancelled,
        total,
        "every enqueued file must report one terminal outcome"
    );
    eprintln!("queue: {done} done, {failed} failed, {cancelled} cancelled of {total}");
    if !args.db && done > 0 {
        eprintln!("library save off by default (privacy) — pass --db to opt in");
    }
    if failed > 0 {
        bail!(
            "{failed} of {total} {} failed",
            if total == 1 { "file" } else { "files" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_positions_are_one_based_bounded_and_deduped() {
        assert_eq!(cancel_positions(&[], 3).unwrap(), Vec::<usize>::new());
        assert_eq!(cancel_positions(&[1, 3], 3).unwrap(), vec![1, 3]);
        assert_eq!(cancel_positions(&[2, 2, 1], 3).unwrap(), vec![2, 1]);
        assert!(cancel_positions(&[0], 2).is_err());
        assert!(cancel_positions(&[3], 2).is_err());
        // The message names the batch size.
        let err = cancel_positions(&[4], 3).unwrap_err().to_string();
        assert!(err.contains("positions 1..=3"), "unexpected: {err}");
    }

    #[test]
    fn duplicate_stems_get_distinct_output_names() {
        let inputs = vec![
            PathBuf::from("/a/clip.wav"),
            PathBuf::from("/b/clip.mp3"),
            PathBuf::from("/c/other.m4a"),
        ];
        assert_eq!(
            output_stems(&inputs),
            vec![
                "clip-1".to_string(),
                "clip-2".to_string(),
                "other".to_string()
            ]
        );
    }

    fn junk(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-cli-queue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        // A real file that is not decodable audio.
        std::fs::write(&path, b"not audio at all").unwrap();
        path
    }

    fn fake_model() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-cli-queue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ggml-fake.bin");
        std::fs::write(&path, b"model").unwrap();
        path
    }

    /// Two undecodable inputs: the batch reaches both, each fails by name, and
    /// the run reports a count instead of stopping at the first failure.
    #[test]
    fn a_multi_file_batch_continues_after_a_failure() {
        let out = std::env::temp_dir().join(format!("suramak-cli-out-{}", std::process::id()));
        let args = TranscribeArgs {
            input: vec![junk("first.wav"), junk("second.wav")],
            model: fake_model().display().to_string(),
            lang: LangArg::En,
            engine: Some(EngineArg::Cpu),
            fast: false,
            initial_prompt: None,
            formats: "txt".to_string(),
            out_dir: out,
            db: false,
            cancel: Vec::new(),
        };
        let err = run(args).expect_err("both inputs are undecodable");
        let message = err.to_string();
        assert!(
            message.contains("2 of 2 files failed"),
            "unexpected: {message}"
        );
    }

    /// `--cancel 2` removes the queued second file: the first still runs (and
    /// fails here), the second is reported cancelled, and the exit status only
    /// counts the failure.
    #[test]
    fn cancel_by_position_reports_the_cancelled_file_and_keeps_going() {
        let out = std::env::temp_dir().join(format!("suramak-cli-out-{}", std::process::id()));
        let args = TranscribeArgs {
            input: vec![junk("keep.wav"), junk("drop.wav")],
            model: fake_model().display().to_string(),
            lang: LangArg::En,
            engine: Some(EngineArg::Cpu),
            fast: false,
            initial_prompt: None,
            formats: "txt".to_string(),
            out_dir: out,
            db: false,
            cancel: vec![2],
        };
        let err = run(args).expect_err("the kept input is undecodable");
        let message = err.to_string();
        assert!(
            message.contains("1 of 2 files failed"),
            "unexpected: {message}"
        );
    }
}
