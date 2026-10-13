//! The playback transport (M8): the state machine the editor draws from and the
//! command protocol the engine thread understands. **Egui-free and device-free**
//! — every rule below is unit-tested without touching cpal.
//!
//! States: `Idle → Playing → Paused → Idle` (stop-on-leave) plus the
//! `Unavailable` path (missing audio / no output device). Positions are
//! milliseconds; the engine's polled position is authoritative while a session
//! is live, the transport's own position (click-to-seek) while it is not.

use std::path::Path;

use crate::source::PlaybackSource;

/// A command for the engine thread (the workorder's protocol).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start (or restart) playback at `from_ms`.
    Play { from_ms: u32 },
    /// Stop advancing the playhead (the engine keeps its position).
    Pause,
    /// Continue from the engine's position.
    Resume,
    /// Jump to `ms` (while playing, playback continues from there).
    Seek { ms: u32 },
    /// Pause and reset to the start (leaving the editor / switching / recording).
    Stop,
}

/// Where the transport is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackState {
    /// Nothing playing (never started, stopped or finished).
    Idle,
    /// Playing; the engine advances the position.
    Playing,
    /// Paused at `position_ms`.
    Paused,
    /// Playback is impossible for now. `retry` is set when the cause may go
    /// away (no output device plugged in yet) — play then retries instead of
    /// being dead. A missing source is not retryable.
    Unavailable { hint: String, retry: bool },
}

/// A copy of the state's shape, so the rules can assign `self.state` while they
/// read it (the real enum owns a `String`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Idle,
    Playing,
    Paused,
    Unavailable { retry: bool },
}

impl PlaybackState {
    fn shape(&self) -> Shape {
        match self {
            PlaybackState::Idle => Shape::Idle,
            PlaybackState::Playing => Shape::Playing,
            PlaybackState::Paused => Shape::Paused,
            PlaybackState::Unavailable { retry, .. } => Shape::Unavailable { retry: *retry },
        }
    }

    /// The state a source starts in (`Ready` → `Idle`, gone → `Unavailable`).
    fn initial(source: &PlaybackSource) -> Self {
        match source {
            PlaybackSource::Ready { .. } => PlaybackState::Idle,
            PlaybackSource::Unavailable { hint } => PlaybackState::Unavailable {
                hint: hint.clone(),
                retry: false,
            },
        }
    }
}

/// The open transcript's playback transport: resolved source, state, position
/// and duration. Owned by the UI state machine (`AppState::playback`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playback {
    source: PlaybackSource,
    state: PlaybackState,
    position_ms: u32,
    duration_ms: u32,
}

impl Playback {
    /// A transport for a resolved source and the transcript's duration estimate.
    pub fn new(source: PlaybackSource, duration_ms: u32) -> Self {
        Self {
            state: PlaybackState::initial(&source),
            source,
            position_ms: 0,
            duration_ms,
        }
    }

    /// The transport with no transcript open (the app's initial state).
    pub fn none() -> Self {
        Self::new(
            PlaybackSource::Unavailable {
                hint: "no transcript open".to_string(),
            },
            0,
        )
    }

    pub fn source(&self) -> &PlaybackSource {
        &self.source
    }

    /// The file to play, when the source is ready.
    pub fn ready_path(&self) -> Option<&Path> {
        self.source.ready_path()
    }

    pub fn state(&self) -> &PlaybackState {
        &self.state
    }

    /// The user-facing reason playback is impossible, when it is.
    pub fn hint(&self) -> Option<&str> {
        match &self.state {
            PlaybackState::Unavailable { hint, .. } => Some(hint),
            _ => None,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.state, PlaybackState::Idle)
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.state, PlaybackState::Playing)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self.state, PlaybackState::Paused)
    }

    /// Whether the transport control can start/resume playback right now (the
    /// strip dims the glyph otherwise).
    pub fn can_play(&self) -> bool {
        match self.state.shape() {
            Shape::Unavailable { retry } => retry && self.source.is_ready(),
            _ => true,
        }
    }

    pub fn position_ms(&self) -> u32 {
        self.position_ms
    }

    pub fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    /// The strip's play/pause control.
    ///
    /// `Idle` starts a session (`Play`), `Playing` pauses, `Paused` resumes,
    /// and a *retryable* unavailable state (a device that went away) tries
    /// again. A missing source never emits a command.
    pub fn toggle(&mut self) -> Option<Command> {
        match self.state.shape() {
            Shape::Playing => {
                self.state = PlaybackState::Paused;
                Some(Command::Pause)
            }
            Shape::Paused => {
                self.state = PlaybackState::Playing;
                Some(Command::Resume)
            }
            Shape::Idle => {
                // A playhead parked at the end starts over (resuming there would
                // play nothing).
                let from_ms = self.restart_position();
                self.position_ms = from_ms;
                self.state = PlaybackState::Playing;
                Some(Command::Play { from_ms })
            }
            Shape::Unavailable { retry } => {
                if retry && self.source.is_ready() {
                    self.state = PlaybackState::Playing;
                    Some(Command::Play {
                        from_ms: self.position_ms,
                    })
                } else {
                    None
                }
            }
        }
    }

    /// Click-to-seek (the strip, a segment's timestamp). Clamped to the
    /// duration. While a session is live the engine is told; while idle the
    /// position is remembered for the next `Play`; an unavailable source is a
    /// no-op.
    pub fn seek(&mut self, ms: u32) -> Option<Command> {
        if let Shape::Unavailable { .. } = self.state.shape() {
            return None;
        }
        let ms = if self.duration_ms > 0 {
            ms.min(self.duration_ms)
        } else {
            ms
        };
        self.position_ms = ms;
        match self.state.shape() {
            Shape::Playing | Shape::Paused => Some(Command::Seek { ms }),
            _ => None,
        }
    }

    /// Stop and reset (leaving the editor, deleting the transcript, switching
    /// transcripts, starting a recording). Emits `Stop` only when a session was
    /// live; the position always resets.
    pub fn stop(&mut self) -> Option<Command> {
        let had_session = matches!(self.state.shape(), Shape::Playing | Shape::Paused);
        self.position_ms = 0;
        self.state = PlaybackState::initial(&self.source);
        had_session.then_some(Command::Stop)
    }

    /// Applies a polled engine status.
    ///
    /// The duration follows the engine (the real audio length replaces the
    /// transcript estimate). The position is only taken while a session is live
    /// — a stale poll after `stop` must not resurrect the playhead. `finished`
    /// (the engine reached the end) parks the playhead at the duration and goes
    /// back to `Idle`.
    pub fn on_status(&mut self, position_ms: u32, duration_ms: u32, finished: bool) {
        if duration_ms > 0 {
            self.duration_ms = duration_ms;
        }
        if finished {
            if matches!(self.state.shape(), Shape::Playing | Shape::Paused) {
                self.state = PlaybackState::Idle;
                self.position_ms = self.duration_ms;
            }
            return;
        }
        if matches!(self.state.shape(), Shape::Playing | Shape::Paused) {
            self.position_ms = if self.duration_ms > 0 {
                position_ms.min(self.duration_ms)
            } else {
                position_ms
            };
        }
    }

    /// The engine reported a failure (no output device, unreadable file, device
    /// lost mid-play). `retry` keeps the control armed for another attempt.
    pub fn on_failure(&mut self, message: String, retry: bool) {
        self.state = PlaybackState::Unavailable {
            hint: message,
            retry,
        };
    }

    /// Where a fresh `Play` starts: at the playhead, or from the top when the
    /// playhead is parked at the end.
    fn restart_position(&self) -> u32 {
        if self.duration_ms > 0 && self.position_ms >= self.duration_ms {
            0
        } else {
            self.position_ms
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{PlaybackSource, KEEP_AUDIO_HINT};
    use std::path::PathBuf;

    fn ready() -> PlaybackSource {
        PlaybackSource::Ready {
            path: PathBuf::from("/tmp/audio.wav"),
        }
    }

    fn playing(duration_ms: u32) -> Playback {
        let mut playback = Playback::new(ready(), duration_ms);
        assert_eq!(playback.toggle(), Some(Command::Play { from_ms: 0 }));
        playback
    }

    #[test]
    fn idle_play_pauses_and_resumes() {
        let mut playback = Playback::new(ready(), 10_000);
        assert!(playback.is_idle());
        assert!(!playback.is_playing());
        assert!(playback.can_play());
        assert_eq!(playback.hint(), None);

        assert_eq!(playback.toggle(), Some(Command::Play { from_ms: 0 }));
        assert!(playback.is_playing(), "idle → playing");
        assert_eq!(playback.hint(), None);

        assert_eq!(playback.toggle(), Some(Command::Pause));
        assert!(playback.is_paused(), "playing → paused");
        assert_eq!(playback.position_ms(), 0);

        assert_eq!(playback.toggle(), Some(Command::Resume));
        assert!(playback.is_playing(), "paused → playing");
    }

    #[test]
    fn stop_on_leave_resets_the_transport_and_tells_the_engine() {
        let mut playback = playing(10_000);
        playback.on_status(4_000, 10_000, false);
        assert_eq!(playback.position_ms(), 4_000);

        assert_eq!(playback.stop(), Some(Command::Stop), "a live session stops");
        assert!(playback.is_idle());
        assert_eq!(playback.position_ms(), 0, "the position resets");
        assert_eq!(playback.stop(), None, "stop is idempotent");
        assert!(playback.can_play(), "the source is still ready");
    }

    #[test]
    fn an_unavailable_source_never_plays() {
        let mut playback = Playback::new(
            PlaybackSource::Unavailable {
                hint: KEEP_AUDIO_HINT.to_string(),
            },
            10_000,
        );
        assert_eq!(
            playback.state(),
            &PlaybackState::Unavailable {
                hint: KEEP_AUDIO_HINT.to_string(),
                retry: false,
            }
        );
        assert!(!playback.can_play(), "the control is dead");
        assert_eq!(playback.hint(), Some(KEEP_AUDIO_HINT));
        assert_eq!(playback.toggle(), None, "no Play is emitted");
        assert_eq!(playback.seek(4_000), None, "seeking does nothing either");
        assert_eq!(playback.position_ms(), 0);
        assert_eq!(playback.stop(), None);
        assert!(!playback.is_playing());

        // The source is still unavailable after a stop: the hint stays visible.
        assert_eq!(playback.hint(), Some(KEEP_AUDIO_HINT));
    }

    #[test]
    fn a_device_failure_is_retryable_but_a_source_failure_is_not() {
        // A device that went away: play retries from where the playhead is.
        let mut playback = playing(10_000);
        playback.on_status(3_000, 10_000, false);
        playback.on_failure("no audio output device on this machine".to_string(), true);
        assert!(
            playback.can_play(),
            "a device error keeps the control armed"
        );
        assert_eq!(
            playback.hint(),
            Some("no audio output device on this machine")
        );
        assert_eq!(playback.toggle(), Some(Command::Play { from_ms: 3_000 }));

        // A source that cannot be read: the control is dead until reopened.
        let mut playback = Playback::new(ready(), 10_000);
        playback.on_failure("could not read the audio: bad header".to_string(), false);
        assert!(!playback.can_play());
        assert_eq!(playback.toggle(), None);
    }

    #[test]
    fn seek_clamps_and_only_commands_a_live_session() {
        let mut playback = Playback::new(ready(), 10_000);
        // Idle: the position is remembered for the next Play (no command).
        assert_eq!(playback.seek(2_500), None);
        assert_eq!(playback.position_ms(), 2_500);
        assert_eq!(playback.toggle(), Some(Command::Play { from_ms: 2_500 }));

        // Playing: the engine is told; out-of-range seeks clamp to the duration.
        assert_eq!(playback.seek(99_000), Some(Command::Seek { ms: 10_000 }));
        assert_eq!(playback.position_ms(), 10_000);

        // Paused: the engine is told and stays paused.
        assert_eq!(playback.toggle(), Some(Command::Pause));
        assert_eq!(playback.seek(1_200), Some(Command::Seek { ms: 1_200 }));
        assert!(playback.is_paused());
        assert_eq!(playback.toggle(), Some(Command::Resume));
    }

    #[test]
    fn status_moves_the_playhead_only_while_a_session_is_live() {
        let mut playback = Playback::new(ready(), 2_000);
        // Idle: a stale poll (an engine that outlived a stop) is ignored.
        playback.on_status(1_500, 2_000, false);
        assert_eq!(playback.position_ms(), 0);

        let mut playback = playing(2_000);
        playback.on_status(1_500, 2_000, false);
        assert_eq!(playback.position_ms(), 1_500);
        playback.toggle(); // pause
        playback.on_status(1_700, 2_000, false);
        assert_eq!(playback.position_ms(), 1_700, "paused follows the engine");

        // The engine's real duration replaces the transcript estimate.
        playback.on_status(1_700, 12_345, false);
        assert_eq!(playback.duration_ms(), 12_345);
    }

    #[test]
    fn the_end_of_the_audio_parks_the_playhead_and_replays_from_the_start() {
        let mut playback = playing(2_000);
        playback.on_status(2_000, 2_000, true);
        assert!(playback.is_idle(), "finished → idle");
        assert_eq!(playback.position_ms(), 2_000, "parked at the end");

        // Play again from the top rather than resuming at the end.
        assert_eq!(playback.toggle(), Some(Command::Play { from_ms: 0 }));

        // A stale `finished` poll after a stop is ignored.
        playback.stop();
        playback.on_status(2_000, 2_000, true);
        assert!(playback.is_idle());
        assert_eq!(playback.position_ms(), 0);
    }

    #[test]
    fn switching_sources_rebuilds_the_transport() {
        let mut playback = playing(10_000);
        assert_eq!(playback.stop(), Some(Command::Stop));

        // A new transcript: the transport is rebuilt from its own resolution.
        playback = Playback::new(
            PlaybackSource::Unavailable {
                hint: "gone".to_string(),
            },
            500,
        );
        assert!(!playback.can_play());
        assert_eq!(playback.duration_ms(), 500);
        assert!(Playback::none().hint().is_some());
    }
}
