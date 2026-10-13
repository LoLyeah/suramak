//! Where a transcript's audio is — or why it cannot be played (M8).
//!
//! Resolution is pure: `transcript.audio_path` already carries the path for both
//! imported files and recordings (kept and temp alike), so **no DB field and no
//! migration are needed**. `keep_audio` off deletes the temp recording once the
//! transcript is saved, and the row's source kind (`Source::Recording`) is what
//! tells the not-kept case apart from a moved/deleted import.

use std::path::{Path, PathBuf};

use suramak_core::transcript::{Source, Transcript};

/// Hint for a recording whose audio was not kept (`keep_audio` off: the temp
/// file is deleted once the transcript is saved).
pub const KEEP_AUDIO_HINT: &str = "keep audio was off — the recording's file is gone \
(turn on “Keep recordings” in Settings)";

/// Hint for an imported file whose source audio is no longer on disk.
pub const MISSING_FILE_HINT: &str = "the source audio is gone — import the file again to play";

/// Hint for a row that never had a path (e.g. a transcript made by another tool).
pub const NO_AUDIO_HINT: &str = "no audio was saved for this transcript — import the file to play";

/// Resolved playable audio for the open transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackSource {
    /// The audio exists on disk and can be played.
    Ready { path: PathBuf },
    /// Playback is impossible; `hint` is the user-facing reason.
    Unavailable { hint: String },
}

impl PlaybackSource {
    /// The path to play, when the source is ready.
    pub fn ready_path(&self) -> Option<&Path> {
        match self {
            PlaybackSource::Ready { path } => Some(path),
            PlaybackSource::Unavailable { .. } => None,
        }
    }

    /// Whether audio can be played at all (the file is there).
    pub fn is_ready(&self) -> bool {
        matches!(self, PlaybackSource::Ready { .. })
    }

    /// The reason playback is impossible, when it is.
    pub fn hint(&self) -> Option<&str> {
        match self {
            PlaybackSource::Ready { .. } => None,
            PlaybackSource::Unavailable { hint } => Some(hint),
        }
    }
}

/// Resolves the source audio for `transcript` (pure; no engine is opened):
///
/// | transcript | file state | result |
/// |---|---|---|
/// | imported file | `audio_path` exists | `Ready` |
/// | imported file | path missing / absent | `Unavailable` (re-import) |
/// | recording (kept) | `audio_path` exists | `Ready` |
/// | recording (`keep_audio` off) | temp file deleted | `Unavailable` (keep audio) |
pub fn resolve_playback_source(transcript: &Transcript) -> PlaybackSource {
    if let Some(path) = transcript.audio_path.as_deref() {
        if path.is_file() {
            return PlaybackSource::Ready {
                path: path.to_path_buf(),
            };
        }
    }
    let hint = match &transcript.source {
        // The temp recording was deleted (keep_audio off) — and for a recording
        // whose path is still set but gone, the same hint is the right one.
        Source::Recording { .. } => KEEP_AUDIO_HINT,
        Source::File { .. } => {
            if transcript.audio_path.is_some() {
                MISSING_FILE_HINT
            } else {
                NO_AUDIO_HINT
            }
        }
    };
    PlaybackSource::Unavailable {
        hint: hint.to_string(),
    }
}

/// The transcript's own duration estimate: the end of its last segment (what the
/// waveform strip shows before the engine has loaded the file and reported the
/// real duration). Hours fold into the minutes field, like the `mm:ss` readout.
pub fn transcript_duration_ms(transcript: &Transcript) -> u32 {
    transcript
        .segments
        .last()
        .map(|segment| segment.end_ms.clamp(0, i64::from(u32::MAX)) as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use suramak_core::transcript::Segment;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-playback-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn segment(idx: u32, end_ms: i64) -> Segment {
        Segment {
            idx,
            start_ms: i64::from(idx) * 1_000,
            end_ms,
            text: format!("segment {idx}"),
        }
    }

    fn transcript(source: Source, audio_path: Option<PathBuf>) -> Transcript {
        Transcript::new(
            "clip.wav".to_string(),
            source,
            "tiny".to_string(),
            Some("en".to_string()),
            audio_path,
            vec![segment(0, 900), segment(1, 1_900)],
        )
    }

    fn recording() -> Source {
        Source::Recording {
            started_at: chrono::Utc::now(),
            duration_ms: 1_900,
        }
    }

    /// Writes a real file so `is_file()` decides.
    fn existing(tag: &str) -> PathBuf {
        let path = temp_dir().join(format!("{tag}.wav"));
        std::fs::write(&path, b"RIFF....").expect("temp wav");
        path
    }

    #[test]
    fn an_imported_file_plays_when_it_is_still_there() {
        let path = existing("imported");
        let source = resolve_playback_source(&transcript(
            Source::File {
                name: "clip.wav".to_string(),
            },
            Some(path.clone()),
        ));
        assert_eq!(source, PlaybackSource::Ready { path: path.clone() });
        assert_eq!(source.ready_path(), Some(path.as_path()));
    }

    #[test]
    fn an_imported_file_that_is_gone_asks_for_a_re_import() {
        let gone = temp_dir().join("moved-away.wav");
        std::fs::remove_file(&gone).ok();
        let source = resolve_playback_source(&transcript(
            Source::File {
                name: "clip.wav".to_string(),
            },
            Some(gone),
        ));
        let hint = source.hint().expect("unavailable");
        assert!(hint.contains("import"), "unexpected hint: {hint}");
        assert!(!source.is_ready());
    }

    #[test]
    fn a_row_without_a_path_has_no_audio() {
        let source = resolve_playback_source(&transcript(
            Source::File {
                name: "clip.wav".to_string(),
            },
            None,
        ));
        assert_eq!(
            source,
            PlaybackSource::Unavailable {
                hint: NO_AUDIO_HINT.to_string()
            }
        );
    }

    #[test]
    fn a_kept_recording_plays_from_its_stored_path() {
        let path = existing("kept-recording");
        let source = resolve_playback_source(&transcript(recording(), Some(path.clone())));
        assert_eq!(source, PlaybackSource::Ready { path });
    }

    #[test]
    fn a_recording_with_keep_audio_off_hints_at_the_setting() {
        // The temp file was deleted after the save: the path is stale.
        let gone = temp_dir().join("temp-recording.wav");
        std::fs::remove_file(&gone).ok();
        let source = resolve_playback_source(&transcript(recording(), Some(gone)));
        let hint = source.hint().expect("unavailable");
        assert!(
            hint.contains("keep audio"),
            "the hint must name the setting: {hint}"
        );
        assert!(hint.contains("Keep recordings"), "unexpected hint: {hint}");
    }

    #[test]
    fn a_recording_without_a_path_hints_at_the_setting_too() {
        let source = resolve_playback_source(&transcript(recording(), None));
        assert_eq!(
            source.hint(),
            Some(KEEP_AUDIO_HINT),
            "a pathless recording is the keep_audio case, not the import case"
        );
    }

    #[test]
    fn the_duration_estimate_is_the_last_segment_end() {
        assert_eq!(
            transcript_duration_ms(&transcript(
                Source::File {
                    name: "clip.wav".to_string()
                },
                None
            )),
            1_900
        );
        let empty = Transcript::new(
            "empty.wav".to_string(),
            Source::File {
                name: "empty.wav".to_string(),
            },
            "tiny".to_string(),
            None,
            None,
            Vec::new(),
        );
        assert_eq!(transcript_duration_ms(&empty), 0);
    }
}
