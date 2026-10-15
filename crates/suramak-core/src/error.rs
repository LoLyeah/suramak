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

    #[error("Checksum mismatch — expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("zip error: {0}")]
    Zip(String),

    /// A multi-file download (M10A: the Parakeet model directory) did not end
    /// with the expected layout on disk.
    #[error("download error: {0}")]
    Download(String),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Transcript not found: {0}")]
    TranscriptNotFound(String),

    #[error("Polish backend error: {0}")]
    Polish(String),

    #[error("Bad polish response: {0}")]
    BadResponse(String),

    #[error("No audio data in file")]
    NoAudioData,

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
