//! The fast-engine readiness model (M14e) — **egui-free** and unit-tested: what
//! each fast engine's row says, what its dot means, and the copy around it.
//!
//! The readiness list replaced a checkbox plus a CLI hint. Nothing here changes
//! routing: Indonesian still never routes fast (the guard lives in
//! `suramak-asr::engine` and is pinned there), and the toggle's behaviour is
//! untouched. This module only *reports* what the machine can do, so the list
//! can never claim an engine is ready when it is not.

use std::path::Path;

use suramak_asr::speech_transcriber::{self, SttAuthorization};
use suramak_core::models;

/// The exact System Settings pane for Speech Recognition privacy — the
/// deep link the "Allow speech recognition…" control opens.
pub const STT_PERMISSION_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition";

/// The one quiet caption under the SpeechTranscriber row.
pub const STT_CAPTION: &str =
    "Managed by macOS — nothing to download; the OS keeps its models updated.";

/// The Parakeet coverage caption (the count must match the manifest; the test
/// below pins that).
pub const PARAKEET_COVERAGE_CAPTION: &str =
    "25 languages — English, Spanish, French… (open the list). Indonesian is always Whisper.";

/// The transcribe card's one-liner when the job's language is `auto`: why the
/// fast engine is idle. Pinned here so the card and the test cannot drift.
pub const AUTO_LANGUAGE_CAPTION: &str =
    "auto stays Whisper — fast engines need a forced covered language";

/// Apple SpeechTranscriber's readiness, as the row shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttStatus {
    /// The `speech` crate reports the OS-managed assets usable.
    Available,
    /// Compiled and the OS supports it, but the app may not use the microphone
    /// / speech recognition yet.
    NeedsPermission,
    /// Compiled, but this macOS predates the analyzer (needs macOS 26+).
    NeedsMacOs26,
    /// The `speech-transcriber` cargo feature is off in this build.
    NotInThisBuild,
}

impl SttStatus {
    pub fn label(self) -> &'static str {
        match self {
            SttStatus::Available => "Available",
            SttStatus::NeedsPermission => "Needs permission",
            SttStatus::NeedsMacOs26 => "Needs macOS 26",
            SttStatus::NotInThisBuild => "Not in this build",
        }
    }

    /// Whether the engine can run right now.
    pub fn ready(self) -> bool {
        matches!(self, SttStatus::Available)
    }

    /// Whether the row offers the inline "Allow speech recognition…" control
    /// (never a dead end: the button opens the exact pane).
    pub fn offers_permission_link(self) -> bool {
        matches!(self, SttStatus::NeedsPermission)
    }
}

/// The Parakeet model's install state, as the row shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParakeetStatus {
    /// Every pinned file is present (layout-checked).
    Installed,
    /// The directory exists but the layout is incomplete (an interrupted run).
    Partial,
    /// Nothing on disk.
    Missing,
    /// The `parakeet` cargo feature is off in this build.
    NotInThisBuild,
}

impl ParakeetStatus {
    pub fn label(self) -> &'static str {
        match self {
            ParakeetStatus::Installed => "Installed",
            ParakeetStatus::Partial => "Partial download",
            ParakeetStatus::Missing => "Not downloaded",
            ParakeetStatus::NotInThisBuild => "Not in this build",
        }
    }

    pub fn ready(self) -> bool {
        matches!(self, ParakeetStatus::Installed)
    }

    /// Whether the row offers the inline install control.
    pub fn offers_download(self) -> bool {
        matches!(self, ParakeetStatus::Missing | ParakeetStatus::Partial)
    }

    /// Whether the row offers the inline Delete control.
    pub fn offers_delete(self) -> bool {
        matches!(self, ParakeetStatus::Installed | ParakeetStatus::Partial)
    }
}

/// The pure SpeechTranscriber rule (all three inputs are probes; the test pins
/// every combination that matters).
pub fn stt_status(
    compiled: bool,
    os_available: bool,
    authorization: Option<SttAuthorization>,
) -> SttStatus {
    if !compiled {
        return SttStatus::NotInThisBuild;
    }
    if !os_available {
        return SttStatus::NeedsMacOs26;
    }
    match authorization {
        Some(SttAuthorization::Authorized) => SttStatus::Available,
        // Denied / restricted / not-determined / unknown: the user has not
        // granted access, so the row says so and offers the exact pane. (A
        // not-determined state would prompt on first use; the link is still the
        // honest control — it never claims a permission that is not there.)
        _ => SttStatus::NeedsPermission,
    }
}

/// The pure Parakeet rule.
pub fn parakeet_status(compiled: bool, installed: bool, dir_exists: bool) -> ParakeetStatus {
    if !compiled {
        return ParakeetStatus::NotInThisBuild;
    }
    if installed {
        ParakeetStatus::Installed
    } else if dir_exists {
        ParakeetStatus::Partial
    } else {
        ParakeetStatus::Missing
    }
}

/// The live readiness of this build on this machine.
///
/// Probed when Settings opens (and again after an install/delete) rather than
/// every frame: `os_available` / `authorization` cross the Swift bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastEngineReadiness {
    pub stt: SttStatus,
    pub parakeet: ParakeetStatus,
    /// False before the first probe (the list then draws nothing rather than
    /// guessing a state).
    pub probed: bool,
}

impl Default for FastEngineReadiness {
    fn default() -> Self {
        Self {
            stt: SttStatus::NotInThisBuild,
            parakeet: ParakeetStatus::NotInThisBuild,
            probed: false,
        }
    }
}

impl FastEngineReadiness {
    /// Probes this build + machine.
    pub fn probe(models_dir: &Path) -> Self {
        Self {
            stt: stt_status(
                speech_transcriber::compiled(),
                speech_transcriber::os_available(),
                speech_transcriber::authorization(),
            ),
            parakeet: parakeet_status(
                suramak_asr::parakeet::compiled(),
                models::parakeet_installed(models_dir),
                models::parakeet_dir(models_dir).exists(),
            ),
            probed: true,
        }
    }
}

/// A Parakeet coverage language's English name (`en` → `English`). Every code
/// in the manifest has one; the test pins that.
pub fn language_name(code: &str) -> String {
    let name = match code {
        "bg" => "Bulgarian",
        "hr" => "Croatian",
        "cs" => "Czech",
        "da" => "Danish",
        "nl" => "Dutch",
        "en" => "English",
        "et" => "Estonian",
        "fi" => "Finnish",
        "fr" => "French",
        "de" => "German",
        "el" => "Greek",
        "hu" => "Hungarian",
        "it" => "Italian",
        "lv" => "Latvian",
        "lt" => "Lithuanian",
        "mt" => "Maltese",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "es" => "Spanish",
        "sv" => "Swedish",
        "ru" => "Russian",
        "uk" => "Ukrainian",
        other => other,
    };
    name.to_string()
}

/// The full coverage list, one language per line (`English (en)`), read from
/// the manifest — never hand-written.
pub fn parakeet_coverage_lines() -> Vec<String> {
    models::PARAKEET
        .languages
        .iter()
        .map(|code| format!("{} ({code})", language_name(code)))
        .collect()
}

/// One-line hover text for the coverage caption.
pub fn parakeet_coverage_hover() -> String {
    parakeet_coverage_lines().join(", ")
}

/// Opens the Speech Recognition pane of System Settings.
///
/// macOS only (the URL scheme is Apple's) — but the command path stays
/// **compiled on every target** and simply never runs elsewhere, so a typo
/// cannot hide behind a `cfg` that no Linux build ever compiles (the audit
/// pass-2 lesson: platform-gated code that only CI's mac job sees).
pub fn open_stt_permission_settings() -> std::io::Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the Speech Recognition privacy pane is macOS-only",
        ));
    }
    std::process::Command::new("open")
        .arg(STT_PERMISSION_URL)
        .status()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_readiness_reports_the_probe_honestly() {
        // Feature off → not in this build, whatever the OS says.
        assert_eq!(
            stt_status(false, true, Some(SttAuthorization::Authorized)),
            SttStatus::NotInThisBuild
        );
        // Compiled but the OS has no analyzer (macOS < 26).
        assert_eq!(
            stt_status(true, false, Some(SttAuthorization::Denied)),
            SttStatus::NeedsMacOs26
        );
        // Available only when the OS is and the permission is granted.
        assert_eq!(
            stt_status(true, true, Some(SttAuthorization::Authorized)),
            SttStatus::Available
        );
        for denied in [
            SttAuthorization::Denied,
            SttAuthorization::Restricted,
            SttAuthorization::NotDetermined,
            SttAuthorization::Unknown,
        ] {
            assert_eq!(
                stt_status(true, true, Some(denied)),
                SttStatus::NeedsPermission,
                "{denied:?} is not a granted permission"
            );
        }
        // No probe at all (a stub build) is also "needs permission", never
        // "available".
        assert_eq!(stt_status(true, true, None), SttStatus::NeedsPermission);
        // The permission link exists exactly where the status is "needs".
        assert!(SttStatus::NeedsPermission.offers_permission_link());
        for other in [SttStatus::NeedsMacOs26, SttStatus::NotInThisBuild] {
            assert!(!other.offers_permission_link());
            assert!(!other.ready());
        }
        assert!(!SttStatus::Available.offers_permission_link());
        assert!(SttStatus::Available.ready());
    }

    #[test]
    fn parakeet_readiness_prefers_the_layout_check_over_the_directory() {
        assert_eq!(
            parakeet_status(false, true, true),
            ParakeetStatus::NotInThisBuild,
            "a feature-off build never offers an install"
        );
        assert_eq!(parakeet_status(true, true, true), ParakeetStatus::Installed);
        assert_eq!(
            parakeet_status(true, false, true),
            ParakeetStatus::Partial,
            "a directory without every pinned file is a partial download"
        );
        assert_eq!(parakeet_status(true, false, false), ParakeetStatus::Missing);
        // The controls each state offers.
        assert!(ParakeetStatus::Missing.offers_download());
        assert!(ParakeetStatus::Partial.offers_download());
        assert!(ParakeetStatus::Partial.offers_delete());
        assert!(ParakeetStatus::Installed.offers_delete());
        assert!(!ParakeetStatus::Installed.offers_download());
        assert!(!ParakeetStatus::NotInThisBuild.offers_download());
        assert!(!ParakeetStatus::NotInThisBuild.offers_delete());
        assert!(ParakeetStatus::Installed.ready());
    }

    #[test]
    fn an_unprobed_readiness_draws_nothing_and_claims_nothing() {
        let probe = FastEngineReadiness::default();
        assert!(!probe.probed);
        assert!(!probe.stt.ready() && !probe.parakeet.ready());
    }

    #[test]
    fn the_coverage_list_comes_from_the_manifest_and_every_code_has_a_name() {
        let lines = parakeet_coverage_lines();
        assert_eq!(lines.len(), models::PARAKEET.languages.len());
        assert_eq!(lines.len(), 25);
        for (line, code) in lines.iter().zip(models::PARAKEET.languages) {
            let name = language_name(code);
            assert_ne!(name.as_str(), *code, "{code} must have an English name");
            assert_eq!(*line, format!("{name} ({code})"));
        }
        // The caption's count and the manifest agree, and the hover text lists
        // all of them.
        assert!(PARAKEET_COVERAGE_CAPTION.starts_with("25 languages"));
        assert_eq!(
            parakeet_coverage_hover().split(", ").count(),
            models::PARAKEET.languages.len()
        );
        // The pinned invariant, restated where the UI reads the list.
        assert!(!parakeet_coverage_hover()
            .to_lowercase()
            .contains("indonesian"));
        assert!(PARAKEET_COVERAGE_CAPTION.contains("Indonesian is always Whisper"));
        assert!(AUTO_LANGUAGE_CAPTION.contains("auto stays Whisper"));
    }

    #[test]
    fn the_deep_link_targets_the_speech_recognition_pane() {
        assert_eq!(
            STT_PERMISSION_URL,
            "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition"
        );
        // Off macOS the launcher refuses (the control is never drawn there).
        if !cfg!(target_os = "macos") {
            assert!(open_stt_permission_settings().is_err());
        }
    }
}
