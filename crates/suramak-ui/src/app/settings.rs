//! Settings route (M3; M6 multi-source list; M9a appearance row; M9e scroll).
//!
//! Engine, default model/language, **appearance (System | Light | Dark)**,
//! keep_audio + the checkbox list of recording inputs (devices, system audio,
//! "Only an app…" with its running-audio-process picker), the recordings
//! folder, and the polish section (base URL / model / keychain key).
//!
//! All form rules live in `app::settings_form` (egui-free, unit-tested); this
//! view edits that form and asks the runtime to persist it
//! (`Effect::SaveSettings`). M9a: painted controls, the one navigation pattern
//! (Home, top-right) and the tokens; the appearance is applied live when the
//! form saves (no restart).
//!
//! M9e (P2-3): the form lives in a `ScrollArea` and lays out **in flow** —
//! at the app's own 900×600 minimum the form is taller than the pane, and
//! Save/Reset must stay reachable (scroll to them) instead of dead-ending.
//! Flow layout also lets the long captions wrap instead of clipping mid-word.
//! The visual composition is unchanged: the 150 pt label column, 26 pt rows,
//! serif section headings, Save/Reset at the form's end.

use egui::{pos2, FontFamily, FontId, Rect};

use suramak_asr::capture::AudioProcessInfo;
use suramak_core::models as core_models;
use suramak_core::settings::{Appearance, EngineSetting, RecordSource};

use crate::app::controls::{self, Tier};
use crate::motion::Motion;
use crate::theme::Tokens;

use super::settings_form::{self, SettingsForm, LANGUAGES};
use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect};

/// System-audio entries exist on macOS builds only (hidden elsewhere; Addendum A).
const SYSTEM_AUDIO_OFFERED: bool = cfg!(target_os = "macos");

const ROW_H: f32 = 26.0;
const LABEL_W: f32 = 150.0;
/// Bottom reserve inside the scroll content so the form's actions clear the
/// floating record pill (24 pt inset + 52 pt pill height) at 900×600.
const FORM_BOTTOM_RESERVE: f32 = 76.0;

pub fn ui(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let rect = ui.max_rect();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, "Settings", tokens);
            pane_caption(
                ui,
                "Everything is stored locally (settings.toml); nothing here leaves this machine except model downloads and polish calls",
                tokens,
                false,
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.add_space(PANE_PAD_X - 8.0);
            if controls::nav_home(ui, tokens, motion, "settings-nav").clicked() {
                effects.extend(state.back_home());
            }
        });
    });

    // M9e (P2-3): the form scrolls. Everything below the header — sections,
    // rows and the Save/Reset actions — lives inside the scroll area, so at
    // 900×600 the form is reachable end to end (the actions are its tail).
    let scroll_rect = Rect::from_min_max(
        pos2(left, rect.top() + 96.0),
        pos2(right, rect.bottom() - 6.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(scroll_rect), |ui| {
        egui::ScrollArea::vertical()
            // The salt keeps this form's scroll offset its own: both this and
            // the models table are children of the same central panel, so
            // without it egui hands them the same id and the offset leaks
            // across views (Models would open mid-table after scrolling here).
            .id_salt("settings-form")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                form(ui, tokens, motion, state, effects);
            });
    });
}

/// The scrollable form body (flow layout).
fn form(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    // The form sections borrow only `state.settings_form`; snapshots are
    // copied out first so the borrows stay disjoint.
    let devices = state.audio_devices.clone();
    let device_error = state.audio_devices_error.clone();
    let processes = state.audio_processes.clone();
    let processes_error = state.audio_processes_error.clone();
    let capture_available = state.capture_available;

    let mut dirty = false;
    {
        let form = &mut state.settings_form;
        dirty |= transcription_section(ui, tokens, form);
        dirty |= appearance_section(ui, tokens, motion, form);
        dirty |= audio_source_section(
            ui,
            tokens,
            motion,
            form,
            &devices,
            device_error.as_deref(),
            &processes,
            processes_error.as_deref(),
            capture_available,
            effects,
        );
        dirty |= polish_section(ui, tokens, form);
    }

    // ---- actions ------------------------------------------------------------
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let save = controls::button(
            ui,
            tokens,
            motion,
            "settings-save",
            "Save",
            Tier::Primary,
            true,
        );
        // M9c "applied" confirm: a subtle one-shot pulse around Save after a
        // successful save (quick rise, calm fall, ends — then the timestamp
        // is cleared). Reduce Motion skips it: the status line confirms.
        let now = std::time::Instant::now();
        if let Some(applied_at) = state.settings_form.applied_at {
            let age = now.saturating_duration_since(applied_at);
            if age.as_millis() as u64 >= settings_form::APPLIED_PULSE_MS {
                state.settings_form.applied_at = None;
            } else if !state.reduce_motion {
                let pulse = settings_form::confirm_pulse(age);
                let ring = save
                    .rect
                    .expand(controls::FOCUS_RING_EXPAND + (1.0 - pulse) * 3.0);
                ui.painter().rect_stroke(
                    ring,
                    egui::CornerRadius::same(tokens.radius.control.saturating_add(2)),
                    egui::Stroke::new(tokens.stroke.focus, tokens.accent.gamma_multiply(pulse)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        if save.clicked() {
            effects.extend(state.save_settings());
        }
        ui.add_space(6.0);
        let reset = controls::button(
            ui,
            tokens,
            motion,
            "settings-reset",
            "Reset to defaults",
            Tier::Tertiary,
            true,
        );
        if reset.clicked() {
            state.reset_settings_form();
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let message = state
            .settings_form
            .status
            .as_ref()
            .map(|status| match status {
                Ok(note) => (note.as_str(), false),
                Err(err) => (err.as_str(), true),
            });
        if message.is_some() {
            super::views::status_caption(ui, tokens, motion, "settings-status", message);
        } else if dirty {
            pane_caption(ui, "unsaved changes — press Save", tokens, false);
        }
    });
    // Breathing room at the end of the scroll content: the floating record
    // pill floats over the window's bottom ~76 pt (24 pt inset + its 52 pt
    // height), so the form's actions must clear it at the 900×600 minimum —
    // Reset sat under the pill without this reserve.
    ui.add_space(FORM_BOTTOM_RESERVE);
}

/// Engine / default model / default language / keep_audio / recordings folder.
/// Returns whether the user changed something.
fn transcription_section(ui: &mut egui::Ui, tokens: &Tokens, form: &mut SettingsForm) -> bool {
    let mut dirty = false;
    section(ui, tokens, "Transcription");

    // Engine.
    row(ui, tokens, "Engine", |ui| {
        for (setting, name) in [
            (EngineSetting::Auto, "auto"),
            (EngineSetting::CoreML, "coreml"),
            (EngineSetting::Metal, "metal"),
            (EngineSetting::Cpu, "cpu"),
        ] {
            if ui
                .selectable_value(&mut form.engine, setting, name)
                .changed()
            {
                dirty = true;
            }
        }
    });

    // M10A: the fast engine (experimental) — one toggle, OFF by default.
    // M10B: the same toggle now drives the engine order per platform.
    row(ui, tokens, "Fast engine", |ui| {
        if ui
            .checkbox(
                &mut form.fast_engine,
                "(experimental) — fast engines for covered languages",
            )
            .changed()
        {
            dirty = true;
        }
    });
    row_caption(
        ui,
        tokens,
        "Off by default. When on, covered languages use the fast engines in order — \
         on macOS 26+: SpeechTranscriber → Parakeet → Whisper; elsewhere: \
         Parakeet → Whisper (covered languages only; Indonesian always Whisper). \
         Parakeet needs its model installed (`suramak-cli models download parakeet`); \
         SpeechTranscriber's models are managed by macOS. The engine line names what \
         actually ran.",
        false,
    );

    // Default model. M9c copy sweep: the combobox shows the human display
    // names; the raw id stays the value that persists.
    row(ui, tokens, "Default model", |ui| {
        let current = form.model_id.clone();
        let mut chosen = current.clone();
        egui::ComboBox::from_id_salt("suramak-default-model")
            .selected_text(super::model_manager::display_name(&current))
            .show_ui(ui, |ui| {
                for (value, display) in model_options(&current) {
                    ui.selectable_value(&mut chosen, value.clone(), display);
                }
            });
        if chosen != current {
            form.model_id = chosen;
            dirty = true;
        }
        if !core_models::MODELS.iter().any(|m| m.id == current) && current != "auto" {
            pane_caption(
                ui,
                "not a manifest id — treated as a file path at transcribe time",
                tokens,
                false,
            );
        }
    });

    // Default language: the presets plus a free-text "Custom…" entry (M4.5).
    // The value that persists is whatever `settings_form` validates; an invalid
    // draft is refused there and only the inline hint shows it here.
    let mut custom_clicked = false;
    row(ui, tokens, "Default language", |ui| {
        for value in LANGUAGES {
            let selected = !form.language_custom && form.language == value;
            let clicked = ui.selectable_label(selected, value).clicked();
            if clicked && !selected {
                form.language = value.to_string();
                form.language_custom = false;
                dirty = true;
            }
        }
        if ui
            .selectable_label(form.language_custom, settings_form::CUSTOM_LANGUAGE_LABEL)
            .clicked()
        {
            custom_clicked = true;
        }
        if form.language_custom {
            let response = ui.add_sized(
                [110.0, 22.0],
                egui::TextEdit::singleline(&mut form.language_draft)
                    .hint_text("es, jv…")
                    .text_color(tokens.ink),
            );
            if response.changed() {
                dirty = true;
            }
        }
    });
    if custom_clicked {
        // Seed the field from the last non-preset code, so switching to
        // Custom… never starts blank when there is something to edit.
        if form.language_draft.trim().is_empty() && !LANGUAGES.contains(&form.language.as_str()) {
            form.language_draft = form.language.clone();
        }
        if !form.language_custom {
            dirty = true;
        }
        form.language_custom = true;
    }
    if form.language_custom {
        let (hint, error) = form.language_hint();
        row_caption(ui, tokens, &hint, error);
    }

    // keep_audio.
    row(ui, tokens, "Keep recordings", |ui| {
        if ui
            .checkbox(&mut form.keep_audio, "keep audio after transcription")
            .changed()
        {
            dirty = true;
        }
    });

    // Recordings folder display.
    row(ui, tokens, "Recordings folder", |ui| {
        let recordings = suramak_core::paths::recordings_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|_| "unavailable".to_string());
        let text = if form.keep_audio {
            recordings
        } else {
            "off — recordings go to the temp dir and are deleted after the transcript is saved"
                .to_string()
        };
        ui.label(
            egui::RichText::new(text)
                .family(FontFamily::Monospace)
                .size(11.5)
                .color(tokens.ink_soft),
        );
    });
    ui.add_space(8.0);
    dirty
}

/// M9a appearance: System (follows the OS live) | Light | Dark, as a segmented
/// control. Applied when the form saves — no restart.
fn appearance_section(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    form: &mut SettingsForm,
) -> bool {
    let mut dirty = false;
    section(ui, tokens, "Appearance");

    row(ui, tokens, "Theme", |ui| {
        let mut appearance = form.appearance;
        let options: Vec<(Appearance, &str)> = Appearance::ALL
            .iter()
            .map(|option| (*option, option.label()))
            .collect();
        if controls::segmented(
            ui,
            tokens,
            motion,
            "settings-appearance",
            &mut appearance,
            &options,
        ) {
            form.appearance = appearance;
            dirty = true;
        }
    });

    row_caption(
        ui,
        tokens,
        "System follows the macOS light/dark setting live; Light or Dark forces this app's appearance. Applied when you press Save.",
        false,
    );
    ui.add_space(4.0);
    dirty
}

/// The M6 checkbox list of recording inputs: default mic, enumerated devices,
/// system audio (macOS), and "Only an app…" with its running-audio-process
/// picker. Everything ticked is recorded together and mixed into one file;
/// ≥1 source is required to save (the inline hint shows the refusal).
#[allow(clippy::too_many_arguments)]
fn audio_source_section(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    form: &mut SettingsForm,
    devices: &[String],
    device_error: Option<&str>,
    processes: &[AudioProcessInfo],
    processes_error: Option<&str>,
    capture_available: bool,
    effects: &mut Vec<Effect>,
) -> bool {
    let mut dirty = false;
    section(ui, tokens, "Audio sources");

    row(ui, tokens, "Inputs", |ui| {
        pane_caption(
            ui,
            "pick one or more — everything ticked is recorded together into one file",
            tokens,
            false,
        );
    });

    // Default microphone.
    dirty |= source_checkbox(
        ui,
        capture_available,
        "Default microphone",
        form.is_checked(&RecordSource::Default),
        |checked| form.set_checked(RecordSource::Default, checked),
    );

    // Enumerated input devices.
    for name in devices {
        let source = RecordSource::Device { name: name.clone() };
        let checked = form.is_checked(&source);
        dirty |= source_checkbox(ui, capture_available, name, checked, |checked| {
            form.set_checked(RecordSource::Device { name: name.clone() }, checked)
        });
    }

    // System audio (macOS builds only — hidden elsewhere, Addendum A).
    if SYSTEM_AUDIO_OFFERED {
        dirty |= source_checkbox(
            ui,
            capture_available,
            "System audio (all processes except Suramak)",
            form.is_checked(&RecordSource::SystemAudio),
            |checked| form.set_checked(RecordSource::SystemAudio, checked),
        );

        // "Only an app…" — the per-app entry + its process picker (M6).
        dirty |= source_checkbox(
            ui,
            capture_available,
            "Only an app…",
            form.app_audio_apps().is_some(),
            |checked| form.set_app_audio_checked(checked),
        );
        if let Some(apps) = form.app_audio_apps() {
            let apps = apps.to_vec();
            dirty |= app_picker(
                ui,
                tokens,
                motion,
                form,
                processes,
                processes_error,
                capture_available,
                &apps,
                effects,
            );
        }
    }

    // Validation hint (never zero sources; the app entry needs ≥1 app).
    if let Some(problem) = form.sources_problem() {
        row_caption(ui, tokens, &problem, true);
    }

    // Enumeration notes (the existing copy, capture-less builds included).
    let note = if let Some(error) = device_error {
        Some((format!("input devices: {error}"), true))
    } else if devices.is_empty() {
        Some((
            "no input devices enumerated — the default entry still opens the system default input"
                .to_string(),
            false,
        ))
    } else {
        Some((
            format!(
                "{} enumerated (virtual loopback devices like BlackHole appear here)",
                super::count_noun(devices.len(), "input device")
            ),
            false,
        ))
    };
    if let Some((text, error)) = note {
        row_caption(ui, tokens, &text, error);
    }
    if !capture_available {
        row_caption(
            ui,
            tokens,
            "recording is not compiled into this build (feature `capture`) — the input list is read-only",
            true,
        );
    }
    ui.add_space(6.0);
    dirty
}

/// One checkbox row of the source list (field column). Returns whether the
/// value changed.
fn source_checkbox(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    checked: bool,
    on_change: impl FnOnce(bool),
) -> bool {
    let mut value = checked;
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W);
        ui.add_enabled(enabled, egui::Checkbox::new(&mut value, label));
    });
    if value != checked {
        on_change(value);
        true
    } else {
        false
    }
}

/// The running-audio-process picker under "Only an app…" (M6): one checkbox per
/// process, keyed by its settings key (bundle id, else `pid:<pid>`).
#[allow(clippy::too_many_arguments)]
fn app_picker(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    form: &mut SettingsForm,
    processes: &[AudioProcessInfo],
    processes_error: Option<&str>,
    capture_available: bool,
    apps: &[String],
    effects: &mut Vec<Effect>,
) -> bool {
    let mut dirty = false;
    let indent = LABEL_W + 24.0;
    ui.horizontal(|ui| {
        ui.add_space(indent);
        pane_caption(
            ui,
            "the tap captures only the ticked apps (a stereo mixdown of exactly those processes)",
            tokens,
            false,
        );
        let refresh = controls::button(
            ui,
            tokens,
            motion,
            "settings-refresh-apps",
            "Refresh",
            Tier::Tertiary,
            true,
        );
        if refresh.clicked() {
            effects.push(Effect::RefreshAudioProcesses);
        }
    });

    if let Some(error) = processes_error {
        indented_caption(
            ui,
            tokens,
            indent,
            &format!("running audio processes: {error}"),
            true,
        );
    } else if processes.is_empty() {
        indented_caption(
            ui,
            tokens,
            indent,
            "no app is producing audio right now — start playback, then Refresh",
            false,
        );
    }

    for process in processes {
        let key = process.settings_key();
        let checked = apps.iter().any(|app| app == &key);
        let mut value = checked;
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.add_enabled(
                capture_available,
                egui::Checkbox::new(&mut value, process_label(process)),
            );
        });
        if value != checked {
            form.set_app_checked(&key, value);
            dirty = true;
        }
    }
    ui.add_space(4.0);
    dirty
}

/// A caption indented to the app-picker's column (wraps in flow).
fn indented_caption(ui: &mut egui::Ui, tokens: &Tokens, indent: f32, text: &str, error: bool) {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        pane_caption(ui, text, tokens, error);
    });
}

/// Display label for one process row (`us.zoom.xos (pid 700) — playing`).
fn process_label(process: &AudioProcessInfo) -> String {
    let name = if process.bundle_id.is_empty() {
        format!("pid {}", process.pid)
    } else {
        format!("{} (pid {})", process.bundle_id, process.pid)
    };
    if process.is_running_output {
        format!("{name} — playing")
    } else {
        name
    }
}

/// Polish section: base URL / model / keychain key (macOS/Windows only).
fn polish_section(ui: &mut egui::Ui, tokens: &Tokens, form: &mut SettingsForm) -> bool {
    let mut dirty = false;
    section(ui, tokens, "Polish (LLM tidy-up)");

    row(ui, tokens, "Base URL", |ui| {
        let response = ui.add_sized(
            [360.0, 22.0],
            egui::TextEdit::singleline(&mut form.polish_base_url)
                .hint_text(settings_form::DEFAULT_POLISH_BASE_URL)
                .text_color(tokens.ink),
        );
        if response.changed() {
            dirty = true;
        }
    });

    row(ui, tokens, "Model", |ui| {
        let response = ui.add_sized(
            [360.0, 22.0],
            egui::TextEdit::singleline(&mut form.polish_model)
                .hint_text("e.g. gpt-4o-mini")
                .text_color(tokens.ink),
        );
        if response.changed() {
            dirty = true;
        }
    });

    // M11: the audio-aware toggle — one control, OFF by default, with the
    // privacy copy (the recording itself uploads when this is on).
    row(ui, tokens, "Send audio", |ui| {
        if ui
            .checkbox(
                &mut form.polish_send_audio,
                "Send the recording too (audio-aware polish)",
            )
            .changed()
        {
            dirty = true;
        }
    });
    row_caption(
        ui,
        tokens,
        "Off by default. When on, the recording itself is uploaded to the endpoint you \
         configured, alongside the transcript, so corrections resolve names, homophones and \
         jargon against what was actually said. Needs an audio-capable model (e.g. the \
         gpt-audio class, Gemini, Voxtral); an endpoint that rejects the audio falls back to \
         text-only with a note.",
        false,
    );

    row(ui, tokens, "API key", |ui| {
        if settings_form::api_key_supported() {
            let response = ui.add_sized(
                [360.0, 22.0],
                egui::TextEdit::singleline(&mut form.api_key)
                    .password(true)
                    .hint_text("stored in the OS keychain — never in settings.toml")
                    .text_color(tokens.ink),
            );
            if response.changed() {
                dirty = true;
            }
        } else {
            pane_caption(
                ui,
                "hidden on this platform — the key lives in the OS keychain on macOS/Windows; \
                 here set SURAMAK_OPENAI_API_KEY in the environment",
                tokens,
                false,
            );
        }
    });
    ui.add_space(12.0);
    dirty
}

/// Model dropdown options as `(value, display label)` pairs: `auto` + every
/// manifest id (human-labelled) + a custom value if the settings carry one.
fn model_options(current: &str) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> =
        vec![("auto".to_string(), "Auto (best installed)".to_string())];
    options.extend(
        core_models::MODELS
            .iter()
            .map(|m| (m.id.to_string(), super::model_manager::display_name(m.id))),
    );
    if !options.iter().any(|(value, _)| value == current) {
        options.insert(1, (current.to_string(), current.to_string()));
    }
    options
}

/// Section heading (serif, flow).
fn section(ui: &mut egui::Ui, tokens: &Tokens, title: &str) {
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(title)
            .font(FontId::new(17.0, crate::theme::serif_family()))
            .color(tokens.ink),
    );
    ui.add_space(8.0);
}

/// One form row: the fixed label column, then the field column. Flow layout —
/// the row grows with its content (M9e: captions wrap instead of clipping).
fn row(ui: &mut egui::Ui, tokens: &Tokens, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [LABEL_W, ROW_H],
            egui::Label::new(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(tokens.ink_soft)
                    .family(FontFamily::Proportional),
            ),
        );
        body(ui);
    });
}

/// A caption in the field column (under its row), wrapping in flow.
fn row_caption(ui: &mut egui::Ui, tokens: &Tokens, text: &str, error: bool) {
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W);
        // M9e (P2-3): wrap inside the field column, measured against the
        // visible clip — not the layout max-rect, which inside the scroll area
        // can be looser than the pane — so a caption can never run past the
        // pane edge at the 900×600 minimum.
        let width = (ui.clip_rect().right() - ui.cursor().left()).max(120.0);
        ui.scope(|ui| {
            ui.set_max_width(width);
            pane_caption(ui, text, tokens, error);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Route;
    use crate::theme::AppTheme;

    /// M9e (P2-3) geometry: the form's content is taller than the 900×600
    /// minimum's pane, and every section + the Save/Reset actions are drawn —
    /// the scroll area is what makes them reachable (no silent clipping).
    #[test]
    fn the_settings_form_scrolls_and_keeps_its_actions() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let tokens = crate::theme::tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let mut state = AppState::new();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(900.0, 600.0));
        let run = |state: &mut AppState, time: f64| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let mut effects = Vec::new();
                        super::ui(ui, &tokens, &motion, state, &mut effects);
                    });
            })
        };
        let _ = run(&mut state, 0.0);
        let output = run(&mut state, 0.1);
        // The Save label paints (it is the form's tail — present in the
        // scrollable content even when the viewport shows only its top).
        let labels: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for expected in ["Transcription", "Appearance", "Audio sources", "Save"] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing {expected:?}; got {labels:?}"
            );
        }
        // The scroll area exists: its viewport is smaller than the content
        // (the content extends past the pane's bottom edge).
        let content_bottom = output
            .shapes
            .iter()
            .map(|clipped| clipped.shape.visual_bounding_rect().bottom())
            .fold(0.0_f32, f32::max);
        assert!(
            content_bottom > 600.0,
            "the form's content must extend past the 600 pt pane (scrollable): {content_bottom}"
        );
    }

    /// The y of a painted text's galley origin, if that label paints this frame.
    fn text_y(output: &egui::FullOutput, label: &str) -> Option<f32> {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos.y),
                _ => None,
            })
    }

    /// M9e (P2-3) caption wrap: at the 900×600 minimum the pane is 693 pt wide
    /// and the appearance caption's field column ~490 pt, while the caption
    /// measures ~649 pt — it must wrap onto ≥ 2 rows instead of running to the
    /// pane edge and clipping ("the text was cut off after 'App'" was exactly
    /// this). A label's default wrap mode inside a horizontal layout is
    /// `Extend` (no wrapping at all), so this guards `pane_caption`'s explicit
    /// `.wrap()`. Rendered through `views::draw` so the sidebar (and thus the
    /// real pane width) is in place.
    #[test]
    fn the_captions_wrap_at_the_minimum_pane_instead_of_clipping() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        state.route = Route::Settings;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(900.0, 600.0));
        let run = |state: &mut AppState, time: f64| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                crate::app::views::draw(ctx, state);
            })
        };
        let _ = run(&mut state, 0.0);
        let mut time = 0.1;
        let mut output = run(&mut state, time);
        // Settle the route fade so the pane's shapes are really emitted.
        for _ in 0..12 {
            time += 0.05;
            output = run(&mut state, time);
        }
        let mut found = false;
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                if text.galley.text().contains("forces this app's appearance") {
                    found = true;
                    println!(
                        "CAPTION rows={} size={:?} rect={:?}",
                        text.galley.rows.len(),
                        text.galley.size(),
                        text.visual_bounding_rect()
                    );
                    for (i, row) in text.galley.rows.iter().enumerate() {
                        println!(
                            "  row {i}: pos {:?} size {:?} glyphs {}",
                            row.pos,
                            row.row.size,
                            row.row.glyphs.len()
                        );
                    }
                    assert!(
                        text.galley.rows.len() >= 2,
                        "the caption must wrap at the minimum pane, got {} row(s)",
                        text.galley.rows.len()
                    );
                    assert!(
                        text.visual_bounding_rect().right() <= 875.0,
                        "the caption must not run past the pane edge: {:?}",
                        text.visual_bounding_rect()
                    );
                }
            }
        }
        assert!(found, "the appearance caption must paint");
    }

    /// M9e (P2-3) scroll isolation: the form and the models table are siblings
    /// under the same central panel; without an explicit `id_salt` egui hands
    /// both scroll areas the same id and the form's offset leaks into the
    /// table (on the 900×600 e2e shot Models opened scrolled past its Re-check
    /// and header). Scroll the form, switch routes, and assert the table opens
    /// at its own top.
    #[test]
    fn the_settings_scroll_offset_does_not_leak_into_the_models_table() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        state.route = Route::Settings;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(900.0, 600.0));
        let run = |state: &mut AppState, time: f64, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                events,
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                crate::app::views::draw(ctx, state);
            })
        };
        let _ = run(&mut state, 0.0, vec![]);
        let _ = run(&mut state, 0.1, vec![]);
        // Wheel-scroll the form down: the pointer sits in its viewport.
        let _ = run(
            &mut state,
            0.2,
            vec![
                egui::Event::PointerMoved(pos2(550.0, 300.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -900.0),
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let mut time = 0.25;
        let mut output = None;
        for _ in 0..16 {
            time += 0.05;
            output = Some(run(&mut state, time, vec![]));
        }
        // The form really did scroll: its tail (Save) is on screen.
        let save_y =
            text_y(output.as_ref().unwrap(), "Save").expect("Save paints after the scroll");
        assert!(
            (142.0..594.0).contains(&save_y),
            "the form must scroll its actions into view: Save at y {save_y}"
        );
        // Switch to Models: the table opens at its own top, not mid-scroll.
        state.route = Route::Models;
        let mut output = None;
        for _ in 0..16 {
            time += 0.05;
            output = Some(run(&mut state, time, vec![]));
        }
        let header_y = text_y(output.as_ref().unwrap(), "MODEL").expect("the models header paints");
        assert!(
            header_y < 220.0,
            "the models table must open at its top (Re-check + header), \
             not carry the form's scroll offset: header at y {header_y}"
        );
    }
}
