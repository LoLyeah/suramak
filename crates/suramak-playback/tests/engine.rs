//! Engine-level behaviour without a device: the headless VPS (and CI) has no
//! output device, so the engine must report that as a value — never a panic —
//! and keep answering commands.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use suramak_playback::{Command, Engine};

/// A real, decodable wav (16 kHz mono i16 — the recording format).
fn temp_wav(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("suramak-playback-engine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(format!("{tag}.wav"));
    let samples: Vec<f32> = (0..16_000)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin() * 0.5)
        .collect();
    suramak_core::decode::write_wav_i16(&path, &samples, 16_000, 1).expect("write wav");
    path
}

/// Polls until the engine reports an error or has advanced past 0.
enum Outcome {
    Failed(suramak_playback::EngineError),
    Playing(u32),
}

fn settle(engine: &Engine, window: Duration) -> Outcome {
    let deadline = Instant::now() + window;
    loop {
        let status = engine.poll();
        if let Some(error) = status.error {
            return Outcome::Failed(error);
        }
        if status.position_ms > 0 {
            return Outcome::Playing(status.position_ms);
        }
        assert!(
            Instant::now() < deadline,
            "the engine neither played nor failed within {window:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn an_unreadable_source_fails_cleanly_and_the_engine_keeps_running() {
    let engine = Engine::start(PathBuf::from("/definitely/not/here.wav")).expect("engine thread");
    engine
        .send(Command::Play { from_ms: 0 })
        .expect("the command channel is open");

    let Outcome::Failed(error) = settle(&engine, Duration::from_secs(5)) else {
        panic!("a missing file must fail, not play");
    };
    assert!(
        error.message.contains("could not read the audio"),
        "unexpected message: {}",
        error.message
    );
    assert!(
        !error.retryable,
        "a source error is not retryable — re-import instead"
    );

    // The engine still answers (playback was never opened, so these are no-ops).
    engine.send(Command::Pause).expect("still alive");
    engine.send(Command::Resume).expect("still alive");
    engine
        .send(Command::Seek { ms: 1_000 })
        .expect("still alive");
    engine.send(Command::Stop).expect("still alive");
    assert!(engine.poll().error.is_none(), "the error is reported once");
}

#[test]
fn a_real_file_either_plays_or_reports_a_clean_device_state() {
    let engine = Engine::start(temp_wav("m8-engine")).expect("engine thread");
    engine
        .send(Command::Play { from_ms: 0 })
        .expect("the command channel is open");

    match settle(&engine, Duration::from_secs(5)) {
        // The headless case (this VPS, CI): a clean, retryable device error.
        Outcome::Failed(error) => {
            assert!(
                error.retryable,
                "a device failure must be retryable: {}",
                error.message
            );
            assert!(
                error.message.contains("output device"),
                "unexpected message: {}",
                error.message
            );
        }
        // A machine with an output device: the playhead advanced.
        Outcome::Playing(position_ms) => {
            assert!(position_ms < 60_000);
            engine.send(Command::Stop).expect("stop");
        }
    }
}
