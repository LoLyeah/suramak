use thiserror::Error;

/// Single error type for all suramak-core operations.
#[derive(Debug, Error)]
pub enum SuramakError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML deserialize error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("Symphonia error: {0}")]
    Symphonia(#[from] symphonia::core::errors::Error),

    #[error("Rubato error: {0}")]
    Rubato(#[from] rubato::ResampleError),

    /// A resampler could not be constructed for the input's rate/channel shape.
    ///
    /// Its own variant rather than a `Polish`/`Rubato` string: this is a property
    /// of the *audio* (a container claiming an impossible rate), not of an LLM
    /// backend, so the user-facing message must not read like a polish failure
    /// (SUR-LOW-01).
    #[error("Resampler initialization error: {0}")]
    ResamplerInit(String),

    #[error("Checksum mismatch — expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("zip error: {0}")]
    Zip(String),

    /// A multi-file download (M10A: the Parakeet model directory) did not end
    /// with the expected layout on disk.
    #[error("download error: {0}")]
    Download(String),

    /// The download was stopped through its progress callback (M14e: the
    /// in-app Cancel on a model download). The bytes already on disk stay in
    /// `<file>.part`, so the next attempt resumes where this one stopped.
    #[error("cancelled")]
    Cancelled,

    /// A model download was refused before the first byte because the target
    /// filesystem cannot hold it (M13d: the phone's free-space preflight).
    /// `message` is the user-facing sentence and names both numbers.
    #[error("{message}")]
    NotEnoughSpace {
        dir: std::path::PathBuf,
        needed: u64,
        available: u64,
        message: String,
    },

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Transcript not found: {0}")]
    TranscriptNotFound(String),

    /// A library database carries a schema this build cannot upgrade or read
    /// (a newer build's version, or an unreadable version marker).
    #[error("database schema error: {0}")]
    Schema(String),

    /// A library backup could not be written, validated or restored.
    #[error("library backup error: {0}")]
    Backup(String),

    /// A segment split/merge was refused by the pure validation rules; the
    /// database was not touched.
    #[error("segment edit refused: {0}")]
    SegmentEdit(#[from] crate::segment_edit::EditError),

    #[error("Polish backend error: {0}")]
    Polish(String),

    #[error("Bad polish response: {0}")]
    BadResponse(String),

    #[error("No audio data in file")]
    NoAudioData,

    /// A container that declares its length, but the bytes ended before that
    /// length was decoded (a cut-off download, a half-copied voice note).
    /// Distinct from [`SuramakError::UnsupportedFormat`]: the file *is* the
    /// right format, it is incomplete.
    #[error(
        "cannot decode {name}: the file is truncated ({detail}) — \
         download or copy it again"
    )]
    Truncated { name: String, detail: String },

    /// A third-party decoder panicked on malformed input. The decode boundary
    /// catches the unwind and reports this instead of taking the app down; the
    /// file is corrupt or not what its extension claims.
    #[error(
        "cannot decode {name}: the audio decoder failed unexpectedly \
         (the file may be corrupt) — {detail}"
    )]
    DecoderPanic { name: String, detail: String },

    /// A file the decode path cannot handle — an unsupported container or an
    /// audio codec without a decoder (e.g. Opus in WebM/MKV). The message is
    /// user-facing and actionable: it names a working alternative.
    #[error(
        "cannot decode {name}: {detail} — convert it first, \
         for example: ffmpeg -i \"{name}\" \"{suggested}\""
    )]
    UnsupportedFormat {
        /// File name (not the full path) as shown to the user.
        name: String,
        /// What exactly is unsupported: the container or the audio codec.
        detail: String,
        /// Suggested conversion target (same stem, `.mp4`).
        suggested: String,
    },

    #[error("Keychain error: {0}")]
    Keychain(String),

    #[error("API key not set — set SURAMAK_OPENAI_API_KEY or configure keychain")]
    ApiKeyNotSet,

    #[error("No app data directory available")]
    NoDataDir,
}
