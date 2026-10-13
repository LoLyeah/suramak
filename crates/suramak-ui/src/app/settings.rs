//! Settings route (M3; M6 multi-source list): engine, default model/language,
//! keep_audio + the **checkbox list of recording inputs** (devices, system
//! audio, "Only an app…" with its running-audio-process picker), the recordings
//! folder, and the polish section (base URL / model / keychain key).
//!
//! All form rules live in `app::settings_form` (egui-free, unit-tested); this view
//! edits that form and asks the runtime to persist it (`Effect::SaveSettings`).

use egui::{pos2, vec2, Align2, FontFamily, FontId, Rect};

use suramak_asr::capture::AudioProcessInfo;
use suramak_core::models as core_models;
use suramak_core::settings::{EngineSetting, RecordSource};

use crate::theme::Palette;

use super::settings_form::{self, SettingsForm, LANGUAGES};
use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect};

/// System-audio entries exist on macOS builds only (hidden elsewhere; Addendum A).
const SYSTEM_AUDIO_OFFERED: bool = cfg!(target_os = "macos");

const ROW_H: f32 = 26.0;
const LABEL_W: f32 = 150.0;

pub fn ui(ui: &mut egui::Ui, palette: &Palette, state: &mut AppState, effects: &mut Vec<Effect>) {
    let rect = ui.max_rect();
    let left = rect.left() + PANE_PAD_X;
    let right = rect.right() - PANE_PAD_X;

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, "Settings", palette);
            pane_caption(
                ui,
                "Everything is stored locally (settings.toml); nothing here leaves this machine except model downloads and polish calls",
                palette,
                false,
            );
        });
    });

    // The form sections borrow only `state.settings_form`; snapshots are
    // copied out first so the borrows stay disjoint.
    let devices = state.audio_devices.clone();
    let device_error = state.audio_devices_error.clone();
    let processes = state.audio_processes.clone();
    let processes_error = state.audio_processes_error.clone();
    let capture_available = state.capture_available;
    let mut y = rect.top() + 96.0;

    let mut dirty = false;
    {
        let form = &mut state.settings_form;
        dirty |= transcription_section(ui, palette, form, left, right, &mut y);
        dirty |= audio_source_section(
            ui,
            palette,
            form,
            &devices,
            device_error.as_deref(),
            &processes,
            processes_error.as_deref(),
            capture_available,
            effects,
            left,
            right,
            &mut y,
        );
        dirty |= polish_section(ui, palette, form, left, right, &mut y);
    }

    // ---- actions ------------------------------------------------------------
    let actions = Rect::from_min_size(pos2(left, y + 8.0), vec2(right - left, 62.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(actions), |ui| {
        ui.horizontal(|ui| {
            if ui.button("Back").clicked() {
                effects.extend(state.back_home());
            }
            ui.add_space(6.0);
            if ui.button("Save").clicked() {
                effects.extend(state.save_settings());
            }
            if ui.button("Reset to defaults").clicked() {
                state.reset_settings_form();
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if let Some(status) = &state.settings_form.status {
                match status {
                    Ok(note) => pane_caption(ui, note, palette, false),
                    Err(err) => pane_caption(ui, err, palette, true),
                }
            } else if dirty {
                pane_caption(ui, "unsaved changes — press Save", palette, false);
            }
        });
    });
}

/// Engine / default model / default language / keep_audio / recordings folder.
/// Returns whether the user changed something.
fn transcription_section(
    ui: &mut egui::Ui,
    palette: &Palette,
    form: &mut SettingsForm,
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    section(ui, palette, left, y, "Transcription");

    // Engine.
    label(ui, palette, left, *y, "Engine");
    inside(ui, left, right, *y, |ui| {
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
    *y += ROW_H;

    // Default model.
    label(ui, palette, left, *y, "Default model");
    let current = form.model_id.clone();
    let mut chosen = current.clone();
    inside(ui, left, right, *y, |ui| {
        egui::ComboBox::from_id_salt("suramak-default-model")
            .selected_text(chosen.clone())
            .show_ui(ui, |ui| {
                for option in model_options(&current) {
                    ui.selectable_value(&mut chosen, option.clone(), option);
                }
            });
        if !core_models::MODELS.iter().any(|m| m.id == current) && current != "auto" {
            pane_caption(
                ui,
                "not a manifest id — treated as a file path at transcribe time",
                palette,
                false,
            );
        }
    });
    if chosen != current {
        form.model_id = chosen;
        dirty = true;
    }
    *y += ROW_H;

    // Default language: the presets plus a free-text "Custom…" entry (M4.5).
    // The value that persists is whatever `settings_form` validates; an invalid
    // draft is refused there and only the inline hint shows it here.
    label(ui, palette, left, *y, "Default language");
    let mut custom_clicked = false;
    inside(ui, left, right, *y, |ui| {
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
                egui::TextEdit::singleline(&mut form.language_draft).hint_text("es, jv…"),
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
    *y += ROW_H;
    if form.language_custom {
        inside(ui, left, right, *y, |ui| {
            let (hint, error) = form.language_hint();
            pane_caption(ui, &hint, palette, error);
        });
        *y += 22.0;
    }

    // keep_audio.
    label(ui, palette, left, *y, "Keep recordings");
    inside(ui, left, right, *y, |ui| {
        if ui
            .checkbox(&mut form.keep_audio, "keep audio after transcription")
            .changed()
        {
            dirty = true;
        }
    });
    *y += ROW_H;

    // Recordings folder display.
    label(ui, palette, left, *y, "Recordings folder");
    let recordings = suramak_core::paths::recordings_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|_| "unavailable".to_string());
    inside(ui, left, right, *y, |ui| {
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
                .color(palette.text_dim),
        );
    });
    *y += ROW_H + 8.0;
    dirty
}

/// The M6 checkbox list of recording inputs: default mic, enumerated devices,
/// system audio (macOS), and "Only an app…" with its running-audio-process
/// picker. Everything ticked is recorded together and mixed into one file;
/// ≥1 source is required to save (the inline hint shows the refusal).
#[allow(clippy::too_many_arguments)]
fn audio_source_section(
    ui: &mut egui::Ui,
    palette: &Palette,
    form: &mut SettingsForm,
    devices: &[String],
    device_error: Option<&str>,
    processes: &[AudioProcessInfo],
    processes_error: Option<&str>,
    capture_available: bool,
    effects: &mut Vec<Effect>,
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    section(ui, palette, left, y, "Audio sources");

    label(ui, palette, left, *y, "Inputs");
    inside(ui, left, right, *y, |ui| {
        pane_caption(
            ui,
            "pick one or more — everything ticked is recorded together into one 16 kHz recording",
            palette,
            false,
        );
    });
    *y += ROW_H;

    // Default microphone.
    dirty |= source_checkbox(
        ui,
        left,
        right,
        y,
        capture_available,
        "Default microphone",
        form.is_checked(&RecordSource::Default),
        |checked| form.set_checked(RecordSource::Default, checked),
    );

    // Enumerated input devices.
    for name in devices {
        let source = RecordSource::Device { name: name.clone() };
        let checked = form.is_checked(&source);
        dirty |= source_checkbox(
            ui,
            left,
            right,
            y,
            capture_available,
            name,
            checked,
            |checked| form.set_checked(RecordSource::Device { name: name.clone() }, checked),
        );
    }

    // System audio (macOS builds only — hidden elsewhere, Addendum A).
    if SYSTEM_AUDIO_OFFERED {
        dirty |= source_checkbox(
            ui,
            left,
            right,
            y,
            capture_available,
            "System audio (all processes except Suramak)",
            form.is_checked(&RecordSource::SystemAudio),
            |checked| form.set_checked(RecordSource::SystemAudio, checked),
        );

        // "Only an app…" — the per-app entry + its process picker (M6).
        dirty |= source_checkbox(
            ui,
            left,
            right,
            y,
            capture_available,
            "Only an app…",
            form.app_audio_apps().is_some(),
            |checked| form.set_app_audio_checked(checked),
        );
        if let Some(apps) = form.app_audio_apps() {
            let apps = apps.to_vec();
            dirty |= app_picker(
                ui,
                palette,
                form,
                processes,
                processes_error,
                capture_available,
                &apps,
                effects,
                left,
                right,
                y,
            );
        }
    }

    // Validation hint (never zero sources; the app entry needs ≥1 app).
    if let Some(problem) = form.sources_problem() {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(left, *y),
                vec2(right - left, 20.0),
            )),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(ui, &problem, palette, true);
                });
            },
        );
        *y += 22.0;
    }

    // Enumeration notes (the existing copy, capture-less builds included).
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, *y),
            vec2(right - left, 44.0),
        )),
        |ui| {
            ui.horizontal(|ui| {
                if let Some(error) = device_error {
                    pane_caption(ui, &format!("input devices: {error}"), palette, true);
                } else if devices.is_empty() {
                    pane_caption(
                        ui,
                        "no input devices enumerated — the default entry still opens the system default input",
                        palette,
                        false,
                    );
                } else {
                    pane_caption(
                        ui,
                        &format!(
                            "{} input device(s) enumerated (virtual loopback devices like BlackHole appear here)",
                            devices.len()
                        ),
                        palette,
                        false,
                    );
                }
            });
            if !capture_available {
                ui.horizontal(|ui| {
                    pane_caption(
                        ui,
                        "recording is not compiled into this build (feature `capture`) — the input list is read-only",
                        palette,
                        true,
                    );
                });
            }
        },
    );
    *y += 46.0;
    dirty
}

/// One checkbox row of the source list. Returns whether the value changed.
#[allow(clippy::too_many_arguments)]
fn source_checkbox(
    ui: &mut egui::Ui,
    left: f32,
    right: f32,
    y: &mut f32,
    enabled: bool,
    label: &str,
    checked: bool,
    on_change: impl FnOnce(bool),
) -> bool {
    let mut value = checked;
    inside(ui, left, right, *y, |ui| {
        ui.add_enabled(enabled, egui::Checkbox::new(&mut value, label));
    });
    *y += ROW_H;
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
    palette: &Palette,
    form: &mut SettingsForm,
    processes: &[AudioProcessInfo],
    processes_error: Option<&str>,
    capture_available: bool,
    apps: &[String],
    effects: &mut Vec<Effect>,
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    let mut cursor = *y;
    let indent = left + 24.0;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(indent, cursor),
            vec2((right - indent).max(160.0), ROW_H),
        )),
        |ui| {
            ui.horizontal(|ui| {
                pane_caption(
                    ui,
                    "the tap captures only the ticked apps (a stereo mixdown of exactly those processes)",
                    palette,
                    false,
                );
                if ui.small_button("Refresh").clicked() {
                    effects.push(Effect::RefreshAudioProcesses);
                }
            });
        },
    );
    cursor += ROW_H;

    if let Some(error) = processes_error {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(indent, cursor),
                vec2((right - indent).max(160.0), ROW_H),
            )),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(
                        ui,
                        &format!("running audio processes: {error}"),
                        palette,
                        true,
                    );
                });
            },
        );
        cursor += ROW_H;
    } else if processes.is_empty() {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(indent, cursor),
                vec2((right - indent).max(160.0), ROW_H),
            )),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(
                        ui,
                        "no app is producing audio right now — start playback, then Refresh",
                        palette,
                        false,
                    );
                });
            },
        );
        cursor += ROW_H;
    }

    for process in processes {
        let key = process.settings_key();
        let checked = apps.iter().any(|app| app == &key);
        let mut value = checked;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(indent, cursor),
                vec2((right - indent).max(160.0), ROW_H),
            )),
            |ui| {
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        capture_available,
                        egui::Checkbox::new(&mut value, process_label(process)),
                    );
                });
            },
        );
        if value != checked {
            form.set_app_checked(&key, value);
            dirty = true;
        }
        cursor += ROW_H;
    }

    *y = cursor + 4.0;
    dirty
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
fn polish_section(
    ui: &mut egui::Ui,
    palette: &Palette,
    form: &mut SettingsForm,
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    section(ui, palette, left, y, "Polish (LLM tidy-up)");

    label(ui, palette, left, *y, "Base URL");
    inside(ui, left, right, *y, |ui| {
        let response = ui.add_sized(
            [360.0, 22.0],
            egui::TextEdit::singleline(&mut form.polish_base_url)
                .hint_text(settings_form::DEFAULT_POLISH_BASE_URL),
        );
        if response.changed() {
            dirty = true;
        }
    });
    *y += ROW_H;

    label(ui, palette, left, *y, "Model");
    inside(ui, left, right, *y, |ui| {
        let response = ui.add_sized(
            [360.0, 22.0],
            egui::TextEdit::singleline(&mut form.polish_model).hint_text("e.g. gpt-4o-mini"),
        );
        if response.changed() {
            dirty = true;
        }
    });
    *y += ROW_H;

    label(ui, palette, left, *y, "API key");
    inside(ui, left, right, *y, |ui| {
        if settings_form::api_key_supported() {
            let response = ui.add_sized(
                [360.0, 22.0],
                egui::TextEdit::singleline(&mut form.api_key)
                    .password(true)
                    .hint_text("stored in the OS keychain — never in settings.toml"),
            );
            if response.changed() {
                dirty = true;
            }
        } else {
            pane_caption(
                ui,
                "hidden on this platform — the key lives in the OS keychain on macOS/Windows; \
                 here set SURAMAK_OPENAI_API_KEY in the environment",
                palette,
                false,
            );
        }
    });
    *y += ROW_H + 12.0;
    dirty
}

/// Model dropdown options: `auto` + every manifest id (+ a custom value if the
/// settings carry one).
fn model_options(current: &str) -> Vec<String> {
    let mut options = vec!["auto".to_string()];
    options.extend(core_models::MODELS.iter().map(|m| m.id.to_string()));
    if !options.iter().any(|option| option == current) {
        options.insert(1, current.to_string());
    }
    options
}

/// Section heading; advances `y` past it.
fn section(ui: &mut egui::Ui, palette: &Palette, left: f32, y: &mut f32, title: &str) {
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(left, *y), vec2(400.0, 26.0))),
        |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .font(FontId::new(17.0, crate::theme::serif_family()))
                        .color(palette.text),
                );
            });
        },
    );
    *y += 30.0;
}

/// Left-hand row label painted at `y`.
fn label(ui: &mut egui::Ui, palette: &Palette, left: f32, y: f32, text: &str) {
    ui.painter().text(
        pos2(left + 2.0, y + 4.0),
        Align2::LEFT_TOP,
        text,
        FontId::new(12.0, FontFamily::Proportional),
        palette.text_dim,
    );
}

/// Runs `body` in the field column of row `y`.
fn inside(ui: &mut egui::Ui, left: f32, right: f32, y: f32, body: impl FnOnce(&mut egui::Ui)) {
    let rect = Rect::from_min_size(
        pos2(left + LABEL_W, y),
        vec2((right - left - LABEL_W).max(120.0), ROW_H),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.horizontal(body);
    });
}
