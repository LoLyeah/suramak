//! Reveal a file or folder in the platform file manager.
//!
//! M14b: the app's first reveal affordance is the Windows-only "Show in
//! Explorer" action on the Settings → Recordings folder row. macOS has no reveal
//! action today, so there is deliberately no `NSWorkspace` port here — this
//! module is the Windows half, plus an honest `Unsupported` stub so a future
//! caller on another platform cannot silently do nothing.
//!
//! The Windows form is `explorer.exe /select,<path>`: one argument, `explorer`'s
//! own syntax (`/select,` glued to the path, not two switches), which opens the
//! containing folder with the item selected — for a folder, its parent with the
//! folder selected. `std::process::Command` quotes the argument when the path
//! contains spaces, which `explorer`'s parser accepts.
//!
//! The module is compiled on **every** target: the spawn is `cfg(windows)`, the
//! stub elsewhere reports `Unsupported`, and the one caller checks
//! `cfg!(windows)` (the `SYSTEM_AUDIO_OFFERED` pattern in `app/settings.rs`) — so
//! the host build type-checks the whole call path and the pure argument builder is
//! unit-tested everywhere.

use std::ffi::OsString;
use std::io;
use std::path::Path;

/// The single argument `explorer.exe` needs to reveal `path`: `"/select,<path>"`.
///
/// Pure and separate from the spawn so the shape is unit-tested: one argument,
/// never two, so a spaced path stays one token.
pub(crate) fn explorer_args(path: &Path) -> Vec<OsString> {
    vec![OsString::from(format!("/select,{}", path.display()))]
}

/// The `explorer.exe` to spawn: `%SystemRoot%\explorer.exe` when the environment
/// names a system root, else the bare name.
///
/// SUR-LOW-03: an unqualified `explorer.exe` is resolved through `PATH`, so a
/// planted `explorer.exe` in a user-writable `PATH` entry would run instead of the
/// shell. Naming the system copy absolutely removes the search entirely; the bare
/// fallback only exists for an environment with no `SystemRoot` at all (where PATH
/// lookup is the only option left).
///
/// Pure so the rule is host-tested; only the Windows spawn calls it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn explorer_exe(system_root: Option<&std::ffi::OsStr>) -> std::path::PathBuf {
    match system_root {
        Some(root) if !root.is_empty() => Path::new(root).join("explorer.exe"),
        _ => std::path::PathBuf::from("explorer.exe"),
    }
}

/// Opens the containing folder with `path` selected.
///
/// Spawns and returns immediately — `explorer.exe` hands the request to the
/// running shell and exits. A missing path is not an error here: `explorer`
/// opens its default folder instead (the caller creates the folder it reveals).
#[cfg(windows)]
pub(crate) fn reveal_in_folder(path: &Path) -> io::Result<()> {
    std::process::Command::new(explorer_exe(std::env::var_os("SystemRoot").as_deref()))
        .args(explorer_args(path))
        // `explorer` detaches; without this a console build would wait for it.
        .spawn()
        .map(|_| ())
}

/// Non-Windows: no reveal action exists (macOS has none either), so say so —
/// naming the command that *would* have run — instead of pretending it worked.
#[cfg(not(windows))]
pub(crate) fn reveal_in_folder(path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "reveal-in-folder is Windows-only (M14b); would run `explorer.exe {}`",
            explorer_args(path)[0].to_string_lossy()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/select,` and the path are ONE argument — the whole point of `explorer`'s
    /// syntax — and a path with spaces in it does not split into two.
    #[test]
    fn the_explorer_argument_is_one_select_token() {
        let args = explorer_args(Path::new(
            r"C:\Users\Dzikri A\AppData\Local\Suramak\recordings",
        ));
        assert_eq!(
            args.len(),
            1,
            "explorer takes /select,<path> as one argument"
        );
        assert_eq!(
            args[0],
            OsString::from(r"/select,C:\Users\Dzikri A\AppData\Local\Suramak\recordings")
        );
        assert!(
            !args[0].to_string_lossy().contains('"'),
            "no manual quoting"
        );
    }

    /// SUR-LOW-03: the system copy of explorer is named absolutely, so a planted
    /// `explorer.exe` on `PATH` cannot be the process that runs.
    #[test]
    fn the_explorer_executable_is_qualified_with_the_system_root() {
        let qualified = explorer_exe(Some(std::ffi::OsStr::new(r"C:\Windows")));
        assert_eq!(qualified, Path::new(r"C:\Windows").join("explorer.exe"));
        assert!(
            qualified.to_string_lossy().contains(r"C:\Windows"),
            "the spawn target must name the system root: {}",
            qualified.display()
        );
        // No usable environment value → the old bare name (PATH lookup) remains.
        assert_eq!(explorer_exe(None), std::path::PathBuf::from("explorer.exe"));
        assert_eq!(
            explorer_exe(Some(std::ffi::OsStr::new(""))),
            std::path::PathBuf::from("explorer.exe")
        );
    }

    /// Non-Windows reports `Unsupported` rather than a silent success.
    #[cfg(not(windows))]
    #[test]
    fn the_non_windows_stub_is_unsupported() {
        let err = reveal_in_folder(Path::new("/tmp")).expect_err("no reveal off Windows");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}
