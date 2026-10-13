//! Settings-screen logic — **egui-free** (unit-tested in this file): the form ↔
//! core [`Settings`] round-trip, Reset-to-defaults, and the language selector.
//!
//! The view (`app::settings`) renders the form and requests
//! [`Effect::SaveSettings`](super::Effect::SaveSettings); `crate::runtime`
//! applies it (toml write + keychain on macOS/Windows).

use suramak_core::settings::{AudioSource, EngineSetting, LangMode, PolishSettings, Settings};

/// Language selector values offered in the UI (PLAN-NOTES §4 — Indonesian needs a
/// forced language; auto-detect translated it to English).
pub const LANGUAGES: [&str; 3] = ["auto", "en", "id"];

/// Placeholder base URL shown (not persisted until typed).
pub const DEFAULT_POLISH_BASE_URL: &str = "https://api.openai.com/v1";

/// Whether the OS keychain is available in this build (macOS/Windows).
pub fn api_key_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// `settings.asr.language` → selector string.
pub fn lang_selector(language: &LangMode) -> String {
    match language {
        LangMode::Auto => "auto".to_string(),
        LangMode::Fixed(code) => code.clone(),
    }
}

/// Selector string (`auto`, `en`, `id`, …) → [`LangMode`].
pub fn lang_mode(selector: &str) -> LangMode {
    let selector = selector.trim();
    if selector.eq_ignore_ascii_case("auto") {
        LangMode::Auto
    } else {
        LangMode::Fixed(selector.to_string())
    }
}

/// Stable dropdown key for an [`AudioSource`] (`default`, `device:<name>`, `system`).
pub fn audio_source_key(source: &AudioSource) -> String {
    match source {
        AudioSource::Default => "default".to_string(),
        AudioSource::Device { name } => format!("device:{name}"),
        AudioSource::SystemAudio => "system".to_string(),
    }
}

/// Inverse of [`audio_source_key`] (unknown keys → [`AudioSource::Default`]).
pub fn audio_source_from_key(key: &str) -> AudioSource {
    if key == "system" {
        AudioSource::SystemAudio
    } else if let Some(name) = key.strip_prefix("device:") {
        if name.is_empty() {
            AudioSource::Default
        } else {
            AudioSource::Device {
                name: name.to_string(),
            }
        }
    } else {
        AudioSource::Default
    }
}

/// Editable settings-form state (a copy of what the screen shows).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsForm {
    pub engine: EngineSetting,
    pub model_id: String,
    /// Language selector (`auto`/`en`/`id`, or a custom code from an older file).
    pub language: String,
    pub keep_audio: bool,
    pub audio_source: AudioSource,
    pub polish_base_url: String,
    pub polish_model: String,
    /// Transient API-key input; **never** written to settings.toml (macOS/Windows
    /// keychain only).
    pub api_key: String,
    /// Unsaved-changes marker for the "Save" affordance.
    pub dirty: bool,
    /// Last save result (ok note / error), shown next to the buttons.
    pub status: Option<Result<String, String>>,
}

impl SettingsForm {
    /// Builds the form from the persisted settings.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            engine: settings.asr.engine,
            model_id: settings.asr.model_id.clone(),
            language: lang_selector(&settings.asr.language),
            keep_audio: settings.storage.keep_audio,
            audio_source: settings.recording.audio_source.clone(),
            polish_base_url: settings
                .polish
                .as_ref()
                .map(|polish| polish.base_url.clone())
                .unwrap_or_default(),
            polish_model: settings
                .polish
                .as_ref()
                .map(|polish| polish.model.clone())
                .unwrap_or_default(),
            api_key: String::new(),
            dirty: false,
            status: None,
        }
    }

    /// Writes the form's values into `settings` (non-UI fields are left alone).
    pub fn apply(&self, settings: &mut Settings) {
        settings.asr.engine = self.engine;
        settings.asr.model_id = self.model_id.trim().to_string();
        settings.asr.language = lang_mode(&self.language);
        settings.storage.keep_audio = self.keep_audio;
        settings.recording.audio_source = self.audio_source.clone();
        let base = self.polish_base_url.trim();
        let model = self.polish_model.trim();
        settings.polish = if base.is_empty() && model.is_empty() {
            None
        } else {
            Some(PolishSettings {
                base_url: base.to_string(),
                model: model.to_string(),
            })
        };
    }

    /// "Reset to defaults": the form goes back to [`Settings::default`] (the user
    /// still presses Save to persist — Reset alone never touches the file).
    pub fn reset(&mut self) {
        let mut form = Self::from_settings(&Settings::default());
        form.dirty = true;
        form.status = Some(Ok("defaults restored — press Save to persist".to_string()));
        *self = form;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::settings::GlassMode;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-form-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("settings.toml")
    }

    #[test]
    fn form_roundtrips_every_settings_value() {
        let mut settings = Settings::default();
        settings.asr.engine = EngineSetting::Metal;
        settings.asr.model_id = "small".to_string();
        settings.asr.language = LangMode::Fixed("id".to_string());
        settings.storage.keep_audio = true;
        settings.recording.audio_source = AudioSource::Device {
            name: "BlackHole 2ch".to_string(),
        };
        settings.polish = Some(PolishSettings {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "llama3".to_string(),
        });
        settings.ui.glass = GlassMode::Never;

        let form = SettingsForm::from_settings(&settings);
        assert_eq!(form.language, "id");
        assert_eq!(form.polish_base_url, "http://localhost:11434/v1");

        let mut target = Settings::default();
        form.apply(&mut target);
        assert_eq!(target.asr.engine, EngineSetting::Metal);
        assert_eq!(target.asr.model_id, "small");
        assert_eq!(target.asr.language, LangMode::Fixed("id".to_string()));
        assert!(target.storage.keep_audio);
        assert_eq!(
            target.recording.audio_source,
            AudioSource::Device {
                name: "BlackHole 2ch".to_string()
            }
        );
        assert_eq!(target.polish.as_ref().unwrap().model, "llama3");
        // Non-form fields survive `apply` untouched (the target kept its own).
        assert_eq!(target.ui.glass, GlassMode::Auto);
        assert_eq!(target.asr.initial_prompt, settings.asr.initial_prompt);
        assert!(!target.asr.translate_to_english);
    }

    #[test]
    fn form_defaults_match_core_defaults() {
        let form = SettingsForm::from_settings(&Settings::default());
        assert_eq!(form.engine, EngineSetting::Auto);
        assert_eq!(form.model_id, "turbo-q5-0");
        assert_eq!(form.language, "auto");
        assert!(!form.keep_audio);
        assert_eq!(form.audio_source, AudioSource::Default);
        assert!(form.polish_base_url.is_empty() && form.polish_model.is_empty());
        assert!(form.api_key.is_empty(), "the key field always starts empty");
        assert!(!form.dirty);
    }

    #[test]
    fn keep_audio_toggle_is_observable_in_the_settings_file() {
        // PLAN's acceptance greps the file after a toggle — this is that check in
        // code. The file deliberately stays on disk (fixed name, not pid-based) so
        // the M3 acceptance can grep it too:
        //   grep keep_audio <temp>/suramak-acceptance-keep-audio/settings.toml
        let dir = std::env::temp_dir().join("suramak-acceptance-keep-audio");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("settings.toml");

        let mut settings = Settings::default();
        let mut form = SettingsForm::from_settings(&settings);
        form.keep_audio = true;
        form.apply(&mut settings);
        settings.save(&path).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("keep_audio = true"), "unexpected: {text}");
        assert!(Settings::load(&path).unwrap().storage.keep_audio);

        // …and back to false (the observable value follows the toggle both ways).
        form.keep_audio = false;
        form.apply(&mut settings);
        settings.save(&path).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("keep_audio = false"), "unexpected: {text}");
        assert!(!Settings::load(&path).unwrap().storage.keep_audio);

        // Leave the *true* value on disk as the acceptance artifact.
        form.keep_audio = true;
        form.apply(&mut settings);
        settings.save(&path).expect("save");
        println!("keep_audio acceptance file: {}", path.display());
    }

    #[test]
    fn reset_restores_defaults_but_only_save_persists() {
        let path = temp_path("reset");
        let mut settings = Settings::default();
        settings.asr.model_id = "turbo".to_string();
        settings.storage.keep_audio = true;
        settings.save(&path).expect("save");

        let mut form = SettingsForm::from_settings(&settings);
        form.reset();
        assert_eq!(form.model_id, "turbo-q5-0");
        assert!(!form.keep_audio);
        assert!(form.dirty, "reset marks the form dirty so Save lights up");
        assert!(form.status.as_ref().unwrap().is_ok());
        // The file is untouched until Save (runtime applies the form).
        let on_disk = Settings::load(&path).expect("reload");
        assert_eq!(on_disk.asr.model_id, "turbo");
        assert!(on_disk.storage.keep_audio);

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn empty_polish_fields_clear_the_section() {
        let mut settings = Settings {
            polish: Some(PolishSettings {
                base_url: "http://x/v1".to_string(),
                model: "m".to_string(),
            }),
            ..Settings::default()
        };
        let mut form = SettingsForm::from_settings(&settings);
        form.polish_base_url = "   ".to_string();
        form.polish_model = String::new();
        form.apply(&mut settings);
        assert!(settings.polish.is_none());

        // A half-filled section persists as-is (trimmed).
        form.polish_base_url = "  http://localhost:8080/v1  ".to_string();
        form.apply(&mut settings);
        assert_eq!(
            settings.polish.as_ref().unwrap().base_url,
            "http://localhost:8080/v1"
        );
    }

    #[test]
    fn language_selector_maps_both_ways() {
        assert_eq!(lang_mode("auto"), LangMode::Auto);
        assert_eq!(lang_mode("AUTO"), LangMode::Auto);
        assert_eq!(lang_mode(" en "), LangMode::Fixed("en".to_string()));
        assert_eq!(lang_mode("id"), LangMode::Fixed("id".to_string()));
        assert_eq!(lang_selector(&LangMode::Auto), "auto");
        assert_eq!(lang_selector(&LangMode::Fixed("id".to_string())), "id");
        // A custom code from an older settings file round-trips even when it is
        // not one of the offered options.
        let form = SettingsForm::from_settings(&Settings {
            asr: suramak_core::settings::AsrSettings {
                language: LangMode::Fixed("fr".to_string()),
                ..Settings::default().asr
            },
            ..Settings::default()
        });
        assert_eq!(form.language, "fr");
        assert_eq!(LANGUAGES, ["auto", "en", "id"]);
    }

    #[test]
    fn api_key_support_matches_the_platform() {
        assert_eq!(
            api_key_supported(),
            cfg!(any(target_os = "macos", target_os = "windows"))
        );
    }

    #[test]
    fn audio_source_dropdown_keys_roundtrip() {
        for source in [
            AudioSource::Default,
            AudioSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            AudioSource::SystemAudio,
        ] {
            let key = audio_source_key(&source);
            assert_eq!(audio_source_from_key(&key), source, "key {key}");
        }
        assert_eq!(audio_source_key(&AudioSource::Default), "default");
        assert_eq!(
            audio_source_key(&AudioSource::Device {
                name: "BlackHole 2ch".to_string()
            }),
            "device:BlackHole 2ch"
        );
        // Unknown / degenerate keys never explode and never lose the safe default.
        assert_eq!(audio_source_from_key(""), AudioSource::Default);
        assert_eq!(audio_source_from_key("nonsense"), AudioSource::Default);
        assert_eq!(audio_source_from_key("device:"), AudioSource::Default);
    }
}
