use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// A single transcribed time-aligned segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub idx: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// One word of a segment with its estimated time range (M15.6).
///
/// This is **estimated alignment**, not forced alignment: the times are
/// whisper's token-level timestamp estimates (10 ms granularity) mapped onto
/// whole words, and they exist only for text the engine itself produced. An
/// edited segment has no usable word timing until it is re-aligned — the
/// store says so with a sticky stale marker, and the UI falls back to
/// segment seeking. Nothing in the product may present these as exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordTiming {
    /// The segment this word belongs to.
    pub segment_idx: u32,
    /// Position of the word inside its segment, dense from 0.
    pub word_idx: u32,
    /// Estimated start in milliseconds, inside the segment's own span.
    pub start_ms: i64,
    /// Estimated end in milliseconds (`>= start_ms`, inside the segment span).
    pub end_ms: i64,
    /// The word exactly as it appears in the segment text (punctuation
    /// attached, whitespace removed).
    pub text: String,
}

/// Where the audio came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    File {
        name: String,
    },
    Recording {
        started_at: DateTime<Utc>,
        duration_ms: i64,
    },
}

/// A complete transcription result stored in the library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source: Source,
    pub model: String,
    pub language: Option<String>,
    pub audio_path: Option<PathBuf>,
    pub segments: Vec<Segment>,
}

impl Transcript {
    /// Creates a new transcript with a generated UUID and current timestamp.
    pub fn new(
        title: String,
        source: Source,
        model: String,
        language: Option<String>,
        audio_path: Option<PathBuf>,
        segments: Vec<Segment>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title,
            created_at: now,
            updated_at: now,
            source,
            model,
            language,
            audio_path,
            segments,
        }
    }
}
