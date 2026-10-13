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
}

/// LLM polish settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolishSettings {
    /// OpenAI-compatible API base URL.
    pub base_url: String,
    pub model: String,
}

/// Storage settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    pub keep_audio: bool,
}

/// Glass window mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GlassMode {
    Auto,
    Always,
    Never,
}

/// UI settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettings {
    pub glass: GlassMode,
    pub font_scale: f32,
}

/// Top-level application settings persisted to `settings.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub asr: AsrSettings,
    pub polish: Option<PolishSettings>,
    pub storage: StorageSettings,
    pub ui: UiSettings,
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
            },
            polish: None,
            storage: StorageSettings { keep_audio: false },
            ui: UiSettings {
                glass: GlassMode::Auto,
                font_scale: 1.0,
            },
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

    #[test]
    fn settings_load_returns_defaults_when_file_missing() {
        let dir = std::env::temp_dir().join(format!("suramak-settings-{}", std::process::id()));
        let path = dir.join("does-not-exist.toml");
        let settings = Settings::load(&path).expect("missing file → defaults");
        assert_eq!(settings.asr.engine, EngineSetting::Auto);
        assert_eq!(settings.asr.model_id, "turbo-q5-0");
    }
}
