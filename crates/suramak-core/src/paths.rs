use directories::ProjectDirs;
use std::path::PathBuf;

use crate::SuramakError;

fn project_dirs() -> Result<ProjectDirs, SuramakError> {
    ProjectDirs::from("dev", "suramak", "Suramak").ok_or(SuramakError::NoDataDir)
}

/// Returns the application data directory (platform-specific).
///
/// On Android the directory is supplied by the shell (the `crate::android`
/// seam); `directories::ProjectDirs` has no Android backend, so without the
/// seam this would always fail there.
pub fn data_dir() -> Result<PathBuf, SuramakError> {
    #[cfg(target_os = "android")]
    if let Some(dir) = crate::android::data_dir_override() {
        return Ok(dir);
    }
    platform_data_dir()
}

/// The desktop data directory for the target platform.
///
/// Windows (M14b): `%LOCALAPPDATA%\Suramak` — deliberately **not** what
/// `ProjectDirs::data_dir()` returns there (`%APPDATA%` = Roaming, plus
/// `\suramak\Suramak\data`). Models are hundreds of megabytes and must not
/// roam with a domain profile, and the SQLite library must not sit in a
/// roamed/synced profile (PLAN §Packaging). One directory for everything the
/// app owns: `models\`, `audio\`, `recordings\`, `suramak.db`, `settings.toml`.
#[cfg(windows)]
fn platform_data_dir() -> Result<PathBuf, SuramakError> {
    if let Some(dir) = windows_data_dir(std::env::var_os("LOCALAPPDATA").as_deref()) {
        return Ok(dir);
    }
    // No `%LOCALAPPDATA%` (never in a normal interactive logon): `directories`'
    // *local* app-data root — still non-roaming, unlike `data_dir()`.
    Ok(project_dirs()?.data_local_dir().to_path_buf())
}

#[cfg(not(windows))]
fn platform_data_dir() -> Result<PathBuf, SuramakError> {
    Ok(project_dirs()?.data_dir().to_path_buf())
}

/// The Windows data-directory rule, as a pure function: `<LOCALAPPDATA>\Suramak`.
///
/// `None` when the variable is absent or empty — the caller then falls back to
/// `directories`' local root. Pure (the value is passed in, never read here) so
/// the rule — including non-ASCII and spaced user profiles — is unit-tested on
/// every host, not only on Windows.
#[cfg(any(windows, test))]
pub(crate) fn windows_data_dir(local_app_data: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let root = local_app_data.filter(|value| !value.is_empty())?;
    Some(PathBuf::from(root).join("Suramak"))
}

/// Returns the models directory (`data_dir/models/`).
pub fn models_dir() -> Result<PathBuf, SuramakError> {
    Ok(data_dir()?.join("models"))
}

/// Returns the audio recordings directory (`data_dir/audio/`).
pub fn audio_dir() -> Result<PathBuf, SuramakError> {
    Ok(data_dir()?.join("audio"))
}

/// Returns the microphone-recordings directory (`data_dir/recordings/`).
///
/// This is where kept recordings land when `settings.storage.keep_audio` is on
/// (M3); with `keep_audio` off they go to the system temp dir and are deleted
/// after the transcript is saved.
pub fn recordings_dir() -> Result<PathBuf, SuramakError> {
    Ok(data_dir()?.join("recordings"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// M14b: the Windows rule is `<LOCALAPPDATA>\Suramak` — one component, no
    /// `\data` tail (the shape `ProjectDirs` would have produced), and every
    /// derived path hangs off it.
    #[test]
    fn windows_data_dir_is_local_app_data_plus_suramak() {
        let local = std::ffi::OsStr::new(r"C:\Users\owner\AppData\Local");
        let dir = windows_data_dir(Some(local)).expect("a non-empty root resolves");
        assert_eq!(dir, PathBuf::from(local).join("Suramak"));
        assert!(
            !dir.ends_with("data"),
            "no `\\data` component — {dir:?} must be the app root itself"
        );
    }

    /// PLAN §Testing: non-ASCII user names, spaces, and paths over 260 chars
    /// (the manifest declares `longPathAware`; the rule must not truncate).
    #[test]
    fn windows_data_dir_keeps_non_ascii_spaces_and_long_paths() {
        let non_ascii = std::ffi::OsStr::new(r"C:\Users\Dzikrī\AppData\Local");
        assert_eq!(
            windows_data_dir(Some(non_ascii)),
            Some(PathBuf::from(non_ascii).join("Suramak"))
        );

        let spaced = std::ffi::OsStr::new(r"C:\Users\Dzikri A\AppData\Local");
        assert_eq!(
            windows_data_dir(Some(spaced)),
            Some(PathBuf::from(spaced).join("Suramak"))
        );

        let deep = format!(r"C:\Users\owner\{}\AppData\Local", "x".repeat(300));
        let dir = windows_data_dir(Some(std::ffi::OsStr::new(&deep))).expect("long path resolves");
        assert!(
            dir.to_string_lossy().len() > 260,
            "the rule must not truncate a long root: {dir:?}"
        );
        assert!(dir.ends_with("Suramak"));
    }

    /// An absent or empty `%LOCALAPPDATA%` is `None` — the caller falls back to
    /// `directories`' *local* root rather than inventing a path.
    #[test]
    fn windows_data_dir_refuses_an_absent_or_empty_root() {
        assert_eq!(windows_data_dir(None), None);
        assert_eq!(windows_data_dir(Some(std::ffi::OsStr::new(""))), None);
    }

    /// The real environment: on Windows every path the app uses sits under
    /// `%LOCALAPPDATA%\Suramak`, and none of them is a roaming (`%APPDATA%`)
    /// path. This is the test the CI `windows` job runs (`-p suramak-core`).
    #[cfg(windows)]
    #[test]
    fn the_resolved_paths_are_local_and_never_roaming() {
        let local = std::env::var_os("LOCALAPPDATA").expect("%LOCALAPPDATA% in a Windows logon");
        let data = data_dir().expect("data dir resolves");
        assert_eq!(data, PathBuf::from(&local).join("Suramak"));
        assert_eq!(models_dir().unwrap(), data.join("models"));
        assert_eq!(audio_dir().unwrap(), data.join("audio"));
        assert_eq!(recordings_dir().unwrap(), data.join("recordings"));
        assert_eq!(db_path().unwrap(), data.join("suramak.db"));
        assert_eq!(settings_path().unwrap(), data.join("settings.toml"));
        let text = data.to_string_lossy();
        assert!(
            !text.contains("Roaming") && !text.contains(r"\suramak\Suramak"),
            "not the ProjectDirs roaming shape: {text}"
        );
    }
}
