use directories::ProjectDirs;
use std::path::PathBuf;

use crate::SuramakError;

fn project_dirs() -> Result<ProjectDirs, SuramakError> {
    ProjectDirs::from("dev", "suramak", "Suramak").ok_or(SuramakError::NoDataDir)
}

/// Returns the application data directory (platform-specific).
pub fn data_dir() -> Result<PathBuf, SuramakError> {
    Ok(project_dirs()?.data_dir().to_path_buf())
}

/// Returns the models directory (`data_dir/models/`).
pub fn models_dir() -> Result<PathBuf, SuramakError> {
    Ok(data_dir()?.join("models"))
}

/// Returns the audio recordings directory (`data_dir/audio/`).
pub fn audio_dir() -> Result<PathBuf, SuramakError> {
    Ok(data_dir()?.join("audio"))
}

/// Returns the path to the SQLite database file.
pub fn db_path() -> Result<PathBuf, SuramakError> {
    Ok(data_dir()?.join("suramak.db"))
}

/// Returns the path to the settings TOML file.
pub fn settings_path() -> Result<PathBuf, SuramakError> {
    Ok(data_dir()?.join("settings.toml"))
}

/// Creates all required directories, returning the data dir path.
pub fn ensure_dirs() -> Result<PathBuf, SuramakError> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(models_dir()?)?;
    std::fs::create_dir_all(audio_dir()?)?;
    Ok(dir)
}
