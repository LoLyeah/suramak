use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// A single transcribed time-aligned segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub idx: u32,
    pub start_ms: i64,
    pub end_ms: i64,
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
