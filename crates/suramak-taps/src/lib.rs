//! suramak-taps — **macOS system-audio capture via CoreAudio process taps**
//! (macOS 14.2+, no kernel drivers, no ScreenCaptureKit).
//!
//! Addendum A of WORKORDER-M3. A tap captures the audio of *other* processes
//! (here: every process except Suramak itself); the tap is then presented to the
//! system as a **private aggregate device** whose UID is returned to the caller.
//! `suramak-asr`'s capture layer opens that device through cpal and records it
//! like any other input.
//!
//! ## API shape (and why)
//!
//! [`SystemTap::start`] takes a [`TapConfig`] carrying a [`ProcessSelection`] —
//! *one parameter*, so the future per-app milestone ("only this Zoom call") is a
//! new `ProcessSelection` variant, not a reshape of this API.
//!
//! ```no_run
//! # fn main() -> Result<(), suramak_taps::TapError> {
//! let tap = suramak_taps::SystemTap::start(&suramak_taps::TapConfig::system_audio())?;
//! let uid = tap.device_uid();
//! // …record from the aggregate device…
//! // `stop()` (or Drop) tears the aggregate device down first, then the tap;
//! // both are idempotent.
//! # let _ = uid;
//! # Ok(())
//! # }
//! ```
//!
//! ## Platforms
//!
//! The macOS implementation lives in [`macos`]; every other platform compiles a
//! no-op stub ([`stub`]) whose `start` returns [`TapError::Unsupported`], so
//! dependents can call this unconditionally. The crate is **pure Rust** (objc2
//! message sends + CoreAudio `extern "C"` declarations, system-framework linkage
//! only) — `cargo check -p suramak-taps --target aarch64-apple-darwin` runs from
//! Linux, the same checkability story as `suramak-glass`.
//!
//! ## Honest status
//!
//! Runtime behavior (tap creation, the TCC audio-capture prompt, aggregate
//! device enumeration) can only be exercised on macOS — see
//! `docs/reports/M3.md`. The Linux error path and the config shapes are
//! unit-tested here.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(not(target_os = "macos"))]
use stub as imp;

/// Which processes a tap captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessSelection {
    /// Every process **except** the listed pids. An empty list captures everything.
    ///
    /// The per-app milestone ("only this Zoom call") adds a sibling variant here;
    /// [`TapConfig`] already carries the selection as one parameter, so that is a
    /// one-line extension of `macos.rs`, not an API reshape.
    AllExcept(Vec<u32>),
}

/// Configuration for one system-audio tap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapConfig {
    /// Which processes to capture (see [`ProcessSelection`]).
    pub processes: ProcessSelection,
    /// Name given to the private aggregate device. It is the name the device
    /// shows up under in input-device enumeration, so keep it distinctive.
    pub name: String,
}

/// Default aggregate-device name (`Suramak System Audio`).
pub const DEFAULT_TAP_DEVICE_NAME: &str = "Suramak System Audio";

impl TapConfig {
    /// System audio = every process except Suramak itself.
    pub fn system_audio() -> Self {
        Self::system_audio_excluding(std::process::id())
    }

    /// System audio excluding `pid` (callers pass their own process id).
    pub fn system_audio_excluding(pid: u32) -> Self {
        Self {
            processes: ProcessSelection::AllExcept(vec![pid]),
            name: DEFAULT_TAP_DEVICE_NAME.to_string(),
        }
    }
}

/// Errors raised by tap setup/teardown.
#[derive(Debug, thiserror::Error)]
pub enum TapError {
    /// CoreAudio process taps do not exist on this platform (non-macOS builds).
    #[error("system audio capture is not supported on this platform (CoreAudio process taps are macOS 14.2+)")]
    Unsupported,
    /// macOS older than 14.2: the `CATapDescription` class / tap entry points are absent.
    #[error("system audio capture requires macOS 14.2 or newer (CATapDescription is unavailable)")]
    OsTooOld,
    /// A CoreAudio call returned a non-zero `OSStatus`.
    #[error("CoreAudio call {call} failed (OSStatus {status})")]
    CoreAudio { call: &'static str, status: i32 },
    /// Setup reached CoreAudio but the result was unusable.
    #[error("system audio tap setup failed: {0}")]
    Setup(String),
}

/// A live system-audio tap plus its private aggregate device.
///
/// Dropping the value tears both down (aggregate device first, then the tap) —
/// exactly once; [`SystemTap::stop`] is idempotent.
pub struct SystemTap {
    inner: imp::TapInner,
}

impl SystemTap {
    /// Creates the tap and its private aggregate device. Fails with
    /// [`TapError::Unsupported`] off macOS and [`TapError::OsTooOld`] on macOS
    /// older than 14.2.
    pub fn start(config: &TapConfig) -> Result<SystemTap, TapError> {
        Ok(SystemTap {
            inner: imp::TapInner::start(config)?,
        })
    }

    /// CoreAudio UID of the private aggregate device (the tap's recording endpoint).
    pub fn device_uid(&self) -> &str {
        self.inner.device_uid()
    }

    /// Name the aggregate device was created with (what device enumeration shows).
    pub fn device_name(&self) -> &str {
        self.inner.device_name()
    }

    /// CoreAudio device id the UID currently resolves to (`None` off macOS or when
    /// the system has not registered the device yet).
    pub fn device_id(&self) -> Option<u32> {
        self.inner.device_id()
    }

    /// Tears down the aggregate device, then the tap. Idempotent.
    pub fn stop(&mut self) -> Result<(), TapError> {
        self.inner.stop()
    }
}

impl Drop for SystemTap {
    fn drop(&mut self) {
        let _ = self.inner.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_audio_config_excludes_only_the_caller() {
        let config = TapConfig::system_audio();
        assert_eq!(
            config.processes,
            ProcessSelection::AllExcept(vec![std::process::id()])
        );
        assert_eq!(config.name, DEFAULT_TAP_DEVICE_NAME);

        let explicit = TapConfig::system_audio_excluding(4242);
        assert_eq!(explicit.processes, ProcessSelection::AllExcept(vec![4242]));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn start_reports_unsupported_off_macos() {
        let err = SystemTap::start(&TapConfig::system_audio())
            .err()
            .expect("tap start must fail off macOS");
        assert!(matches!(err, TapError::Unsupported));
        assert!(
            err.to_string().contains("macOS 14.2"),
            "the error must say what is missing: {err}"
        );
    }
}
