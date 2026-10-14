//! Frame accounting (M9b): the before/after evidence for the repaint budget.
//!
//! Every **presented** frame is attributed to the visible state it was drawn in
//! ([`classify`]) and counted; the per-state span is accumulated between frames
//! so the summary can report an fps per state. The summary prints at exit when
//! `SURAMAK_FRAME_STATS=1` (or `=anything-but-0`) — see `App::quit`:
//!
//! ```text
//! suramak frame stats
//! elapsed=30.0s frames=41 fps=1.4
//! idle       frames=35  seconds=29.8  fps=1.2
//! recording  frames=6   seconds=0.2   fps=30.0
//! ```
//!
//! This is the seam the workorder's before/after table is reproduced through:
//! run any smoke scenario with `SURAMAK_FRAME_STATS=1` and read the block off
//! the log (the plain smoke's `build/smoke_ok.txt` and the job smoke's
//! `build/smoke_job_ok.txt` also carry the raw frame count).

use std::time::{Duration, Instant};

use crate::app::{AppState, Job, RecordState};

/// The visible state a frame is attributed to (priority order: recording wins
/// over playback, which wins over a job — the same order as the cadence policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameState {
    Idle,
    Recording,
    Playing,
    Job,
    Polish,
    Download,
}

impl FrameState {
    /// Every state, in the order the summary prints them.
    pub const ALL: [FrameState; 6] = [
        FrameState::Idle,
        FrameState::Recording,
        FrameState::Playing,
        FrameState::Job,
        FrameState::Polish,
        FrameState::Download,
    ];

    /// The summary label.
    pub fn label(self) -> &'static str {
        match self {
            FrameState::Idle => "idle",
            FrameState::Recording => "recording",
            FrameState::Playing => "playing",
            FrameState::Job => "job",
            FrameState::Polish => "polish",
            FrameState::Download => "download",
        }
    }

    fn index(self) -> usize {
        match self {
            FrameState::Idle => 0,
            FrameState::Recording => 1,
            FrameState::Playing => 2,
            FrameState::Job => 3,
            FrameState::Polish => 4,
            FrameState::Download => 5,
        }
    }
}

/// The state the frame being drawn belongs to.
pub fn classify(state: &AppState) -> FrameState {
    if matches!(state.record, RecordState::Recording(_)) {
        FrameState::Recording
    } else if state.playback.is_playing() {
        FrameState::Playing
    } else if matches!(state.job, Job::Running(_)) {
        FrameState::Job
    } else if state.polish.is_running() {
        FrameState::Polish
    } else if state.model_manager.download.is_running() {
        FrameState::Download
    } else {
        FrameState::Idle
    }
}

/// Whether `SURAMAK_FRAME_STATS` asks for the exit summary (`1` — or anything
/// except empty/`0`).
pub fn enabled() -> bool {
    std::env::var("SURAMAK_FRAME_STATS")
        .map(|value| !value.is_empty() && value != "0")
        .unwrap_or(false)
}

/// The running frame tally: total frames plus a per-state frame count and span.
#[derive(Debug, Clone)]
pub struct FrameStats {
    started: Instant,
    frames: u64,
    per_state: [(u64, Duration); 6],
    previous: Option<(FrameState, Instant)>,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameStats {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            frames: 0,
            per_state: [(0, Duration::ZERO); 6],
            previous: None,
        }
    }

    /// One presented frame: the time since the previous frame is attributed to
    /// the state that was on screen during it, then the frame is counted.
    pub fn on_frame(&mut self, state: &AppState, now: Instant) {
        if let Some((previous_state, at)) = self.previous {
            self.per_state[previous_state.index()].1 += now.saturating_duration_since(at);
        }
        let current = classify(state);
        self.per_state[current.index()].0 += 1;
        self.frames += 1;
        self.previous = Some((current, now));
    }

    /// The block printed at exit under `SURAMAK_FRAME_STATS`.
    pub fn summary(&self) -> String {
        let elapsed = self.started.elapsed().as_secs_f64();
        let fps = |frames: u64, seconds: f64| {
            if seconds > 0.0 {
                frames as f64 / seconds
            } else {
                0.0
            }
        };
        let mut text = format!(
            "suramak frame stats\nelapsed={elapsed:.1}s frames={} fps={:.1}\n",
            self.frames,
            fps(self.frames, elapsed)
        );
        for state in FrameState::ALL {
            let (frames, span) = self.per_state[state.index()];
            let seconds = span.as_secs_f64();
            text.push_str(&format!(
                "{:<10} frames={frames:<6} seconds={seconds:.1}  fps={:.1}\n",
                state.label(),
                fps(frames, seconds)
            ));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_attributed_to_the_state_that_was_on_screen() {
        let mut stats = FrameStats::new();
        let idle = AppState::new();
        let t0 = Instant::now();
        // Ten idle frames at 30 fps…
        for step in 0..10 {
            stats.on_frame(&idle, t0 + Duration::from_millis(step * 33));
        }
        assert_eq!(stats.per_state[FrameState::Idle.index()].0, 10);
        assert!(
            stats.summary().contains("frames=10"),
            "the total is the frame count"
        );
        // …then one recording frame: the span before it counts as idle.
        let mut recording = AppState::new();
        recording.capture_available = true;
        recording.start_recording(t0, "2026-10-14 09:00:00");
        stats.on_frame(&recording, t0 + Duration::from_millis(330));
        assert_eq!(stats.per_state[FrameState::Idle.index()].0, 10);
        assert_eq!(stats.per_state[FrameState::Recording.index()].0, 1);
        assert!(
            stats.per_state[FrameState::Idle.index()].1 >= Duration::from_millis(297),
            "the span up to the state change is idle's"
        );
    }

    #[test]
    fn classification_follows_the_cadence_priority() {
        let mut state = AppState::new();
        assert_eq!(classify(&state), FrameState::Idle);

        state.capture_available = true;
        state.start_recording(Instant::now(), "2026-10-14 09:00:00");
        assert_eq!(classify(&state), FrameState::Recording);

        let mut state = AppState::new();
        state.job = Job::Running(crate::app::RunningJob {
            input: std::path::PathBuf::from("/tmp/x.wav"),
            pct: 0,
            model: None,
            requested_engine: None,
            planned_engine: None,
            warnings: Vec::new(),
        });
        assert_eq!(classify(&state), FrameState::Job);

        assert!(state.model_manager.begin_download("tiny"));
        // A running job still outranks the download.
        assert_eq!(classify(&state), FrameState::Job);
        state.job = Job::Idle;
        assert_eq!(classify(&state), FrameState::Download);
    }

    #[test]
    fn the_summary_lists_every_state_and_the_total() {
        let mut stats = FrameStats::new();
        let idle = AppState::new();
        stats.on_frame(&idle, Instant::now());
        let text = stats.summary();
        assert!(text.starts_with("suramak frame stats\n"));
        assert!(text.contains("frames=1 fps="));
        for state in FrameState::ALL {
            assert!(
                text.contains(&format!("{:<10} frames=", state.label())),
                "the summary lists {state:?}"
            );
        }
    }
}
