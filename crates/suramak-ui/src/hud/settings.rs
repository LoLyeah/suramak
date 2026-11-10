//! Compact HUD preferences — the Settings section's persistent half (M16B).
//!
//! These live in their own `hud.toml` next to `settings.toml` instead of inside
//! the core `Settings` tree: the M16B workorder's overlap guard keeps this lane
//! out of `suramak-core/`, and the HUD is a platform-integration surface that
//! can ship independently of the core schema. The file is written atomically
//! (temp + rename, the core `Settings::save` rule) and read defensively: an
//! unknown or reserved chord degrades to the shipped default with a note, never
//! a failed start.
//!
//! The chord's grammar and the reservation tables are `suramak-taps::hotkey`
//! (shared with the registrator). This module owns only the serde shape.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use suramak_core::paths;
use suramak_taps::hotkey::{reserved_by_app, reserved_by_system, HotkeyChord, DEFAULT_HOTKEY};

/// What the result card's primary action is.
///
/// Insert is **always** gated by the macOS Accessibility permission: without it
/// Copy is the primary action regardless of this preference (the card says so
/// and offers the System Settings deep-link). The preference only decides what
/// happens automatically when the permission is granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InsertionPreference {
    /// The calm default: the result waits for Copy / Insert, no synthetic
    /// keystroke is ever posted without a click.
    #[default]
    CopyPrimary,
    /// Dictation flow: on a result, write the clipboard and press ⌘V into the
    /// app the user was in — but only when Accessibility is granted. With the
    /// permission missing the result quietly falls back to Copy-primary.
    InsertPrimary,
}

impl InsertionPreference {
    pub const ALL: [InsertionPreference; 2] = [
        InsertionPreference::CopyPrimary,
        InsertionPreference::InsertPrimary,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InsertionPreference::CopyPrimary => "Copy",
            InsertionPreference::InsertPrimary => "Insert",
        }
    }

    pub fn caption(self) -> &'static str {
        match self {
            InsertionPreference::CopyPrimary => {
                "Copy is the primary action. Insert is offered when Accessibility is granted."
            }
            InsertionPreference::InsertPrimary => {
                "With Accessibility granted, a finished dictation is copied and pasted into the app \
                 you were in. Without the permission, Copy stays primary — nothing is typed."
            }
        }
    }
}

/// The Compact HUD settings (`hud.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudSettings {
    /// The global hotkey and the floating HUD are opt-in. Off: no registration,
    /// no window, no capture entry point beyond the existing floating pill.
    pub enabled: bool,
    /// The chord in the Settings grammar (`cmd+shift+space`). Parsed by
    /// `suramak_taps::hotkey`; an unparsable value degrades to the default.
    pub chord: String,
    /// What a result does when Accessibility is granted.
    pub insertion: InsertionPreference,
}

impl Default for HudSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            chord: DEFAULT_HOTKEY.canonical(),
            insertion: InsertionPreference::default(),
        }
    }
}

impl HudSettings {
    /// The persisted chord, or the shipped default when the stored string is
    /// invalid (the caller shows [`Self::sanitize_note`]'s reason).
    pub fn chord(&self) -> HotkeyChord {
        HotkeyChord::parse(&self.chord).unwrap_or(DEFAULT_HOTKEY)
    }

    /// Stores a parsed chord in its canonical Settings spelling.
    pub fn set_chord(&mut self, chord: HotkeyChord) {
        self.chord = chord.canonical();
    }

    /// Parses `text` and checks the two reservation tables. The error is the
    /// exact message the Settings field shows (no generic "invalid").
    pub fn parse_chord(text: &str) -> Result<HotkeyChord, String> {
        let chord = HotkeyChord::parse(text).map_err(|err| err.to_string())?;
        if let Some(owner) = reserved_by_system(&chord) {
            return Err(format!(
                "{} is reserved by the system — {owner}. Choose another chord.",
                chord.label()
            ));
        }
        if let Some(owner) = reserved_by_app(&chord) {
            return Err(format!(
                "{} is used by {owner}. Choose another chord.",
                chord.label()
            ));
        }
        Ok(chord)
    }

    /// The registration-time check for a chord that passed [`Self::parse_chord`]
    /// (the platform can still refuse it as already registered).
    pub fn sanitize_note(&self) -> Option<String> {
        match Self::parse_chord(&self.chord) {
            Ok(_) => None,
            Err(note) => Some(format!(
                "the saved hotkey is unusable ({note}) — the default {} is in effect",
                DEFAULT_HOTKEY.label()
            )),
        }
    }

    /// Reads `hud.toml`, falling back to defaults on a missing file. A parse or
    /// validation problem is returned as a note (never fatal) and the whole
    /// value is reset to defaults — a half-applied hotkey configuration is
    /// worse than the shipped one.
    pub fn load_from(path: &Path) -> (Self, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(err) => {
                return (
                    Self::default(),
                    Some(format!("could not read {}: {err}", path.display())),
                );
            }
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut settings) => match settings.validate() {
                Ok(()) => (settings, None),
                Err(note) => {
                    let defaults = Self::default();
                    // Keep the enable/insertion choices; only the chord is
                    // repaired, so an older file with a bad chord still honors
                    // an explicit "enabled".
                    settings.chord = defaults.chord;
                    (
                        settings,
                        Some(format!(
                            "the saved hotkey is unusable ({note}) — the default {} is in effect",
                            DEFAULT_HOTKEY.label()
                        )),
                    )
                }
            },
            Err(err) => (
                Self::default(),
                Some(format!(
                    "{} is not a valid hud.toml ({err}) — defaults are in effect",
                    path.display()
                )),
            ),
        }
    }

    fn validate(&mut self) -> Result<(), String> {
        match Self::parse_chord(&self.chord) {
            Ok(chord) => {
                self.chord = chord.canonical();
                Ok(())
            }
            Err(note) => {
                self.chord = DEFAULT_HOTKEY.canonical();
                Err(note)
            }
        }
    }

    /// Writes `hud.toml` atomically (temp in the same directory, then rename).
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|err| err.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let temp = path.with_extension(format!("toml.tmp{}", std::process::id()));
        std::fs::write(&temp, text).map_err(|err| format!("cannot write {temp:?}: {err}"))?;
        std::fs::rename(&temp, path).map_err(|err| {
            let _ = std::fs::remove_file(&temp);
            format!("cannot replace {}: {err}", path.display())
        })
    }

    /// The user's `hud.toml` path (`<data dir>/hud.toml`).
    pub fn path() -> Result<PathBuf, String> {
        paths::data_dir()
            .map(|dir| dir.join("hud.toml"))
            .map_err(|err| err.to_string())
    }

    /// Reads the user's file (see [`Self::load_from`]).
    pub fn load() -> (Self, Option<String>) {
        match Self::path() {
            Ok(path) => Self::load_from(&path),
            Err(err) => (Self::default(), Some(err)),
        }
    }

    /// Writes the user's file (see [`Self::save_to`]).
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path()?)
    }
}

/// Whether `text` differs from the canonical spelling of its parsed chord
/// (the Settings field normalises on blur, so this drives the "normalised"
/// status line rather than silently rewriting as the user types).
pub fn needs_canonicalization(text: &str) -> bool {
    matches!(HotkeyChord::parse(text), Ok(chord) if chord.canonical() != text)
}

/// A readable list of the *avoided* system chords for the Settings caption.
/// The default is chosen to avoid every entry (the workorder asks for the
/// avoided list to be documented in the UI, not only the report).
pub const AVOIDED_SYSTEM_CHORDS: &[(&str, &str)] = &[
    ("⌘Space", "Spotlight"),
    ("⌃⌘Space", "Emoji & Symbols"),
    ("⌘⇧Q", "Log Out"),
];

/// The common app shortcuts that are shown as an advisory when chosen.
pub const AVOIDED_APP_CHORDS: &[(&str, &str)] = &[
    ("⌘⇧R", "Safari Reader / Chrome hard reload"),
    ("⌘⇧S", "Save As"),
    ("⌘⇧V", "Paste and Match Style"),
    ("⌘⇧T", "Reopen Tab"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "suramak-hud-{tag}-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn defaults_are_opt_in_and_use_the_calm_copy_flow() {
        let settings = HudSettings::default();
        assert!(!settings.enabled, "the global hotkey must be opt-in");
        assert_eq!(settings.chord, "cmd+shift+space");
        assert_eq!(settings.chord(), DEFAULT_HOTKEY);
        assert_eq!(settings.insertion, InsertionPreference::CopyPrimary);
        assert_eq!(settings.sanitize_note(), None);
        assert_eq!(settings, HudSettings::default());
    }

    #[test]
    fn toml_round_trips_every_field() {
        let settings = HudSettings {
            enabled: true,
            chord: "cmd+shift+r".to_string(),
            insertion: InsertionPreference::InsertPrimary,
        };
        let text = toml::to_string_pretty(&settings).expect("serializes");
        assert!(text.contains("enabled = true"));
        assert!(text.contains("chord = \"cmd+shift+r\""));
        assert!(text.contains("insertion = \"insert_primary\""));
        let back: HudSettings = toml::from_str(&text).expect("parses");
        assert_eq!(back, settings);
    }

    #[test]
    fn older_or_partial_files_take_the_defaults() {
        let parsed: HudSettings = toml::from_str("enabled = true\n").expect("partial file");
        assert!(parsed.enabled);
        assert_eq!(parsed.chord, DEFAULT_HOTKEY.canonical());
        assert_eq!(parsed.insertion, InsertionPreference::CopyPrimary);
    }

    #[test]
    fn a_saved_chord_is_normalised_to_its_canonical_spelling() {
        let mut settings = HudSettings::default();
        let chord = HudSettings::parse_chord("⇧⌘R").expect("parses");
        settings.set_chord(chord);
        assert_eq!(settings.chord, "cmd+shift+r");
        assert_eq!(settings.sanitize_note(), None);
    }

    #[test]
    fn system_and_app_reserved_chords_are_refused_with_the_owner_named() {
        let err = HudSettings::parse_chord("cmd+space").expect_err("Spotlight");
        assert!(err.contains("reserved by the system"), "{err}");
        assert!(err.contains("Spotlight"), "{err}");
        let err = HudSettings::parse_chord("cmd+q").expect_err("app quit");
        assert!(err.contains("Suramak's Quit"), "{err}");
    }

    #[test]
    fn an_unusable_stored_chord_degrades_to_the_default_and_keeps_the_other_choices() {
        let mut settings = HudSettings {
            enabled: true,
            chord: "cmd+space".to_string(),
            ..HudSettings::default()
        };
        let note = settings.validate().expect_err("reserved");
        assert!(note.contains("Spotlight"), "{note}");
        assert!(settings.enabled, "the enabling choice is kept");
        assert_eq!(settings.chord, DEFAULT_HOTKEY.canonical());
        assert_eq!(settings.sanitize_note(), None);
    }

    #[test]
    fn load_from_a_missing_file_is_silent_defaults() {
        let path = temp_path("missing");
        let (settings, note) = HudSettings::load_from(&path);
        assert_eq!(settings, HudSettings::default());
        assert_eq!(note, None);
    }

    #[test]
    fn save_and_load_round_trip_through_a_real_file() {
        let path = temp_path("roundtrip");
        let settings = HudSettings {
            enabled: true,
            chord: "cmd+opt+d".to_string(),
            insertion: InsertionPreference::InsertPrimary,
        };
        settings.save_to(&path).expect("saves");
        let (back, note) = HudSettings::load_from(&path);
        assert_eq!(note, None);
        assert_eq!(back, settings);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_corrupt_file_reports_and_resets() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "enabled = maybe\n").expect("writes");
        let (settings, note) = HudSettings::load_from(&path);
        assert_eq!(settings, HudSettings::default());
        assert!(note.as_deref().unwrap_or_default().contains("not a valid"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_invalid_stored_chord_is_repaired_on_load() {
        let path = temp_path("badchord");
        std::fs::write(
            &path,
            "enabled = true\nchord = \"cmd+shift\"\ninsertion = \"copy_primary\"\n",
        )
        .expect("writes");
        let (settings, note) = HudSettings::load_from(&path);
        assert!(settings.enabled);
        assert_eq!(settings.chord, DEFAULT_HOTKEY.canonical());
        assert!(note.as_deref().unwrap_or_default().contains("unusable"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_canonicalize_helper_matches_the_grammar() {
        assert!(needs_canonicalization("⇧⌘R"));
        assert!(!needs_canonicalization("cmd+shift+r"));
        assert!(!needs_canonicalization("not a chord"));
    }

    #[test]
    fn the_avoided_lists_cover_the_default_and_the_advertised_alternative() {
        let default = DEFAULT_HOTKEY.label();
        for (chord, _) in AVOIDED_SYSTEM_CHORDS {
            assert_ne!(*chord, default);
        }
        // The workorder's alternative default is the one the advisory names.
        let reader = HotkeyChord::parse("cmd+shift+r").unwrap();
        assert!(suramak_taps::hotkey::advisory_collision(&reader).is_some());
        assert!(AVOIDED_APP_CHORDS.iter().any(|(chord, _)| *chord == "⌘⇧R"));
    }
}
