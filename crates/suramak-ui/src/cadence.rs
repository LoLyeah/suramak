//! Repaint cadence policy (M9b) — the owner's heat complaint, made a rule.
//!
//! The M9a build kept the winit loop awake while *any* "busy" flag held and
//! requested a full-window redraw on every wake (`about_to_wait`: 50 ms
//! `WaitUntil` + `request_redraw`), so idle, recording, playback and job states
//! all painted at the present rate — the measured 7,733 frames / 240 s of the
//! critique. This module is the policy the frame loop now follows:
//!
//! | state | cadence | why |
//! |---|---|---|
//! | recording | 100 ms (~10 fps) | the meter/timer only; live transcript lines repaint on arrival |
//! | playing | 33 ms (≤30 fps) | the playhead moves continuously while it plays |
//! | model download | 150 ms | progress is polled; no thread wake-up exists for it |
//! | unsaved edits | at the autosave due time | the debounce tick must run; the save caption paints then |
//! | everything else (idle, job, polish) | event-driven | input, queue arrivals (worker bridge) and animation frames each request their own repaint |
//!
//! Animations need no entry: egui's animation manager calls `request_repaint`
//! while a value is between 0 and 1, and every animation ends (M9a contract).
//! A running transcription job needs no entry either: the worker bridge calls
//! `egui::Context::request_repaint()` for every queued event it delivers.
//!
//! The policy is pure (`&AppState` + now → [`Cadence`]), so the state → interval
//! mapping is unit-tested without a window.

use std::time::{Duration, Instant};

use crate::app::{AppState, RecordState};

/// Recording: the meter/timer cadence (~10 fps).
pub const RECORDING_INTERVAL: Duration = Duration::from_millis(100);
/// Playback: the playhead cadence (≤30 fps).
pub const PLAYBACK_INTERVAL: Duration = Duration::from_millis(33);
/// Model download: progress is polled, so the loop must wake for it.
pub const DOWNLOAD_INTERVAL: Duration = Duration::from_millis(150);
/// Lower bound for any scheduled wake (never a busy loop).
pub const MIN_WAKE: Duration = Duration::from_millis(5);

/// How the frame loop should schedule its next wake-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// No periodic work: wait for events (input, queue arrivals, animations).
    EventDriven,
    /// Wake and repaint at this interval while the state lasts.
    Interval(Duration),
}

impl Cadence {
    /// The interval, when there is one.
    pub fn interval(self) -> Option<Duration> {
        match self {
            Cadence::EventDriven => None,
            Cadence::Interval(interval) => Some(interval),
        }
    }
}

/// The cadence the app state asks for, measured from `now` (the autosave case
/// schedules the *remaining* debounce, not the full window).
///
/// The policy is a refinement of the coarse [`AppState::busy`] predicate: a
/// model download is the one in-flight state `busy` does not name (it owns no
/// worker events — progress is polled), everything else with an interval is a
/// sub-case of it.
pub fn repaint_interval(state: &AppState, now: Instant) -> Cadence {
    if state.model_manager.download.is_running() {
        return Cadence::Interval(DOWNLOAD_INTERVAL);
    }
    if !state.busy() {
        // Nothing in flight: wait for events (input, queue arrivals, animations).
        return Cadence::EventDriven;
    }
    if matches!(state.record, RecordState::Recording(_)) {
        return Cadence::Interval(RECORDING_INTERVAL);
    }
    if state.playback.is_playing() {
        return Cadence::Interval(PLAYBACK_INTERVAL);
    }
    // Unsaved edits: the loop must wake when the autosave debounce is due (the
    // tick that writes them runs in the pump; the save caption repaints then).
    if let Some(editor) = state.editor.as_ref().filter(|editor| editor.dirty) {
        let due = editor
            .last_edit
            .map(|last| last + crate::app::AUTOSAVE_DEBOUNCE)
            .unwrap_or(now);
        let remaining = due.saturating_duration_since(now).max(MIN_WAKE);
        return Cadence::Interval(remaining);
    }
    // A running job repaints on the worker bridge's queue arrivals; a polish run
    // on its own thread's wake-up at completion.
    Cadence::EventDriven
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Job;
    use std::path::PathBuf;
    use std::time::Duration;

    fn idle() -> AppState {
        AppState::new()
    }

    fn with_audio() -> (AppState, PathBuf) {
        let dir = std::env::temp_dir().join(format!("suramak-cadence-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let wav = dir.join("cadence.wav");
        std::fs::write(&wav, b"RIFF....").expect("temp wav");
        let mut state = AppState::new();
        let mut transcript = suramak_core::transcript::Transcript::new(
            "cadence.wav".to_string(),
            suramak_core::transcript::Source::File {
                name: "cadence.wav".to_string(),
            },
            "tiny".to_string(),
            Some("en".to_string()),
            Some(wav.clone()),
            vec![suramak_core::transcript::Segment {
                idx: 0,
                start_ms: 0,
                end_ms: 1_900,
                text: "hello".to_string(),
            }],
        );
        transcript.audio_path = Some(wav.clone());
        state.on_transcript_loaded(transcript);
        (state, wav)
    }

    #[test]
    fn an_idle_app_is_event_driven() {
        let state = idle();
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::EventDriven
        );
    }

    #[test]
    fn recording_meters_at_ten_fps() {
        let mut state = idle();
        state.capture_available = true;
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::Interval(RECORDING_INTERVAL)
        );
    }

    #[test]
    fn playback_runs_at_thirty_fps_and_stops_on_pause() {
        let (mut state, _wav) = with_audio();
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::EventDriven,
            "a parked transport keeps no cadence"
        );
        state.toggle_playback();
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::Interval(PLAYBACK_INTERVAL)
        );
        // Pause ends the periodic cadence (no yanking while paused).
        state.toggle_playback();
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::EventDriven
        );
    }

    #[test]
    fn a_running_job_is_event_driven() {
        let mut state = idle();
        let input = std::env::temp_dir().join("suramak-cadence-job.wav");
        std::fs::write(&input, b"RIFF....").expect("temp wav");
        state.begin_import(input);
        assert!(matches!(state.job, Job::Running(_)));
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::EventDriven,
            "progress arrives as queued worker events (the bridge requests a repaint)"
        );
    }

    #[test]
    fn a_download_wakes_for_its_progress() {
        let mut state = idle();
        assert!(state.model_manager.begin_download("tiny"));
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::Interval(DOWNLOAD_INTERVAL)
        );
        state.on_download_done("tiny", Ok(()));
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::EventDriven
        );
    }

    #[test]
    fn unsaved_edits_wake_at_the_autosave_due_time() {
        let (mut state, _wav) = with_audio();
        let t0 = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "edited".to_string(), t0);
        let interval = repaint_interval(&state, t0).interval().expect("interval");
        assert_eq!(
            interval,
            crate::app::AUTOSAVE_DEBOUNCE,
            "the full debounce remains"
        );
        // Halfway through the debounce the remaining window is scheduled…
        let interval = repaint_interval(&state, t0 + Duration::from_millis(600))
            .interval()
            .expect("interval");
        assert_eq!(interval, Duration::from_millis(400));
        // …and past it the floor keeps the loop from spinning.
        let interval = repaint_interval(&state, t0 + Duration::from_secs(5))
            .interval()
            .expect("interval");
        assert_eq!(interval, MIN_WAKE);
        // Once the save lands, the cadence is gone.
        state.on_edits_saved(t0, Ok(()));
        assert_eq!(repaint_interval(&state, t0), Cadence::EventDriven);
    }

    #[test]
    fn recording_outranks_playback_and_edits() {
        let (mut state, _wav) = with_audio();
        state.capture_available = true;
        // Playback then recording: recording's cadence wins.
        state.toggle_playback();
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        assert_eq!(
            repaint_interval(&state, Instant::now()),
            Cadence::Interval(RECORDING_INTERVAL)
        );
    }
}
