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

    #[error("Keychain error: {0}")]
    Keychain(String),

    #[error("API key not set — set SURAMAK_OPENAI_API_KEY or configure keychain")]
    ApiKeyNotSet,

    #[error("No app data directory available")]
    NoDataDir,
}
