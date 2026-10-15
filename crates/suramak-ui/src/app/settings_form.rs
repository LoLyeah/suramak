//! Settings-screen logic — **egui-free** (unit-tested in this file): the form ↔
//! core [`Settings`] round-trip, Reset-to-defaults, and the language selector.
//!
//! The view (`app::settings`) renders the form and requests
//! [`Effect::SaveSettings`](super::Effect::SaveSettings); `crate::runtime`
//! applies it (toml write + keychain on macOS/Windows).

use suramak_core::settings::{
    Appearance, EngineSetting, LangMode, PolishSettings, RecordSource, Settings,
};

/// Language selector values offered in the UI (PLAN-NOTES §4 — Indonesian needs a
/// forced language; auto-detect translated it to English).
pub const LANGUAGES: [&str; 3] = ["auto", "en", "id"];

/// Label of the free-text entry in the language selector row (M4.5).
pub const CUSTOM_LANGUAGE_LABEL: &str = "Custom…";

/// Loose validation of a custom whisper language code (M4.5): trimmed; 2–8
/// chars of ASCII letters/digits/`-` (whisper codes are lowercase
/// ISO-639-1-ish — deliberately not stricter); returned lowercased. `Err`
/// carries the inline hint shown under the field.
pub fn normalize_language_code(raw: &str) -> Result<String, String> {
    let code = raw.trim();
    if code.is_empty() {
        return Err("enter a language code (2–8 letters, digits or '-')".to_string());
    }
    let acceptable = (2..=8).contains(&code.len())
        && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !acceptable {
        return Err(format!(
            "'{code}' is not a usable language code — use 2–8 letters, digits or '-' (e.g. es, jv)"
        ));
    }
    Ok(code.to_ascii_lowercase())
}

/// The one-shot "applied" confirm's total envelope (M9c): the pulse is exactly
/// 0 after this long — a one-shot, never a loop.
pub const APPLIED_PULSE_MS: u64 = 700;
/// The confirm pulse's rise (the rest of the envelope is the calm fall).
pub const APPLIED_PULSE_RISE_MS: u64 = 120;

/// The "applied" confirm pulse's opacity for an age since the save landed:
/// quick rise, calm fall, exactly 0 by [`APPLIED_PULSE_MS`] (and for anything
/// older). Drawn around the Save control; under Reduce Motion the view skips
/// it entirely (the status line carries the confirmation).
pub fn confirm_pulse(age: std::time::Duration) -> f32 {
    let t = age.as_millis() as f32;
    let rise = APPLIED_PULSE_RISE_MS as f32;
    let total = APPLIED_PULSE_MS as f32;
    if t >= total {
        return 0.0;
    }
    if t <= rise {
        return (t / rise).clamp(0.0, 1.0);
    }
    1.0 - ((t - rise) / (total - rise)).clamp(0.0, 1.0)
}

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

/// Canonical, deterministic order of an M6 source list: default mic, named
/// devices (case-insensitive by name), system audio, app audio. Keeps the
/// settings round-trip and the record view's rows stable regardless of the
/// order the user ticked the boxes.
pub fn order_sources(sources: &mut [RecordSource]) {
    sources.sort_by_key(|source| match source {
        RecordSource::Default => (0u8, String::new()),
        RecordSource::Device { name } => (1, name.to_lowercase()),
        RecordSource::SystemAudio => (2, String::new()),
        RecordSource::AppAudio { .. } => (3, String::new()),
    });
}

/// Editable settings-form state (a copy of what the screen shows).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsForm {
    pub engine: EngineSetting,
    /// M10A: "Fast engine (experimental)" — persisted as `settings.asr.fast_engine`,
    /// **OFF by default**. On: covered languages route to Parakeet when installed;
    /// Indonesian and everything else stay Whisper.
    pub fast_engine: bool,
    pub model_id: String,
    /// Language selector (`auto`/`en`/`id`, or a custom code from an older file).
    pub language: String,
    /// True while the "Custom…" entry is selected (reveals the free-text field,
    /// M4.5; UI-only — the persisted value stays in [`SettingsForm::language`]).
    pub language_custom: bool,
    /// Free-text draft for the custom language code; validated on save.
    pub language_draft: String,
    pub keep_audio: bool,
    /// M9a: System | Light | Dark — persisted as `settings.ui.appearance` and
    /// applied live by the shell when the form saves.
    pub appearance: Appearance,
    /// M6: the checked recording sources (canonical order). This is what the
    /// record pipeline reads at start; ≥1 entry is required to save
    /// ([`SettingsForm::sources_problem`]).
    pub sources: Vec<RecordSource>,
    pub polish_base_url: String,
    pub polish_model: String,
    /// M11: "Send the recording too (audio-aware polish)" — persisted as
    /// `settings.polish.send_audio`, **OFF by default**. On: the recording
    /// itself is uploaded to the configured endpoint alongside the transcript.
    pub polish_send_audio: bool,
    /// Transient API-key input; **never** written to settings.toml (macOS/Windows
    /// keychain only).
    pub api_key: String,
    /// Unsaved-changes marker for the "Save" affordance.
    pub dirty: bool,
    /// Last save result (ok note / error), shown next to the buttons.
    pub status: Option<Result<String, String>>,
    /// M9c: when the last successful save landed — the view plays the one-shot
    /// "applied" confirm pulse around Save, then clears this.
    pub applied_at: Option<std::time::Instant>,
}

impl SettingsForm {
    /// Builds the form from the persisted settings.
    pub fn from_settings(settings: &Settings) -> Self {
        let language = lang_selector(&settings.asr.language);
        // A code that is not one of the offered presets came from an older
        // file (or an earlier Custom… entry): show Custom… with the code in
        // the field so it is editable in place (M4.5).
        let language_custom = !LANGUAGES.contains(&language.as_str());
        let language_draft = if language_custom {
            language.clone()
        } else {
            String::new()
        };
        // M6: the effective source list (legacy `audio_source` when the file
        // predates the list), in canonical order.
        let mut sources = settings.recording.effective_sources();
        order_sources(&mut sources);
        Self {
            engine: settings.asr.engine,
            fast_engine: settings.asr.fast_engine,
            model_id: settings.asr.model_id.clone(),
            language,
            language_custom,
            language_draft,
            keep_audio: settings.storage.keep_audio,
            appearance: settings.ui.appearance,
            sources,
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
            polish_send_audio: settings
                .polish
                .as_ref()
                .is_some_and(|polish| polish.send_audio),
            api_key: String::new(),
            dirty: false,
            status: None,
            applied_at: None,
        }
    }

    // ---- M6: the multi-select source list -----------------------------------

    /// Whether a source entry of this shape is checked.
    pub fn is_checked(&self, source: &RecordSource) -> bool {
        self.sources.iter().any(|checked| checked == source)
    }

    /// Ticks/untickets one entry and restores the canonical order.
    pub fn set_checked(&mut self, source: RecordSource, checked: bool) {
        self.sources.retain(|entry| entry != &source);
        if checked {
            self.sources.push(source);
        }
        order_sources(&mut self.sources);
    }

    /// Ticks/untickets the "Only an app…" entry. Ticking starts with no apps
    /// (the picker is empty until one is chosen — [`SettingsForm::sources_problem`]
    /// refuses to save that state); unticking discards the app list with the
    /// entry.
    pub fn set_app_audio_checked(&mut self, checked: bool) {
        self.sources
            .retain(|entry| !matches!(entry, RecordSource::AppAudio { .. }));
        if checked {
            self.sources
                .push(RecordSource::AppAudio { apps: Vec::new() });
        }
        order_sources(&mut self.sources);
    }

    /// The app keys of the "Only an app…" entry (`None` when it is not ticked).
    pub fn app_audio_apps(&self) -> Option<&[String]> {
        self.sources.iter().find_map(|source| match source {
            RecordSource::AppAudio { apps } => Some(apps.as_slice()),
            _ => None,
        })
    }

    /// Ticks/untickets one app key inside the "Only an app…" entry (keys are
    /// [`AudioProcessInfo::settings_key`](suramak_asr::capture::AudioProcessInfo::settings_key)
    /// values). A no-op when the entry is not ticked.
    pub fn set_app_checked(&mut self, key: &str, checked: bool) {
        for source in &mut self.sources {
            if let RecordSource::AppAudio { apps } = source {
                apps.retain(|app| app != key);
                if checked {
                    apps.push(key.to_string());
                }
                apps.sort();
            }
        }
    }

    /// Validation of the source list as an inline hint (M6: **never zero
    /// sources**, and the app entry needs at least one app). `None` = valid.
    pub fn sources_problem(&self) -> Option<String> {
        if self.sources.is_empty() {
            return Some("select at least one input source".to_string());
        }
        if self
            .sources
            .iter()
            .any(|source| matches!(source, RecordSource::AppAudio { apps } if apps.is_empty()))
        {
            return Some("pick at least one app for the \"Only an app…\" entry".to_string());
        }
        None
    }

    /// The language selector value [`SettingsForm::apply`] will persist: the
    /// chosen preset as-is, or the validated (trimmed, lowercased) custom code.
    /// `Err` carries the inline hint shown under the field (M4.5).
    pub fn validated_language(&self) -> Result<String, String> {
        if !self.language_custom {
            return Ok(self.language.trim().to_string());
        }
        normalize_language_code(&self.language_draft)
    }

    /// The caption shown under the Custom… field: the friendly "your own risk"
    /// note when the draft is usable, or the inline validation hint (error)
    /// when it is not. Returns `(text, error)` (M4.5).
    pub fn language_hint(&self) -> (String, bool) {
        match self.validated_language() {
            Ok(code) => (
                format!(
                    "custom: {code} — not officially supported; the model may produce poor results"
                ),
                false,
            ),
            Err(_) if self.language_draft.trim().is_empty() => (
                "type a code — 2–8 letters, digits or '-' (e.g. es, jv); not officially \
                 supported — the model may produce poor results"
                    .to_string(),
                false,
            ),
            Err(hint) => (hint, true),
        }
    }

    /// Writes the form's values into `settings` (non-UI fields are left alone).
    ///
    /// An invalid custom language draft refuses to persist: the previous
    /// `settings.asr.language` stays (the view shows the inline hint). The same
    /// rule guards the M6 source list: an empty list (or an app entry without
    /// apps) refuses, the previous list stays.
    pub fn apply(&self, settings: &mut Settings) {
        settings.asr.engine = self.engine;
        settings.asr.fast_engine = self.fast_engine;
        settings.asr.model_id = self.model_id.trim().to_string();
        if let Ok(code) = self.validated_language() {
            settings.asr.language = lang_mode(&code);
        }
        settings.storage.keep_audio = self.keep_audio;
        settings.ui.appearance = self.appearance;
        if self.sources_problem().is_none() {
            settings.recording.sources = self.sources.clone();
            // The legacy single-source field follows the first entry, so older
            // readers keep a sane view of the file (documented; the list is
            // authoritative in M6+ builds).
            if let Some(first) = self.sources.first() {
                settings.recording.audio_source = first.legacy_audio_source();
            }
        }
        let base = self.polish_base_url.trim();
        let model = self.polish_model.trim();
        settings.polish = if base.is_empty() && model.is_empty() {
            None
        } else {
            Some(PolishSettings {
                base_url: base.to_string(),
                model: model.to_string(),
                send_audio: self.polish_send_audio,
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
    use suramak_core::settings::{AudioSource, GlassMode};

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
        settings.recording.sources = vec![
            RecordSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            RecordSource::SystemAudio,
        ];
        settings.polish = Some(PolishSettings {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "llama3".to_string(),
            send_audio: false,
        });
        settings.ui.glass = GlassMode::Never;

        let form = SettingsForm::from_settings(&settings);
        assert_eq!(form.language, "id");
        assert_eq!(form.polish_base_url, "http://localhost:11434/v1");
        assert_eq!(
            form.sources,
            vec![
                RecordSource::Device {
                    name: "BlackHole 2ch".to_string()
                },
                RecordSource::SystemAudio
            ]
        );

        let mut target = Settings::default();
        form.apply(&mut target);
        assert_eq!(target.asr.engine, EngineSetting::Metal);
        assert_eq!(target.asr.model_id, "small");
        assert_eq!(target.asr.language, LangMode::Fixed("id".to_string()));
        assert!(target.storage.keep_audio);
        assert_eq!(
            target.recording.sources,
            vec![
                RecordSource::Device {
                    name: "BlackHole 2ch".to_string()
                },
                RecordSource::SystemAudio
            ]
        );
        // The legacy single-source field follows the list's first entry.
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
        assert!(!form.fast_engine, "the fast engine starts OFF");
        assert_eq!(form.model_id, "turbo-q5-0");
        assert_eq!(form.language, "auto");
        assert!(!form.keep_audio);
        assert_eq!(form.appearance, Appearance::System);
        assert_eq!(form.sources, vec![RecordSource::Default]);
        assert!(form.polish_base_url.is_empty() && form.polish_model.is_empty());
        assert!(!form.polish_send_audio, "audio-aware polish starts OFF");
        assert!(form.api_key.is_empty(), "the key field always starts empty");
        assert!(!form.dirty);
    }

    // ---- M10A: the fast-engine toggle ---------------------------------------

    #[test]
    fn fast_engine_toggle_roundtrips_form_settings_file_and_back() {
        let mut settings = Settings::default();
        let mut form = SettingsForm::from_settings(&settings);
        assert!(!form.fast_engine);

        form.fast_engine = true;
        form.apply(&mut settings);
        assert!(settings.asr.fast_engine);

        let path = temp_path("fast-engine");
        settings.save(&path).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("fast_engine = true"),
            "unexpected TOML: {text}"
        );
        let reloaded = Settings::load(&path).expect("reload");
        assert!(reloaded.asr.fast_engine);
        let form2 = SettingsForm::from_settings(&reloaded);
        assert!(form2.fast_engine, "the form follows the file");

        // …and back off (the observable value follows the toggle both ways).
        form.fast_engine = false;
        form.apply(&mut settings);
        settings.save(&path).expect("save");
        assert!(!Settings::load(&path).unwrap().asr.fast_engine);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    // ---- M11: the audio-aware polish toggle ---------------------------------

    #[test]
    fn polish_send_audio_toggle_roundtrips_form_settings_file_and_back() {
        let mut settings = Settings {
            polish: Some(PolishSettings {
                base_url: "http://localhost:11434/v1".to_string(),
                model: "llama3".to_string(),
                send_audio: false,
            }),
            ..Settings::default()
        };
        let mut form = SettingsForm::from_settings(&settings);
        assert!(!form.polish_send_audio, "the toggle starts OFF");

        form.polish_send_audio = true;
        form.apply(&mut settings);
        assert!(settings.polish.as_ref().unwrap().send_audio);

        let path = temp_path("polish-send-audio");
        settings.save(&path).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("send_audio = true"),
            "unexpected TOML: {text}"
        );
        let reloaded = Settings::load(&path).expect("reload");
        assert!(reloaded.polish.as_ref().unwrap().send_audio);
        let form2 = SettingsForm::from_settings(&reloaded);
        assert!(form2.polish_send_audio, "the form follows the file");

        // …and back off (the observable value follows the toggle both ways).
        form.polish_send_audio = false;
        form.apply(&mut settings);
        settings.save(&path).expect("save");
        assert!(!Settings::load(&path).unwrap().polish.unwrap().send_audio);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    // ---- M9a: appearance (live apply) ---------------------------------------

    #[test]
    fn appearance_roundtrips_form_settings_file_and_back() {
        let mut settings = Settings::default();
        let mut form = SettingsForm::from_settings(&settings);
        assert_eq!(form.appearance, Appearance::System);
        for appearance in Appearance::ALL {
            form.appearance = appearance;
            form.apply(&mut settings);
            assert_eq!(settings.ui.appearance, appearance);

            let path = temp_path("appearance");
            settings.save(&path).expect("save");
            let reloaded = Settings::load(&path).expect("reload");
            assert_eq!(reloaded.ui.appearance, appearance, "toml round-trip");
            let form2 = SettingsForm::from_settings(&reloaded);
            assert_eq!(form2.appearance, appearance, "the form follows the file");
            std::fs::remove_dir_all(path.parent().unwrap()).ok();
        }
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
                send_audio: false,
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
        // not one of the offered options — and shows as Custom… with the code
        // in the field (M4.5).
        let form = SettingsForm::from_settings(&Settings {
            asr: suramak_core::settings::AsrSettings {
                language: LangMode::Fixed("fr".to_string()),
                ..Settings::default().asr
            },
            ..Settings::default()
        });
        assert_eq!(form.language, "fr");
        assert!(form.language_custom, "a non-preset code shows as Custom…");
        assert_eq!(form.language_draft, "fr");
        assert_eq!(LANGUAGES, ["auto", "en", "id"]);
        assert_eq!(CUSTOM_LANGUAGE_LABEL, "Custom…");
    }

    // ---- M4.5: custom language codes ----------------------------------------

    #[test]
    fn custom_language_code_is_accepted_lowered_and_persisted() {
        let mut settings = Settings::default();
        let mut form = SettingsForm::from_settings(&settings);
        form.language_custom = true;
        form.language_draft = "  ES  ".to_string();
        assert_eq!(form.validated_language().unwrap(), "es");
        form.apply(&mut settings);
        assert_eq!(settings.asr.language, LangMode::Fixed("es".to_string()));

        // Dashes and mixed case are fine (trimmed, lowered) — whisper codes
        // are ISO-639-1-ish and the validation is deliberately loose.
        form.language_draft = "PT-br".to_string();
        form.apply(&mut settings);
        assert_eq!(settings.asr.language, LangMode::Fixed("pt-br".to_string()));

        // The stored value survives a settings.toml round-trip (acceptance §5).
        let path = temp_path("custom-lang");
        settings.save(&path).expect("save");
        let reloaded = Settings::load(&path).expect("reload");
        assert_eq!(reloaded.asr.language, LangMode::Fixed("pt-br".to_string()));
        let form = SettingsForm::from_settings(&reloaded);
        assert_eq!(form.language, "pt-br");
        assert!(form.language_custom);
        assert_eq!(form.language_draft, "pt-br");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn custom_code_validation_is_loose_but_bounded() {
        assert_eq!(normalize_language_code(" es ").unwrap(), "es");
        assert_eq!(normalize_language_code("AB").unwrap(), "ab");
        assert_eq!(normalize_language_code("jv").unwrap(), "jv");
        assert_eq!(normalize_language_code("pt-BR").unwrap(), "pt-br");
        assert_eq!(normalize_language_code("zh-8").unwrap(), "zh-8");
        assert_eq!(normalize_language_code("abcdefgh").unwrap(), "abcdefgh");
        // Too short / too long / empty / not letters-digits-dash → refused.
        for bad in ["", "a", "abcdefghi", "e!", "es es", "é", "e_s"] {
            assert!(
                normalize_language_code(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn invalid_custom_language_is_refused_and_keeps_the_previous_value() {
        let mut settings = Settings::default();
        settings.asr.language = LangMode::Fixed("en".to_string());
        let mut form = SettingsForm::from_settings(&settings);
        form.language_custom = true;

        for bad in ["e", "abcdefghi", "e!", "es es", "é", "e_s"] {
            form.language_draft = bad.to_string();
            assert!(
                form.validated_language().is_err(),
                "{bad:?} must be refused"
            );
            assert!(form.language_hint().1, "{bad:?} must show an error hint");
            form.apply(&mut settings);
            assert_eq!(
                settings.asr.language,
                LangMode::Fixed("en".to_string()),
                "the previous value must be kept for {bad:?}"
            );
        }

        // An empty draft is a neutral prompt (not an error) but still refuses.
        form.language_draft = String::new();
        assert!(!form.language_hint().1);
        form.apply(&mut settings);
        assert_eq!(settings.asr.language, LangMode::Fixed("en".to_string()));

        // Fixing the code persists it on the next apply.
        form.language_draft = "jv".to_string();
        assert_eq!(
            form.language_hint().0,
            "custom: jv — not officially supported; the model may produce poor results"
        );
        form.apply(&mut settings);
        assert_eq!(settings.asr.language, LangMode::Fixed("jv".to_string()));
    }

    #[test]
    fn language_presets_still_work_and_win_over_a_stale_draft() {
        let mut settings = Settings::default();
        let mut form = SettingsForm::from_settings(&settings);
        // The user typed a custom draft, then went back to a preset.
        form.language_draft = "es".to_string();
        for preset in LANGUAGES {
            form.language = preset.to_string();
            form.language_custom = false;
            assert_eq!(form.validated_language().unwrap(), preset);
            form.apply(&mut settings);
            assert_eq!(lang_selector(&settings.asr.language), preset);
        }
        assert_eq!(
            settings.asr.language,
            LangMode::Fixed("id".to_string()),
            "the last preset wins"
        );
    }

    #[test]
    fn api_key_support_matches_the_platform() {
        assert_eq!(
            api_key_supported(),
            cfg!(any(target_os = "macos", target_os = "windows"))
        );
    }

    #[test]
    fn the_applied_confirm_pulse_rises_falls_and_ends() {
        use std::time::Duration;
        // 0 at the instant, full at the end of the rise…
        assert_eq!(confirm_pulse(Duration::ZERO), 0.0);
        assert_eq!(
            confirm_pulse(Duration::from_millis(APPLIED_PULSE_RISE_MS)),
            1.0
        );
        // …falling monotonically through the hold…
        let a = confirm_pulse(Duration::from_millis(300));
        let b = confirm_pulse(Duration::from_millis(500));
        assert!(a > b && b > 0.0, "the fall is monotonic: {a} -> {b}");
        // …and exactly 0 once the envelope elapsed (a one-shot, not a loop).
        assert_eq!(confirm_pulse(Duration::from_millis(APPLIED_PULSE_MS)), 0.0);
        assert_eq!(confirm_pulse(Duration::from_secs(30)), 0.0);
        // A fresh form has no armed pulse; a successful save arms one.
        let mut form = SettingsForm::from_settings(&Settings::default());
        assert!(form.applied_at.is_none());
        form.applied_at = Some(std::time::Instant::now());
        assert!(form.applied_at.is_some());
    }

    // ---- M6: the multi-select source list -----------------------------------

    #[test]
    fn multi_select_roundtrips_through_settings_in_canonical_order() {
        let mut form = SettingsForm::from_settings(&Settings::default());
        assert_eq!(form.sources, vec![RecordSource::Default]);

        // Toggle in a deliberately non-canonical order: system, then a device,
        // then a second device, and untick the default.
        form.set_checked(RecordSource::SystemAudio, true);
        form.set_checked(
            RecordSource::Device {
                name: "USB Mic".to_string(),
            },
            true,
        );
        form.set_checked(
            RecordSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            true,
        );
        form.set_checked(RecordSource::Default, false);
        assert_eq!(
            form.sources,
            vec![
                RecordSource::Device {
                    name: "BlackHole 2ch".to_string()
                },
                RecordSource::Device {
                    name: "USB Mic".to_string()
                },
                RecordSource::SystemAudio,
            ],
            "canonical order: devices (by name), then system audio"
        );
        assert!(form.sources_problem().is_none());

        // Save → toml → reload → the same list (ordered, stable).
        let path = temp_path("m6-sources");
        let mut settings = Settings::default();
        form.apply(&mut settings);
        settings.save(&path).expect("save");
        let reloaded = Settings::load(&path).expect("reload");
        assert_eq!(reloaded.recording.sources, form.sources);
        let form2 = SettingsForm::from_settings(&reloaded);
        assert_eq!(form2.sources, form.sources);
        // The legacy field follows the first entry.
        assert_eq!(
            reloaded.recording.audio_source,
            AudioSource::Device {
                name: "BlackHole 2ch".to_string()
            }
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn never_zero_sources_is_refused_on_save() {
        let mut settings = Settings::default();
        settings.recording.sources = vec![RecordSource::SystemAudio];
        let mut form = SettingsForm::from_settings(&settings);

        // Unticking the last source is allowed in the form (the user is mid-
        // decision), but it must not save.
        form.set_checked(RecordSource::SystemAudio, false);
        assert!(form.sources.is_empty());
        assert_eq!(
            form.sources_problem().as_deref(),
            Some("select at least one input source")
        );
        form.apply(&mut settings);
        assert_eq!(
            settings.recording.sources,
            vec![RecordSource::SystemAudio],
            "an empty list never persists — the previous list stays"
        );

        // Ticking anything again clears the hint and saves (the unticked
        // system-audio entry stays unticked — the list is what the user sees).
        form.set_checked(RecordSource::Default, true);
        assert!(form.sources_problem().is_none());
        form.apply(&mut settings);
        assert_eq!(settings.recording.sources, vec![RecordSource::Default]);
    }

    #[test]
    fn app_audio_entry_plumbing() {
        let mut form = SettingsForm::from_settings(&Settings::default());
        assert_eq!(form.app_audio_apps(), None);

        // Tick "Only an app…": the entry appears with no apps yet — invalid
        // until the picker ticks one.
        form.set_app_audio_checked(true);
        assert_eq!(form.app_audio_apps(), Some([].as_slice()));
        assert_eq!(
            form.sources_problem().as_deref(),
            Some("pick at least one app for the \"Only an app…\" entry")
        );
        let mut settings = Settings::default();
        form.apply(&mut settings);
        assert!(
            settings.recording.sources.is_empty(),
            "the app entry without apps never persists — the stored list stays untouched"
        );
        assert_eq!(
            settings.recording.effective_sources(),
            vec![RecordSource::Default],
            "the legacy default remains effective until a list is saved"
        );

        // Tick two apps (keys from the process picker); they are stored sorted.
        form.set_app_checked("us.zoom.xos", true);
        form.set_app_checked("com.google.Chrome", true);
        assert_eq!(
            form.app_audio_apps(),
            Some(["com.google.Chrome".to_string(), "us.zoom.xos".to_string()].as_slice())
        );
        assert!(form.sources_problem().is_none());
        form.apply(&mut settings);
        assert_eq!(
            settings.recording.sources,
            vec![
                RecordSource::Default,
                RecordSource::AppAudio {
                    apps: vec!["com.google.Chrome".to_string(), "us.zoom.xos".to_string()]
                }
            ]
        );
        // The legacy field degrades AppAudio to full system audio (documented).
        assert_eq!(settings.recording.audio_source, AudioSource::Default);

        // Untick one app; untick the entry (its app list goes with it — the
        // picker starts fresh when the entry is ticked again).
        form.set_app_checked("us.zoom.xos", false);
        assert_eq!(
            form.app_audio_apps(),
            Some(["com.google.Chrome".to_string()].as_slice())
        );
        form.set_app_audio_checked(false);
        assert_eq!(form.app_audio_apps(), None);
        form.set_app_audio_checked(true);
        assert_eq!(
            form.app_audio_apps(),
            Some([].as_slice()),
            "a re-ticked entry starts with no apps (the validation prompts)"
        );
    }

    #[test]
    fn sources_roundtrip_through_the_settings_file() {
        // The exact settings.toml shape the acceptance greps for.
        let dir = std::env::temp_dir().join("suramak-acceptance-m6-sources");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("settings.toml");
        let mut settings = Settings::default();
        settings.recording.sources = vec![
            RecordSource::Default,
            RecordSource::Device {
                name: "BlackHole 2ch".to_string(),
            },
            RecordSource::SystemAudio,
            RecordSource::AppAudio {
                apps: vec!["us.zoom.xos".to_string()],
            },
        ];
        settings.save(&path).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[recording.sources]]"), "unexpected: {text}");
        assert!(text.contains("kind = \"app_audio\""), "unexpected: {text}");
        assert_eq!(
            Settings::load(&path).unwrap().recording.sources,
            settings.recording.sources
        );
        println!("m6 sources acceptance file: {}", path.display());
    }
}
