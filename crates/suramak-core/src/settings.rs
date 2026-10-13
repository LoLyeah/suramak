use serde::{Deserialize, Serialize};

use crate::SuramakError;

/// Language selection mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LangMode {
    Auto,
    Fixed(String),
}

/// ASR pipeline settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrSettings {
    pub model_id: String,
    pub language: LangMode,
    pub initial_prompt: Option<String>,
    pub translate_to_english: bool,
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
