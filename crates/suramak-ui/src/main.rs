// M14b: the GUI binary is a Windows-subsystem app so launching it never opens a
// console window (the CLI, `suramak-cli`, stays a console binary). The
// attribute must sit at the crate root (main.rs) because desktop.rs became a
// module in the split.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! Desktop entry point for Suramak.
//!
//! The whole desktop shell lives in [`desktop`] and is compiled on every
//! non-Android target only. Android loads the `suramak-android` cdylib instead;
//! this binary target exists there solely so `cargo clippy --all-targets` for
//! `aarch64-linux-android` has a valid (empty) entry point — the phone never
//! runs a winit desktop shell.
//!
//! `theme` and `motion` are re-exported from the shared Deck library
//! (`suramak_ui::deck`), so the desktop keeps its historical `crate::theme` /
//! `crate::motion` paths while the phone draws the same token ladder and
//! motion vocabulary.

#[cfg(not(target_os = "android"))]
mod desktop;

#[cfg(not(target_os = "android"))]
// Some of these modules are only named through `crate::…` in `cfg(test)`
// blocks, so the binary build itself does not mention every re-export.
#[allow(unused_imports)]
pub(crate) use desktop::{
    app, framestats, hud, pending_transcription, reveal, runtime, shell, shutdown, updates,
    UserEvent,
};

// The single-instance claim is internal to the desktop shell
// (`desktop::instance_windows` is `pub(crate)`): no crate-root re-export —
// desktop.rs consumes it directly, and no `crate::instance_windows` path
// exists anywhere.

pub use suramak_ui::deck::{motion, theme};

#[cfg(target_os = "android")]
fn main() {
    // Android launches `suramak_android::android_main` through the cdylib, not
    // this binary. Nothing to do; kept as a valid target for `--all-targets`.
}

#[cfg(not(target_os = "android"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    desktop::run()
}
