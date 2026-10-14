//! Suramak playback (M8): play the transcript's source audio inside the editor.
//!
//! Three layers, all unit-tested headless:
//!
//! * [`source`] — the pure source-resolution matrix: where a transcript's audio
//!   is, or the hint explaining that it is gone (recording with `keep_audio`
//!   off) — no DB field involved, the kept path *is* `transcript.audio_path`.
//! * [`segments`] — the position → active-segment binary search and the `mm:ss`
//!   readout format.
//! * [`transport`] — the playback state machine (`Idle → Playing → Paused →`
//!   `stop-on-leave`, plus the unavailable path) and the command protocol.
//! * [`peaks`] — the min/max envelope of a decoded source (M9b): computed once
//!   per source load, drawn by the editor's deck strip.
//!
//! [`engine`] is the cpal half: one `suramak-playback` thread owns the decoded
//! audio and the output stream; the UI thread only sends [`Command`]s and polls
//! [`Engine::poll`] — the same poll-a-shared-status pattern as the capture RMS
//! meter. Failure paths (no output device, missing/gone source, device lost
//! mid-play) are reported as values, never panics: the headless VPS and CI have
//! no output device at all.

pub mod engine;
pub mod peaks;
pub mod segments;
pub mod source;
pub mod transport;

pub use engine::{pick_output_rate, Engine, EngineError, RateRange, Status};
pub use peaks::{Peaks, DEFAULT_BUCKETS as PEAKS_BUCKETS};
pub use segments::{format_mmss, segment_index_at};
pub use source::{resolve_playback_source, transcript_duration_ms, PlaybackSource};
pub use transport::{Command, Playback, PlaybackState};
