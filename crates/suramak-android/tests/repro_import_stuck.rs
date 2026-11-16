//! Repro for the owner's report: an import that says "Transcribing 1 file with
//! small" and a notification pinned at 0%, forever, while the Library stays
//! empty.
//!
//! This drives the SAME host the Android service drives — `JobHost::production`,
//! real whisper worker, real file decoder — with the real `small` model and the
//! real 2-minute WAV, on Linux. If it hangs here, the bug is in the Rust host
//! and the emulator is not needed to see it.
//!
//! ```text
//! SURAMAK_REQUIRE_MODEL=1 GGML_SMALL=/path/ggml-small.bin \
//!   cargo test -p suramak-android --test repro_import_stuck -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use suramak_android::jobs::{HostDirs, JobHost, Phase};

fn scratch(name: &str) -> PathBuf {
    // A directory this test owns: never the system temp root, never a shared
    // target dir that another run may have removed (the first version used
    // CARGO_TARGET_TMPDIR, which does not exist for an integration test that
    // never ran before, and the setup panicked on line 46).
    let dir = std::env::var_os("SURAMAK_REPRO_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("suramak-{name}-{}", std::process::id()))
        });
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn model_or_skip() -> Option<PathBuf> {
    let path = std::env::var_os("GGML_SMALL").map(PathBuf::from)?;
    Some(path).filter(|path| path.is_file())
}

#[test]
#[ignore = "real model + real decode; run by hand with GGML_SMALL set"]
fn an_import_of_the_two_minute_wav_reaches_done() {
    let Some(model) = model_or_skip() else {
        eprintln!("GGML_SMALL is not set: skipping");
        return;
    };
    let root = scratch("repro-import-stuck");
    let dirs = HostDirs::under(&root);
    std::fs::create_dir_all(&dirs.models_dir).unwrap();
    println!("model  = {}", model.display());
    println!("models = {}", dirs.models_dir.display());
    std::fs::copy(&model, dirs.models_dir.join("ggml-small.bin")).unwrap_or_else(|err| {
        panic!(
            "copy {} -> {}: {err}",
            model.display(),
            dirs.models_dir.display()
        )
    });

    let host = JobHost::production(dirs.clone());
    // Default to the committed fixture; WAV_SOURCE points at a longer file.
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/en.wav");
    let source = std::env::var_os("WAV_SOURCE")
        .map(PathBuf::from)
        .unwrap_or(default);
    let size = std::fs::metadata(&source)
        .unwrap_or_else(|err| panic!("the audio to import ({}): {err}", source.display()))
        .len();
    let (key, dest) = host
        .begin_import(
            "suramak-test-en-2min.wav",
            "audio/wav",
            size,
            "small",
            "auto",
        )
        .expect("accepted");

    // The service's copy loop, byte for byte.
    let bytes = std::fs::read(source).unwrap();
    let mut written = 0usize;
    for chunk in bytes.chunks(64 * 1024) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&dest)
            .unwrap();
        file.write_all(chunk).unwrap();
        written += chunk.len();
        host.import_progress(&key, written as u64);
    }
    host.import_done(&key).expect("the copy verified");

    let deadline = Instant::now() + Duration::from_secs(240);
    let mut last = String::new();
    while Instant::now() < deadline {
        let status = host.status();
        let line = format!(
            "running={} phase={} percent={} waiting={} done={} failed={} msg={}",
            status.running,
            status.current_phase,
            status.percent,
            status.waiting,
            status.done,
            status.failed,
            status.message
        );
        if line != last {
            println!("{line}");
            last = line;
        }
        if status.done + status.failed > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let status = host.status();
    let journal = std::fs::read_to_string(&dirs.journal_path).unwrap_or_default();
    println!("--- journal ---\n{journal}");
    println!("--- recovery view ---");
    for entry in host.recover() {
        println!("{entry:?}");
    }
    assert_eq!(status.failed, 0, "the job failed: {}", status.message);
    assert_eq!(
        status.done, 1,
        "the job never finished (see the journal above)"
    );
}

#[test]
fn the_dirs_shape_matches_the_service() {
    // Guards the repro itself: the host must be looking at a real tree.
    let dirs = HostDirs::under(&scratch("repro-dirs"));
    assert!(dirs.media_dir.ends_with("media"));
    assert_eq!(Phase::Queued.as_str(), "queued");
}

/// The owner's second half: kill mid-transcription, relaunch, tap Resume.
///
/// A relaunch loads the journal (every live phase becomes `Paused`), offers
/// `recover()`, and `resume()` puts the row back in the queue. This asserts the
/// resumed job reaches `done` — the exact path that sits at 0 % on the phone.
#[test]
#[ignore = "real model + real decode; run by hand with GGML_SMALL set"]
fn a_job_interrupted_mid_transcription_resumes_to_done() {
    let Some(model) = model_or_skip() else {
        eprintln!("GGML_SMALL is not set: skipping");
        return;
    };
    let root = scratch("repro-resume-stuck");
    let dirs = HostDirs::under(&root);
    std::fs::create_dir_all(&dirs.models_dir).unwrap();
    std::fs::copy(&model, dirs.models_dir.join("ggml-small.bin")).unwrap();
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/en.wav");
    let source = std::env::var_os("WAV_SOURCE")
        .map(PathBuf::from)
        .unwrap_or(default);
    let size = std::fs::metadata(&source).unwrap().len();

    // First process: import, start, then "die" mid-transcription.
    let key;
    {
        let host = JobHost::production(dirs.clone());
        let (k, dest) = host
            .begin_import("interrupted.wav", "audio/wav", size, "small", "auto")
            .expect("accepted");
        std::fs::copy(&source, &dest).unwrap();
        host.import_progress(&k, size);
        host.import_done(&k).expect("the copy verified");
        // Wait until the driver owns it, then pause the way a kill would look.
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline && host.status().current_phase != "transcribing" {
            std::thread::sleep(Duration::from_millis(50));
        }
        println!("first process: phase={}", host.status().current_phase);
        host.pause_all();
        key = k;
        std::thread::sleep(Duration::from_millis(500));
    }

    // Second process: the journal is what survives.
    let host = JobHost::production(dirs.clone());
    let recovery = host.recover();
    println!("recover() after relaunch: {recovery:?}");
    let entry = recovery.iter().find(|entry| entry.key == key);
    let entry = entry.unwrap_or_else(|| panic!("the interrupted job is not offered: {recovery:?}"));
    assert!(entry.resumable, "offered but not resumable: {entry:?}");

    host.resume(&key).expect("resume accepted");
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut last = String::new();
    while Instant::now() < deadline {
        let status = host.status();
        let line = format!(
            "running={} phase={} percent={} waiting={} done={} failed={} msg={}",
            status.running,
            status.current_phase,
            status.percent,
            status.waiting,
            status.done,
            status.failed,
            status.message
        );
        if line != last {
            println!("{line}");
            last = line;
        }
        if status.done + status.failed > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let status = host.status();
    println!(
        "--- journal after resume ---\n{}",
        std::fs::read_to_string(&dirs.journal_path).unwrap_or_default()
    );
    assert_eq!(
        status.failed, 0,
        "the resumed job failed: {}",
        status.message
    );
    assert_eq!(status.done, 1, "the resumed job never finished: {last}");
}
