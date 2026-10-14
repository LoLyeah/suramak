//! Settings route (M3; M6 multi-source list; M9a appearance row): engine,
//! default model/language, **appearance (System | Light | Dark)**, keep_audio +
//! the checkbox list of recording inputs (devices, system audio, "Only an
//! app…" with its running-audio-process picker), the recordings folder, and the
//! polish section (base URL / model / keychain key).
//!
//! All form rules live in `app::settings_form` (egui-free, unit-tested); this view
//! edits that form and asks the runtime to persist it (`Effect::SaveSettings`).
//! M9a: painted controls, the one navigation pattern (Home, top-right) and the
//! tokens; the appearance is applied live when the form saves (no restart).

use egui::{pos2, vec2, Align2, FontFamily, FontId, Rect};

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
        dirty |= transcription_section(ui, tokens, form, left, right, &mut y);
        dirty |= appearance_section(ui, tokens, motion, form, left, right, &mut y);
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
            left,
            right,
            &mut y,
        );
        dirty |= polish_section(ui, tokens, form, left, right, &mut y);
    }

    // ---- actions ------------------------------------------------------------
    let actions = Rect::from_min_size(pos2(left, y + 8.0), vec2(right - left, 62.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(actions), |ui| {
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
    });
}

/// Engine / default model / default language / keep_audio / recordings folder.
/// Returns whether the user changed something.
fn transcription_section(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    form: &mut SettingsForm,
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    section(ui, tokens, left, y, "Transcription");

    // Engine.
    label(ui, tokens, left, *y, "Engine");
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

    // Default model. M9c copy sweep: the combobox shows the human display
    // names; the raw id stays the value that persists.
    label(ui, tokens, left, *y, "Default model");
    let current = form.model_id.clone();
    let mut chosen = current.clone();
    inside(ui, left, right, *y, |ui| {
        egui::ComboBox::from_id_salt("suramak-default-model")
            .selected_text(super::model_manager::display_name(&current))
            .show_ui(ui, |ui| {
                for (value, display) in model_options(&current) {
                    ui.selectable_value(&mut chosen, value.clone(), display);
                }
            });
        if !core_models::MODELS.iter().any(|m| m.id == current) && current != "auto" {
            pane_caption(
                ui,
                "not a manifest id — treated as a file path at transcribe time",
                tokens,
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
    label(ui, tokens, left, *y, "Default language");
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
    *y += ROW_H;
    if form.language_custom {
        inside(ui, left, right, *y, |ui| {
            let (hint, error) = form.language_hint();
            pane_caption(ui, &hint, tokens, error);
        });
        *y += 22.0;
    }

    // keep_audio.
    label(ui, tokens, left, *y, "Keep recordings");
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
    label(ui, tokens, left, *y, "Recordings folder");
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
                .color(tokens.ink_soft),
        );
    });
    *y += ROW_H + 8.0;
    dirty
}

/// M9a appearance: System (follows the OS live) | Light | Dark, as a segmented
/// control. Applied when the form saves — no restart.
fn appearance_section(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    form: &mut SettingsForm,
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    section(ui, tokens, left, y, "Appearance");

    label(ui, tokens, left, *y, "Theme");
    inside(ui, left, right, *y, |ui| {
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
    *y += ROW_H;

    inside(ui, left, right, *y, |ui| {
        pane_caption(
            ui,
            "System follows the macOS light/dark setting live; Light or Dark forces this app's appearance. Applied when you press Save.",
            tokens,
            false,
        );
    });
    *y += 24.0;
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
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    section(ui, tokens, left, y, "Audio sources");

    label(ui, tokens, left, *y, "Inputs");
    inside(ui, left, right, *y, |ui| {
        pane_caption(
            ui,
            "pick one or more — everything ticked is recorded together into one file",
            tokens,
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
                tokens,
                motion,
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
                    pane_caption(ui, &problem, tokens, true);
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
                    pane_caption(ui, &format!("input devices: {error}"), tokens, true);
                } else if devices.is_empty() {
                    pane_caption(
                        ui,
                        "no input devices enumerated — the default entry still opens the system default input",
                        tokens,
                        false,
                    );
                } else {
                    pane_caption(
                        ui,
                        &format!(
                            "{} enumerated (virtual loopback devices like BlackHole appear here)",
                            super::count_noun(devices.len(), "input device")
                        ),
                        tokens,
                        false,
                    );
                }
            });
            if !capture_available {
                ui.horizontal(|ui| {
                    pane_caption(
                        ui,
                        "recording is not compiled into this build (feature `capture`) — the input list is read-only",
                        tokens,
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
    tokens: &Tokens,
    motion: &Motion,
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
                        tokens,
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
                        tokens,
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
    tokens: &Tokens,
    form: &mut SettingsForm,
    left: f32,
    right: f32,
    y: &mut f32,
) -> bool {
    let mut dirty = false;
    section(ui, tokens, left, y, "Polish (LLM tidy-up)");

    label(ui, tokens, left, *y, "Base URL");
    inside(ui, left, right, *y, |ui| {
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
    *y += ROW_H;

    label(ui, tokens, left, *y, "Model");
    inside(ui, left, right, *y, |ui| {
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
    *y += ROW_H;

    label(ui, tokens, left, *y, "API key");
    inside(ui, left, right, *y, |ui| {
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
    *y += ROW_H + 12.0;
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

/// Section heading; advances `y` past it.
fn section(ui: &mut egui::Ui, tokens: &Tokens, left: f32, y: &mut f32, title: &str) {
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(left, *y), vec2(400.0, 26.0))),
        |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .font(FontId::new(17.0, crate::theme::serif_family()))
                        .color(tokens.ink),
                );
            });
        },
    );
    *y += 30.0;
}

/// Left-hand row label painted at `y`.
fn label(ui: &mut egui::Ui, tokens: &Tokens, left: f32, y: f32, text: &str) {
    ui.painter().text(
        pos2(left + 2.0, y + 4.0),
        Align2::LEFT_TOP,
        text,
        FontId::new(12.0, FontFamily::Proportional),
        tokens.ink_soft,
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
