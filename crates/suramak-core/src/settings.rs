use serde::{Deserialize, Serialize};

use crate::SuramakError;

/// Language selection mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LangMode {
    Auto,
    Fixed(String),
}

/// Transcription engine selection (`settings.asr.engine`, `--engine`).
///
/// `Auto` resolves to the best engine the platform, build and installed models can
/// offer: on macOS Core ML (ANE encoder, only when the encoder package is installed) →
/// Metal → CPU; on other platforms CPU. See `suramak-asr::engine::resolve_engine`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum EngineSetting {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "coreml")]
    CoreML,
    #[serde(rename = "metal")]
    Metal,
    #[serde(rename = "cpu")]
    Cpu,
}

impl EngineSetting {
    /// Lowercase identifier used by the CLI and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineSetting::Auto => "auto",
            EngineSetting::CoreML => "coreml",
            EngineSetting::Metal => "metal",
            EngineSetting::Cpu => "cpu",
        }
    }
}

/// ASR pipeline settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrSettings {
    pub model_id: String,
    pub language: LangMode,
    pub initial_prompt: Option<String>,
    pub translate_to_english: bool,
    /// Acceleration tier; missing in older settings files → `Auto`.
    #[serde(default)]
    pub engine: EngineSetting,
    /// M10A: the "Fast engine (experimental)" toggle (Parakeet TDT v3 via
    /// sherpa-onnx for covered languages). **OFF by default**; missing in older
    /// settings files → `false`, which keeps today's routing exactly.
    #[serde(default)]
    pub fast_engine: bool,
}

/// LLM polish settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolishSettings {
    /// OpenAI-compatible API base URL.
    pub base_url: String,
    pub model: String,
    /// M11: "Send the recording too (audio-aware polish)" — **OFF by
    /// default**; missing in older settings files → `false`, which keeps the
    /// text-only request exactly as it was. When on, the recording itself is
    /// uploaded to the configured endpoint alongside the transcript.
    #[serde(default)]
    pub send_audio: bool,
}

/// Storage settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    pub keep_audio: bool,
}

/// Which audio input recording uses (`settings.recording.audio_source`, M3).
///
/// Addendum A: `Default` → the system default input; `Device` → a named input
/// device (matched case-insensitively against enumeration at record time, with a
/// documented fallback to the default); `SystemAudio` → macOS-only CoreAudio
/// process taps (`suramak-taps`). Persisted as a tagged table:
///
/// ```toml
/// [recording.audio_source]
/// kind = "device"
/// name = "BlackHole 2ch"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioSource {
    /// The system default input device (the pre-M3 behavior).
    #[default]
    Default,
    /// A named input device, matched case-insensitively at record time.
    Device { name: String },
    /// macOS system audio via CoreAudio process taps (macOS 14.2+).
    SystemAudio,
}

/// Recording settings (M3; multi-source list added in M6). Missing in older
/// settings files → all defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordingSettings {
    /// Legacy single-source view (M3). Still written (it follows the first
    /// entry of `sources` on save) so older readers keep working; when
    /// `sources` is absent it *is* the effective list.
    #[serde(default)]
    pub audio_source: AudioSource,
    /// M6: the multi-select source list — every checked input is recorded and
    /// mixed into the one 16 kHz mono recording. Empty (older files) → the
    /// legacy `audio_source` is the effective list.
    #[serde(default)]
    pub sources: Vec<RecordSource>,
}

impl RecordingSettings {
    /// The effective M6 source list: `sources` when non-empty, else the legacy
    /// `audio_source` as a one-entry list.
    pub fn effective_sources(&self) -> Vec<RecordSource> {
        if self.sources.is_empty() {
            vec![RecordSource::from(&self.audio_source)]
        } else {
            self.sources.clone()
        }
    }
}

/// One entry of the M6 recording source list (multi-select checkbox list).
///
/// Persisted as an array of tagged tables:
///
/// ```toml
/// [[recording.sources]]
/// kind = "device"
/// name = "MacBook Microphone"
///
/// [[recording.sources]]
/// kind = "system_audio"
///
/// [[recording.sources]]
/// kind = "app_audio"
/// apps = ["us.zoom.xos"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordSource {
    /// The system default input device.
    Default,
    /// A named input device, matched case-insensitively at record time.
    Device { name: String },
    /// macOS system audio via CoreAudio process taps (macOS 14.2+): every
    /// process except Suramak.
    SystemAudio,
    /// macOS system audio restricted to the listed apps (M6 per-app capture).
    ///
    /// Entries are process keys: a bundle id (`us.zoom.xos`) when the process
    /// has one, else `pid:<pid>` (which only matches while that pid lives).
    /// Resolved to live pids at record time; never silently widened — a key
    /// with no running process fails the start with a clear message.
    AppAudio { apps: Vec<String> },
}

impl RecordSource {
    /// The legacy single-source view of this entry (M3's [`AudioSource`]).
    ///
    /// `AppAudio` maps to `SystemAudio` — the closest thing an older reader can
    /// express (it captures every process instead of just the listed apps);
    /// M6+ builds read the `sources` list, which is authoritative.
    pub fn legacy_audio_source(&self) -> AudioSource {
        match self {
            RecordSource::Default => AudioSource::Default,
            RecordSource::Device { name } => AudioSource::Device { name: name.clone() },
            RecordSource::SystemAudio | RecordSource::AppAudio { .. } => AudioSource::SystemAudio,
        }
    }
}

impl From<&AudioSource> for RecordSource {
    fn from(source: &AudioSource) -> Self {
        match source {
            AudioSource::Default => RecordSource::Default,
            AudioSource::Device { name } => RecordSource::Device { name: name.clone() },
            AudioSource::SystemAudio => RecordSource::SystemAudio,
        }
    }
}

/// Glass window mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GlassMode {
    Auto,
    Always,
    Never,
}

/// Appearance mode (`settings.ui.appearance`, M9a): follow the system's
/// light/dark setting (the default) or force Light/Dark. Missing in older
/// settings files → `System`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    /// Every variant, in the order the Settings row offers them.
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    /// The label shown on the segmented control.
    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }
}

/// In-app update settings (M12; macOS Sparkle). Missing in older settings
/// files → defaults (automatic checks **ON**).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatesSettings {
    /// "Automatic updates" — check for a new version in the background and
    /// offer it in place (Sparkle's standard UI asks before installing).
    /// **ON by default**; missing in older settings files → `true` (the M12
    /// policy; the `updates` feature itself stays OFF until a build enables
    /// it — the setting is inert in builds without the updater).
    #[serde(default = "default_true")]
    pub auto_check: bool,
}

/// `serde` default for the ON-by-default booleans (`bool::default` is false).
fn default_true() -> bool {
    true
}

impl Default for UpdatesSettings {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

/// UI settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettings {
    pub glass: GlassMode,
    pub font_scale: f32,
    /// M9a: System (default) | Light | Dark. Older files have no key.
    #[serde(default)]
    pub appearance: Appearance,
}

/// Top-level application settings persisted to `settings.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub asr: AsrSettings,
    pub polish: Option<PolishSettings>,
    pub storage: StorageSettings,
    pub ui: UiSettings,
    /// M3: microphone/system-audio recording options. Missing in older files → defaults.
    #[serde(default)]
    pub recording: RecordingSettings,
    /// M12: in-app update options (automatic checks ON). Missing in older
    /// files → defaults.
    #[serde(default)]
    pub updates: UpdatesSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            asr: AsrSettings {
                model_id: "turbo-q5-0".to_string(),
                language: LangMode::Auto,
                initial_prompt: None,
                translate_to_english: false,
                engine: EngineSetting::Auto,
                fast_engine: false,
            },
            polish: None,
            storage: StorageSettings { keep_audio: false },
            ui: UiSettings {
                glass: GlassMode::Auto,
                font_scale: 1.0,
                appearance: Appearance::System,
            },
            recording: RecordingSettings::default(),
            updates: UpdatesSettings::default(),
        }
    }
}

impl Settings {
    /// Loads settings from `path`, returning defaults if the file does not exist.
    pub fn load(path: &std::path::Path) -> Result<Self, SuramakError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    /// Saves settings to `path` as TOML.
    pub fn save(&self, path: &std::path::Path) -> Result<(), SuramakError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_setting_roundtrips_through_toml() {
        for setting in [
            EngineSetting::Auto,
            EngineSetting::CoreML,
            EngineSetting::Metal,
            EngineSetting::Cpu,
        ] {
            let mut settings = Settings::default();
            settings.asr.engine = setting;
            let text = toml::to_string_pretty(&settings).expect("serialize settings");
            let back: Settings = toml::from_str(&text).expect("deserialize settings");
            assert_eq!(back.asr.engine, setting);
        }
        // The serialized spelling is the lowercase id used by `--engine`.
        let mut settings = Settings::default();
        settings.asr.engine = EngineSetting::CoreML;
        let text = toml::to_string_pretty(&settings).unwrap();
        assert!(
            text.contains("engine = \"coreml\""),
            "unexpected TOML: {text}"
        );
    }

    #[test]
    fn engine_defaults_to_auto_when_absent() {
        // A settings file written before M1.5 has no `engine` key.
        let text = r#"
[asr]
model_id = "tiny"
language = "auto"
translate_to_english = false

[storage]
keep_audio = false

[ui]
glass = "auto"
font_scale = 1.0
"#;
        let settings: Settings = toml::from_str(text).expect("legacy settings must load");
        assert_eq!(settings.asr.engine, EngineSetting::Auto);
        assert_eq!(EngineSetting::default(), EngineSetting::Auto);
        assert_eq!(EngineSetting::CoreML.as_str(), "coreml");
    }

    // ---- M10A: the fast-engine toggle ---------------------------------------

    #[test]
    fn fast_engine_roundtrips_and_defaults_to_off() {
        for fast in [false, true] {
            let mut settings = Settings::default();
            settings.asr.fast_engine = fast;
            let text = toml::to_string_pretty(&settings).expect("serialize settings");
            assert!(
                text.contains(&format!("fast_engine = {fast}")),
                "unexpected TOML: {text}"
            );
            let back: Settings = toml::from_str(&text).expect("deserialize settings");
            assert_eq!(back.asr.fast_engine, fast);
        }
        // The built-in default is OFF (existing builds behave identically).
        assert!(!Settings::default().asr.fast_engine);
        // A settings file written before M10A has no `fast_engine` key.
        let text = r#"
[asr]
model_id = "tiny"
language = "auto"
translate_to_english = false
engine = "coreml"

[storage]
keep_audio = false

[ui]
glass = "auto"
font_scale = 1.0
"#;
        let settings: Settings = toml::from_str(text).expect("M9-era settings must load");
        assert!(!settings.asr.fast_engine, "missing key → OFF");
        assert_eq!(settings.asr.engine, EngineSetting::CoreML);
    }

    // ---- M11: the audio-aware polish toggle ---------------------------------

    #[test]
    fn polish_send_audio_roundtrips_and_defaults_to_off() {
        for send in [false, true] {
            let settings = Settings {
                polish: Some(PolishSettings {
                    base_url: "http://localhost:11434/v1".to_string(),
                    model: "m".to_string(),
                    send_audio: send,
                }),
                ..Settings::default()
            };
            let text = toml::to_string_pretty(&settings).expect("serialize settings");
            assert!(
                text.contains(&format!("send_audio = {send}")),
                "unexpected TOML: {text}"
            );
            let back: Settings = toml::from_str(&text).expect("deserialize settings");
            assert_eq!(back.polish.unwrap().send_audio, send);
        }
        // A settings file written before M11 has no `send_audio` key → OFF.
        let text = r#"
[asr]
model_id = "tiny"
language = "auto"
translate_to_english = false

[polish]
base_url = "http://localhost:11434/v1"
model = "llama3"

[storage]
keep_audio = false

[ui]
glass = "auto"
font_scale = 1.0
"#;
        let settings: Settings = toml::from_str(text).expect("M10-era settings must load");
        let polish = settings.polish.expect("polish section");
        assert!(!polish.send_audio, "missing key → OFF");
        assert_eq!(polish.model, "llama3");
    }

    #[test]
    fn settings_load_returns_defaults_when_file_missing() {
        let dir = std::env::temp_dir().join(format!("suramak-settings-{}", std::process::id()));
        let path = dir.join("does-not-exist.toml");
        let settings = Settings::load(&path).expect("missing file → defaults");
        assert_eq!(settings.asr.engine, EngineSetting::Auto);
        assert_eq!(settings.asr.model_id, "turbo-q5-0");
        assert_eq!(settings.recording.audio_source, AudioSource::Default);
        assert_eq!(settings.ui.appearance, Appearance::System);
    }

    // ---- M9a: appearance ----------------------------------------------------

    #[test]
    fn appearance_roundtrips_through_toml() {
        for appearance in Appearance::ALL {
            let mut settings = Settings::default();
            settings.ui.appearance = appearance;
            let text = toml::to_string_pretty(&settings).expect("serialize settings");
            let back: Settings = toml::from_str(&text).expect("deserialize settings");
            assert_eq!(back.ui.appearance, appearance, "toml: {text}");
        }
        // The serialized spelling is the lowercase name the Settings row shows.
        let mut settings = Settings::default();
        settings.ui.appearance = Appearance::Dark;
        let text = toml::to_string_pretty(&settings).unwrap();
        assert!(
            text.contains("appearance = \"dark\""),
            "unexpected TOML: {text}"
        );
        assert_eq!(Appearance::default(), Appearance::System);
        assert_eq!(Appearance::Light.label(), "Light");
    }

    #[test]
    fn appearance_defaults_to_system_when_absent() {
        // A settings file written before M9a has no `appearance` key.
        let text = r#"
[asr]
model_id = "tiny"
language = "auto"
translate_to_english = false

[storage]
keep_audio = false

[ui]
glass = "auto"
font_scale = 1.0
"#;
        let settings: Settings = toml::from_str(text).expect("legacy settings must load");
        assert_eq!(settings.ui.appearance, Appearance::System);
    }

    #[test]
    fn audio_source_roundtrips_through_toml() {
        for source in [
            AudioSource::Default,
            AudioSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            AudioSource::SystemAudio,
        ] {
            let mut settings = Settings::default();
            settings.recording.audio_source = source.clone();
            let text = toml::to_string_pretty(&settings).expect("serialize settings");
            let back: Settings = toml::from_str(&text).expect("deserialize settings");
            assert_eq!(back.recording.audio_source, source);
        }
        // The serialized spelling is the documented tagged table.
        let mut settings = Settings::default();
        settings.recording.audio_source = AudioSource::Device {
            name: "BlackHole 2ch".to_string(),
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        assert!(
            text.contains("kind = \"device\"") && text.contains("name = \"BlackHole 2ch\""),
            "unexpected TOML: {text}"
        );
    }

    #[test]
    fn recording_section_defaults_when_absent() {
        // A settings file written before M3 has no `[recording]` section.
        let text = r#"
[asr]
model_id = "tiny"
language = "auto"
translate_to_english = false

[storage]
keep_audio = true

[ui]
glass = "auto"
font_scale = 1.0
"#;
        let settings: Settings = toml::from_str(text).expect("legacy settings must load");
        assert_eq!(settings.recording.audio_source, AudioSource::Default);
        assert!(settings.storage.keep_audio);
        // M6: no source list either — the legacy single source is effective.
        assert!(settings.recording.sources.is_empty());
        assert_eq!(
            settings.recording.effective_sources(),
            vec![RecordSource::Default]
        );
    }

    // ---- M6: the multi-select source list -----------------------------------

    #[test]
    fn record_sources_roundtrip_through_toml() {
        for source in [
            RecordSource::Default,
            RecordSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            RecordSource::SystemAudio,
            RecordSource::AppAudio {
                apps: vec!["us.zoom.xos".to_string(), "pid:4242".to_string()],
            },
        ] {
            let mut settings = Settings::default();
            settings.recording.sources = vec![source.clone()];
            let text = toml::to_string_pretty(&settings).expect("serialize settings");
            let back: Settings = toml::from_str(&text).expect("deserialize settings");
            assert_eq!(back.recording.sources, vec![source.clone()], "toml: {text}");
            assert_eq!(back.recording.effective_sources(), vec![source]);
        }
    }

    #[test]
    fn record_sources_serialize_as_the_documented_tagged_tables() {
        let mut settings = Settings::default();
        settings.recording.sources = vec![
            RecordSource::Device {
                name: "MacBook Microphone".to_string(),
            },
            RecordSource::SystemAudio,
            RecordSource::AppAudio {
                apps: vec!["us.zoom.xos".to_string()],
            },
        ];
        let text = toml::to_string_pretty(&settings).unwrap();
        assert!(
            text.contains("[[recording.sources]]"),
            "the list is an array of tables: {text}"
        );
        assert!(
            text.contains("kind = \"device\""),
            "unexpected TOML: {text}"
        );
        assert!(
            text.contains("kind = \"system_audio\""),
            "unexpected: {text}"
        );
        assert!(text.contains("kind = \"app_audio\""), "unexpected: {text}");
        assert!(
            text.contains("apps = [\"us.zoom.xos\"]"),
            "unexpected: {text}"
        );
    }

    #[test]
    fn legacy_audio_source_is_the_fallback_until_sources_is_set() {
        // An M3-era file: `[recording]` with only `audio_source`.
        let text = r#"
[asr]
model_id = "tiny"
language = "auto"
translate_to_english = false

[storage]
keep_audio = false

[ui]
glass = "auto"
font_scale = 1.0

[recording.audio_source]
kind = "device"
name = "BlackHole 2ch"
"#;
        let settings: Settings = toml::from_str(text).expect("M3 settings must load");
        assert!(settings.recording.sources.is_empty());
        assert_eq!(
            settings.recording.effective_sources(),
            vec![RecordSource::Device {
                name: "BlackHole 2ch".to_string()
            }]
        );

        // Once a list is present it wins over the legacy field.
        let mut settings = settings;
        settings.recording.sources = vec![
            RecordSource::SystemAudio,
            RecordSource::Device {
                name: "MacBook Microphone".to_string(),
            },
        ];
        assert_eq!(settings.recording.effective_sources().len(), 2);
    }

    #[test]
    fn legacy_audio_source_maps_every_record_source() {
        assert_eq!(
            RecordSource::Default.legacy_audio_source(),
            AudioSource::Default
        );
        assert_eq!(
            RecordSource::Device {
                name: "X".to_string()
            }
            .legacy_audio_source(),
            AudioSource::Device {
                name: "X".to_string()
            }
        );
        assert_eq!(
            RecordSource::SystemAudio.legacy_audio_source(),
            AudioSource::SystemAudio
        );
        // AppAudio degrades to full system audio for older readers (documented).
        assert_eq!(
            RecordSource::AppAudio {
                apps: vec!["us.zoom.xos".to_string()]
            }
            .legacy_audio_source(),
            AudioSource::SystemAudio
        );
        // The `From` direction is the exact inverse for the three M3 variants.
        for source in [
            AudioSource::Default,
            AudioSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            AudioSource::SystemAudio,
        ] {
            assert_eq!(RecordSource::from(&source).legacy_audio_source(), source);
        }
    }

    // ---- M12: the updates section -------------------------------------------

    #[test]
    fn updates_auto_check_roundtrips_and_defaults_to_on() {
        for auto in [true, false] {
            let mut settings = Settings::default();
            settings.updates.auto_check = auto;
            let text = toml::to_string_pretty(&settings).expect("serialize settings");
            assert!(
                text.contains(&format!("auto_check = {auto}")),
                "unexpected TOML: {text}"
            );
            let back: Settings = toml::from_str(&text).expect("deserialize settings");
            assert_eq!(back.updates.auto_check, auto);
        }
        // The built-in default is ON (M12's first-run policy).
        assert!(Settings::default().updates.auto_check);
        assert!(UpdatesSettings::default().auto_check);
    }

    #[test]
    fn updates_defaults_to_on_when_absent() {
        // A settings file written before M12 has no `[updates]` section.
        let text = r#"
[asr]
model_id = "tiny"
language = "auto"
translate_to_english = false

[storage]
keep_audio = false

[ui]
glass = "auto"
font_scale = 1.0
"#;
        let settings: Settings = toml::from_str(text).expect("M11-era settings must load");
        assert!(
            settings.updates.auto_check,
            "missing section → automatic checks ON"
        );
    }
}
