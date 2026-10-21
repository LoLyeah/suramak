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
use crate::app::fast_engine;
use crate::motion::Motion;
use crate::theme::Tokens;

use super::model_manager::DownloadState;
use super::settings_form::{self, SettingsForm, LANGUAGES};
use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect};

/// System-audio entries exist on macOS builds only (hidden elsewhere; Addendum A).
const SYSTEM_AUDIO_OFFERED: bool = cfg!(target_os = "macos");

/// M12: whether this build carries the in-app updater (the `updates` cargo
/// feature on a macOS target — `crate::updates::compiled()`). Drives the
/// Updates section's honest platform note; the row itself always exists (the
/// setting is persisted by every build).
const UPDATES_COMPILED: bool = crate::updates::compiled();

/// M14b: whether this build can reveal a folder in the platform file manager.
/// Windows only — macOS has no reveal action in the app today, and the Linux
/// build is layout-truth. The code path stays compiled everywhere (so the host
/// build type-checks it) and simply never runs elsewhere.
const REVEAL_IN_FOLDER: bool = cfg!(windows);

const ROW_H: f32 = 26.0;
const LABEL_W: f32 = 150.0;
/// Air kept between the scroll viewport's bottom edge and an **opaque** record
/// pill's top edge (M14e): a painted pill hides whatever is under it, so the
/// viewport must end above it.
const PILL_CLEARANCE: f32 = 10.0;
/// Tail reserve for the **glass** pill case (macOS 26+): content still scrolls
/// under the real material (the intended floating look), so the form's actions
/// need to clear it at the end of the scroll — the M9e value, unchanged.
const GLASS_FORM_BOTTOM_RESERVE: f32 = 76.0;
/// Tail reserve when the viewport already ends above a painted pill: only air.
const PAINTED_FORM_BOTTOM_RESERVE: f32 = 14.0;

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
    //
    // M14e (two Windows findings at once):
    // * top — the scroll starts **below the header as measured**, not at a
    //   mac-tuned 96 pt. A platform whose fonts set the title/caption block
    //   taller had the scroll's clip edge cut into the first heading
    //   ("Appearance" clipped at the top of the scroll).
    // * bottom — when the pill is the **painted** fallback (Windows, Linux, a
    //   pre-26 Mac) the viewport ends above its actual rect: a painted pill is
    //   opaque and hid "Check for new versions in the background" mid-scroll,
    //   which the old 76 pt *content* reserve only fixed at the very end of the
    //   scroll. Real glass (macOS 26+) is designed to float over content and
    //   keeps the old geometry exactly — the owner's Mac scrolls correctly
    //   today, so its layout is untouched.
    // `min_rect` here is the pane's max rect (a CentralPanel ui starts at its
    // full height), so the header's true end is the **flow cursor** after it.
    let header_bottom = ui.cursor().top();
    let viewport = ui.ctx().viewport_rect();
    let pill = suramak_glass::float_rect(
        (viewport.width(), viewport.height()),
        ui.ctx().pixels_per_point(),
    );
    let pill_top = viewport.min.y + pill.y;
    let painted_pill = !state.float_glass;
    let scroll_top = header_bottom.max(rect.top() + 96.0) + 10.0;
    let scroll_bottom = if painted_pill {
        (pill_top - PILL_CLEARANCE).min(rect.bottom() - 6.0)
    } else {
        rect.bottom() - 6.0
    }
    .max(scroll_top + 120.0);
    let tail_reserve = if painted_pill {
        PAINTED_FORM_BOTTOM_RESERVE
    } else {
        GLASS_FORM_BOTTOM_RESERVE
    };
    let scroll_rect = Rect::from_min_max(pos2(left, scroll_top), pos2(right, scroll_bottom));
    ui.scope_builder(egui::UiBuilder::new().max_rect(scroll_rect), |ui| {
        egui::ScrollArea::vertical()
            // The salt keeps this form's scroll offset its own: both this and
            // the models table are children of the same central panel, so
            // without it egui hands them the same id and the offset leaks
            // across views (Models would open mid-table after scrolling here).
            .id_salt("settings-form")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // A hair of air under the clip edge: a heading scrolled to the
                // top must not touch it.
                ui.add_space(2.0);
                form(ui, tokens, motion, state, effects, tail_reserve);
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
    tail_reserve: f32,
) {
    // The form sections borrow only `state.settings_form`; snapshots are
    // copied out first so the borrows stay disjoint.
    let devices = state.audio_devices.clone();
    let device_error = state.audio_devices_error.clone();
    let processes = state.audio_processes.clone();
    let processes_error = state.audio_processes_error.clone();
    let capture_available = state.capture_available;

    let mut dirty = false;
    // The form is borrowed in two spans (M14e): the readiness list between them
    // needs the whole state (probe + download), which cannot coexist with the
    // form borrow.
    dirty |= transcription_section_head(ui, tokens, &mut state.settings_form);
    fast_engine_readiness(ui, tokens, motion, state, effects);
    {
        let form = &mut state.settings_form;
        dirty |= transcription_section_tail(ui, tokens, motion, form);
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
        dirty |= updates_section(ui, tokens, form);
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
    // The tail reserve: 76 pt of air under the glass pill (macOS 26+, where
    // content still scrolls under the material), or plain air when the viewport
    // already ends above a painted pill (M14e — see `ui`).
    ui.add_space(tail_reserve);
}

/// Engine / default model / default language / keep_audio / recordings folder.
/// Returns whether the user changed something.
fn transcription_section_head(ui: &mut egui::Ui, tokens: &Tokens, form: &mut SettingsForm) -> bool {
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
    // M14e: the toggle's copy loses the CLI instruction — the readiness list
    // right below it installs, deletes and explains everything in place.
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
         The engine line names what actually ran.",
        false,
    );

    dirty
}

/// The rest of the Transcription section (model / language / keep_audio /
/// folder). Split from the head (M14e) so the fast-engine readiness list — which
/// needs the whole `AppState` (download state, the probe) — can sit between the
/// toggle and the model row without two mutable borrows of `state`.
fn transcription_section_tail(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    form: &mut SettingsForm,
) -> bool {
    let mut dirty = false;

    // Default model. M9c copy sweep: the combobox shows the human display
    // names; the raw id stays the value that persists.
    row(ui, tokens, "Default model", |ui| {
        let current = form.model_id.clone();
        let options = model_options(&current);
        // M14e: our dropdown (scrim + opaque sheet), not egui's translucent
        // popup — the owner's "the drop down not blurring the bg so it
        // cluttered" finding.
        let chosen = super::dropdown::dropdown(
            ui,
            tokens,
            motion,
            "suramak-default-model",
            &current,
            &super::model_manager::display_name(&current),
            &options,
            280.0,
        );
        if let Some(chosen) = chosen {
            if chosen != current {
                form.model_id = chosen;
                dirty = true;
            }
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
    let recordings_path = suramak_core::paths::recordings_dir().ok();
    row(ui, tokens, "Recordings folder", |ui| {
        let recordings = recordings_path
            .as_ref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "unavailable".to_string());
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
        // M14b: "reveal in folder" — Windows only, because the app has no reveal
        // action on any other platform (macOS included); this is the one entry,
        // and it uses `explorer /select,` via `crate::reveal`. The folder is
        // created first: `explorer` silently opens Documents for a missing path,
        // which would look like a bug.
        if REVEAL_IN_FOLDER && form.keep_audio {
            if let Some(dir) = &recordings_path {
                ui.add_space(8.0);
                let response = controls::button(
                    ui,
                    tokens,
                    motion,
                    "settings-reveal-recordings",
                    "Show in Explorer",
                    Tier::Tertiary,
                    true,
                );
                if response.clicked() {
                    let revealed = std::fs::create_dir_all(dir)
                        .and_then(|()| crate::reveal::reveal_in_folder(dir));
                    if let Err(err) = revealed {
                        tracing::warn!(
                            error = %err,
                            path = %dir.display(),
                            "reveal in Explorer failed"
                        );
                    }
                }
            }
        }
    });
    ui.add_space(8.0);
    dirty
}

/// M14e: the fast-engine **readiness list** — one line per fast engine, each
/// with a live status dot and its action inline.
///
/// Nothing here routes: Indonesian still never goes fast (the guard is pinned in
/// `suramak-asr::engine`), and the toggle above still decides whether covered
/// languages use a fast engine at all. This list only *reports* what this build
/// and this machine can do, and provisions the one thing that needs bytes on
/// disk — Parakeet — through the same installer the CLI runs.
fn fast_engine_readiness(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
) {
    let readiness = state.fast_engine;
    if !readiness.probed {
        // Before the first probe the list says nothing rather than guessing a
        // state (the probe runs when Settings opens).
        return;
    }
    let download = state.model_manager.download.clone();
    let any_download = download.is_running();
    let installing = download.active_id() == Some(core_models::PARAKEET.id);

    let mut effects_here: Vec<Effect> = Vec::new();
    let mut note: Option<String> = None;

    // ---- Apple SpeechTranscriber (macOS 26+) -------------------------------
    let stt = readiness.stt;
    row(ui, tokens, "Apple speech", |ui| {
        status_chip(ui, tokens, stt_color(tokens, stt), stt.ready(), stt.label());
        if stt.offers_permission_link() {
            ui.add_space(8.0);
            let allow = controls::button(
                ui,
                tokens,
                motion,
                "settings-stt-permission",
                "Allow speech recognition…",
                Tier::Secondary,
                true,
            )
            .on_hover_text("Open System Settings → Privacy & Security → Speech Recognition");
            if allow.clicked() {
                note = Some(match fast_engine::open_stt_permission_settings() {
                    Ok(()) => "System Settings → Privacy & Security → Speech Recognition is open"
                        .to_string(),
                    Err(err) => format!("could not open System Settings: {err}"),
                });
            }
        }
    });
    row_caption(ui, tokens, fast_engine::STT_CAPTION, false);

    // ---- Parakeet (the fast engine with a model to install) ----------------
    let parakeet = readiness.parakeet;
    let total = super::model_manager::human_size(core_models::parakeet_total_bytes());
    row(ui, tokens, "Parakeet (0.6B v3)", |ui| {
        status_chip(
            ui,
            tokens,
            parakeet_color(tokens, parakeet),
            parakeet.ready(),
            parakeet.label(),
        );
        if parakeet.ready() || parakeet.offers_download() {
            // The size is part of the decision, not a surprise after the click.
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("· {total}"))
                    .size(12.0)
                    .color(tokens.ink_soft),
            );
        }
        if parakeet.offers_download() && !installing {
            ui.add_space(8.0);
            let button = controls::button(
                ui,
                tokens,
                motion,
                "settings-parakeet-download",
                "Download",
                Tier::Secondary,
                !any_download,
            )
            .on_hover_text(format!(
                "Download {total} from HuggingFace (sha256-verified, resumable)"
            ));
            if button.clicked() {
                effects_here.extend(state.start_model_download(core_models::PARAKEET.id));
            }
        }
        if parakeet.offers_delete() && !installing {
            ui.add_space(6.0);
            let button = controls::button(
                ui,
                tokens,
                motion,
                "settings-parakeet-delete",
                "Delete",
                Tier::Tertiary,
                !any_download,
            )
            .on_hover_text("Remove the Parakeet model files from this machine");
            if button.clicked() {
                effects_here.extend(state.delete_model(core_models::PARAKEET.id));
            }
        }
    });
    row_caption(ui, tokens, fast_engine::PARAKEET_COVERAGE_CAPTION, false);
    // The coverage list, expandable (a collapsible line — never a wall of text).
    let open_id = ui.id().with("settings-parakeet-coverage-open");
    let mut open = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(open_id).unwrap_or(false));
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W);
        let count = fast_engine::parakeet_coverage_lines().len();
        let label = if open {
            "Hide languages".to_string()
        } else {
            format!("Show the {count} languages")
        };
        if controls::button(
            ui,
            tokens,
            motion,
            "settings-parakeet-coverage",
            &label,
            Tier::Tertiary,
            true,
        )
        .clicked()
        {
            open = !open;
            ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
        }
    });
    if open {
        row_caption(ui, tokens, &fast_engine::parakeet_coverage_hover(), false);
    }

    // ---- the install card (the existing job-card machinery) -----------------
    if installing && install_card(ui, tokens, motion, &download, &mut effects_here) {
        effects_here.extend(state.cancel_download());
    }

    for effect in effects_here {
        effects.push(effect);
    }
    if let Some(note) = note {
        state.settings_form.status = Some(Ok(note));
    }
    ui.add_space(4.0);
}

/// The readiness dot's colour: green when ready, amber when the user can do
/// something about it, faint when this build/machine cannot.
fn stt_color(tokens: &Tokens, status: fast_engine::SttStatus) -> egui::Color32 {
    match status {
        fast_engine::SttStatus::Available => tokens.ok,
        fast_engine::SttStatus::NeedsPermission => tokens.warn,
        fast_engine::SttStatus::NeedsMacOs26 | fast_engine::SttStatus::NotInThisBuild => {
            tokens.ink_faint
        }
    }
}

fn parakeet_color(tokens: &Tokens, status: fast_engine::ParakeetStatus) -> egui::Color32 {
    match status {
        fast_engine::ParakeetStatus::Installed => tokens.ok,
        fast_engine::ParakeetStatus::Partial => tokens.warn,
        fast_engine::ParakeetStatus::Missing | fast_engine::ParakeetStatus::NotInThisBuild => {
            tokens.ink_faint
        }
    }
}

/// A status dot + word (the readiness chip). The word carries the meaning for
/// screen readers and colour-blind users; the dot is the glance. A ready state
/// reads in full ink, anything else dims (the state is the one loud thing).
fn status_chip(ui: &mut egui::Ui, tokens: &Tokens, dot: egui::Color32, ready: bool, label: &str) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(9.0, 16.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, dot);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    ui.label(
        egui::RichText::new(label)
            .size(12.0)
            .color(if ready { tokens.ink } else { tokens.ink_soft })
            .family(egui::FontFamily::Proportional),
    );
}

/// The inline install card (M14e): the job-card pattern inside Settings —
/// bytes, per-file progress, the honest total, and a Cancel that really stops
/// the transfer. Returns true when Cancel was pressed.
fn install_card(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    download: &DownloadState,
    _effects: &mut Vec<Effect>,
) -> bool {
    let fraction = f32::from(download.percent().unwrap_or(0)) / 100.0;
    let height = 62.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(240.0) - LABEL_W, height),
        egui::Sense::hover(),
    );
    // Indent to the field column so the card belongs to the Parakeet row.
    let rect = rect.translate(egui::vec2(LABEL_W, 0.0));
    super::views::paint_card(ui.painter(), rect, tokens);
    let painter = ui.painter();
    let title = format!(
        "Installing {} — {} of {}",
        super::model_manager::display_name(core_models::PARAKEET.id),
        super::model_manager::human_size(download_bytes_done(download)),
        super::model_manager::human_size(core_models::parakeet_total_bytes()),
    );
    painter.text(
        egui::pos2(rect.left() + 12.0, rect.top() + 8.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        tokens.ink,
    );
    if let Some(line) = download.file_line() {
        painter.text(
            egui::pos2(rect.right() - 12.0, rect.top() + 8.0),
            egui::Align2::RIGHT_TOP,
            line,
            egui::FontId::new(10.5, egui::FontFamily::Monospace),
            tokens.ink_soft,
        );
    }
    let bar = Rect::from_min_size(
        egui::pos2(rect.left() + 12.0, rect.top() + 30.0),
        egui::vec2(rect.width() - 24.0, 8.0),
    );
    painter.rect_filled(bar, egui::CornerRadius::same(4), tokens.panel_faint);
    if fraction > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(bar.min, egui::vec2(bar.width() * fraction, bar.height())),
            egui::CornerRadius::same(4),
            tokens.accent,
        );
    }
    painter.text(
        egui::pos2(bar.left(), bar.bottom() + 4.0),
        egui::Align2::LEFT_TOP,
        format!("{}%", (fraction * 100.0).round() as u8),
        egui::FontId::new(10.5, egui::FontFamily::Monospace),
        tokens.ink_soft,
    );

    // Cancel (or the in-flight label): the run stops at its next chunk.
    let mut cancel = false;
    let cancel_rect = Rect::from_min_size(
        egui::pos2(rect.right() - 96.0, rect.top() + 26.0),
        egui::vec2(84.0, 24.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(cancel_rect), |ui| {
        let label = if download.is_cancelling() {
            "cancelling…"
        } else {
            "Cancel"
        };
        if controls::button(
            ui,
            tokens,
            motion,
            "settings-parakeet-cancel",
            label,
            Tier::Tertiary,
            !download.is_cancelling(),
        )
        .clicked()
        {
            cancel = true;
        }
    });
    cancel
}

fn download_bytes_done(download: &DownloadState) -> u64 {
    match download {
        DownloadState::Running { bytes_done, .. } => *bytes_done,
        _ => 0,
    }
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

    // M14e: the copy resolves per platform (it said "macOS" on Windows).
    row_caption(ui, tokens, crate::app::copy::theme_caption_here(), false);
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

/// M12: the Updates section's caption — the same string the view paints and
/// the paint test asserts, so the two can never drift.
fn updates_caption() -> &'static str {
    if UPDATES_COMPILED {
        "On by default. The packaged app checks for a new version in the background (about \
         once a day) and installs it in place — Sparkle shows the release notes and asks \
         before installing. Off: only the app menu's Check for Updates… checks. Development \
         builds (`cargo run`) have no updater; the release app does."
    } else {
        "On by default, but this build has no updater: in-app updates are macOS-only and \
         compiled out of this build (the `updates` cargo feature). The setting is persisted \
         and takes effect in a macOS build that has it."
    }
}

/// M12: the in-app updates section — one row ("Automatic updates", ON by
/// default) and the honest platform note. The updater itself is macOS-only
/// behind the default-OFF `updates` cargo feature (`crate::updates`); the row
/// always exists because the setting is persisted by every build.
fn updates_section(ui: &mut egui::Ui, tokens: &Tokens, form: &mut SettingsForm) -> bool {
    let mut dirty = false;
    section(ui, tokens, "Updates");

    row(ui, tokens, "Automatic updates", |ui| {
        if ui
            .checkbox(
                &mut form.updates_auto_check,
                "Check for new versions in the background",
            )
            .changed()
        {
            dirty = true;
        }
    });
    row_caption(ui, tokens, updates_caption(), false);
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

    /// M12: the Updates section really renders — the section heading, the
    /// "Automatic updates" row and its checkbox paint on a tall pane (the
    /// 900×600 minimum scrolls; labels below the viewport are not painted,
    /// so this test gives the form room). The toggle itself is covered by the
    /// `settings_form` round-trip tests; this is the view-level check.
    #[test]
    fn the_updates_section_and_its_row_paint() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let tokens = crate::theme::tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let mut state = AppState::new();
        // Tall enough that the whole form (which scrolls at 900×600) is visible.
        let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(900.0, 2400.0));
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
        let labels: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for expected in [
            "Updates",
            "Automatic updates",
            "Check for new versions in the background",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing {expected:?}; got {labels:?}"
            );
        }
        // The caption tells the truth for this build (compiled or not) — the
        // exact string the view uses.
        let caption = updates_caption();
        assert!(
            labels.iter().any(|label| label == caption),
            "the updates caption must paint; got {labels:?}"
        );
    }

    // ---- M14e: the fast-engine readiness list -------------------------------

    fn frame_settings(
        ctx: &egui::Context,
        state: &mut AppState,
        size: (f32, f32),
        time: f64,
    ) -> egui::FullOutput {
        let tokens = crate::theme::tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let screen = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(size.0, size.1));
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
    }

    fn painted_labels(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    /// M14e: the readiness list renders both rows with a live status and the
    /// inline action — the toggle's one checkbox plus a CLI hint is gone.
    #[test]
    fn the_readiness_list_shows_every_fast_engine_with_its_status_and_action() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        // A macOS-26 machine with speech permission granted and no Parakeet yet.
        state.fast_engine = fast_engine::FastEngineReadiness {
            stt: fast_engine::SttStatus::Available,
            parakeet: fast_engine::ParakeetStatus::Missing,
            probed: true,
        };
        // Tall enough that the whole form is laid out (the form scrolls at 900×600).
        let _ = frame_settings(&ctx, &mut state, (900.0, 2600.0), 0.0);
        let output = frame_settings(&ctx, &mut state, (900.0, 2600.0), 0.2);
        let labels = painted_labels(&output);
        for expected in [
            "Fast engine",
            "(experimental) — fast engines for covered languages",
            "Apple speech",
            "Available",
            "Parakeet (0.6B v3)",
            "Not downloaded",
            "Download",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing {expected:?}; got {labels:?}"
            );
        }
        // The two captions are painted verbatim (they are the copy pins).
        for caption in [
            fast_engine::STT_CAPTION,
            fast_engine::PARAKEET_COVERAGE_CAPTION,
        ] {
            assert!(
                labels.iter().any(|label| label == caption),
                "the caption must paint: {caption:?}"
            );
        }
        // No "Needs permission" control when the permission is granted.
        assert!(!labels.iter().any(|label| label.contains("Allow speech")));

        // …and the permission state offers the exact-pane control instead.
        state.fast_engine.stt = fast_engine::SttStatus::NeedsPermission;
        let _ = frame_settings(&ctx, &mut state, (900.0, 2600.0), 0.3);
        let output = frame_settings(&ctx, &mut state, (900.0, 2600.0), 0.4);
        let labels = painted_labels(&output);
        assert!(labels.iter().any(|label| label == "Needs permission"));
        assert!(
            labels
                .iter()
                .any(|label| label == "Allow speech recognition…"),
            "a needs-permission row must never be a dead end; got {labels:?}"
        );
    }

    /// M14e: an installed Parakeet offers Delete + its size and no Download.
    #[test]
    fn an_installed_parakeet_costs_a_delete_control_not_a_download() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        state.fast_engine = fast_engine::FastEngineReadiness {
            stt: fast_engine::SttStatus::NotInThisBuild,
            parakeet: fast_engine::ParakeetStatus::Installed,
            probed: true,
        };
        let _ = frame_settings(&ctx, &mut state, (900.0, 2600.0), 0.0);
        let output = frame_settings(&ctx, &mut state, (900.0, 2600.0), 0.2);
        let labels = painted_labels(&output);
        assert!(labels.iter().any(|label| label == "Installed"));
        assert!(labels.iter().any(|label| label == "Delete"));
        assert!(
            !labels.iter().any(|label| label == "Download"),
            "an installed model never offers a second download: {labels:?}"
        );
        assert!(
            labels.iter().any(|label| label.contains("670")),
            "the installed row states its size: {labels:?}"
        );
        // The SpeechTranscriber row is honest about a feature-off build.
        assert!(labels.iter().any(|label| label == "Not in this build"));
    }

    /// The copy seam (M14e): the Settings screen never tells a GUI user to run a
    /// CLI command. This is the check the workorder names — it scans every
    /// painted label of the real Settings frame.
    #[test]
    fn no_settings_copy_asks_the_user_to_run_a_cli_command() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        state.fast_engine = fast_engine::FastEngineReadiness {
            stt: fast_engine::SttStatus::NeedsPermission,
            parakeet: fast_engine::ParakeetStatus::Missing,
            probed: true,
        };
        let _ = frame_settings(&ctx, &mut state, (900.0, 2800.0), 0.0);
        let output = frame_settings(&ctx, &mut state, (900.0, 2800.0), 0.3);
        for label in painted_labels(&output) {
            // A backtick alone is fine ("feature `capture`" is a build
            // feature, not a command); the checks are the command shapes.
            for leak in [
                "suramak-cli",
                "--encoder",
                "models download",
                "cargo build",
                "cargo run",
            ] {
                assert!(
                    !label.contains(leak),
                    "Settings copy leaks {leak:?} to a GUI user: {label:?}"
                );
            }
        }
    }

    /// M14e (the Windows pill finding): the scroll viewport ends **above** the
    /// floating record pill's actual rect, so no row can sit under it, and the
    /// first heading is never clipped by a mac-tuned top offset.
    #[test]
    fn the_form_viewport_clears_the_pill_and_starts_under_the_measured_header() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        state.fast_engine = fast_engine::FastEngineReadiness {
            stt: fast_engine::SttStatus::NotInThisBuild,
            parakeet: fast_engine::ParakeetStatus::Missing,
            probed: true,
        };
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
                        super::ui(
                            ui,
                            &crate::theme::tokens(AppTheme::Light),
                            &Motion::new(false),
                            state,
                            &mut effects,
                        );
                    });
            })
        };
        let _ = run(&mut state, 0.0);
        let output = run(&mut state, 0.2);
        // The pill's rect at this window (the same function the painter and the
        // macOS glass use).
        let pill = suramak_glass::float_rect((900.0, 600.0), 1.0);
        // Every painted label of the form body must end above the pill's top.
        let mut body_shapes = 0;
        for clipped in &output.shapes {
            let bounds = clipped.shape.visual_bounding_rect();
            if !bounds.is_finite() || bounds.is_negative() || bounds.height() <= 0.0 {
                continue;
            }
            // Only what is actually *visible* counts: a control that straddles
            // the scroll viewport's edge is clipped away by the viewport, so the
            // test intersects each shape with its clip rect (the same thing the
            // renderer does).
            let visible = bounds.intersect(clipped.clip_rect);
            if !visible.is_positive() || visible.top() < 150.0 {
                continue;
            }
            body_shapes += 1;
            assert!(
                visible.bottom() <= pill.y,
                "a form shape is visible under the record pill ({visible:?} vs pill top {}): {bounds:?}",
                pill.y,
            );
        }
        assert!(body_shapes > 0, "the form body painted nothing");
        // …and the heading is inside the viewport (not clipped away).
        let labels = painted_labels(&output);
        assert!(labels.iter().any(|label| label == "Transcription"));
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
