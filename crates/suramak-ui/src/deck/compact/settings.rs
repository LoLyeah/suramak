//! Compact Settings: language, models (with M13D's phone tiers), appearance,
//! storage, device facts, version and Quit.
//!
//! The phone's Settings is deliberately smaller than the desktop surface: no
//! HUD section (macOS-only), no fast-engine toggles that the Android build does
//! not compile, and no update channel. What remains is every setting the phone
//! build actually honours, plus the owner-requested Quit.

use egui::emath::GuiRounding;
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, ScrollArea, Stroke,
    StrokeKind, Ui, WidgetType,
};

use std::time::{Duration, Instant};

use suramak_core::settings::Appearance;

use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;

use super::a11y;
use super::chrome::{self, GUTTER, MIN_TOUCH};
use super::model::{bytes_label, CompactAction, CompactApp, CompactSetting, Insets, Sheet};

/// The DEVICE card's last line — the smoke's second probe field, kept because
/// it names the phone's disk truth (M19A2).
pub const DEVICE_NOTE: &str = "Everything runs on this phone. No analytics, no uploads.";

/// The Quit row's accessibility label — **the list's real last control**, and
/// therefore the element the bottom-clearance probe measures (M19A2). It is a
/// constant because the shell finds the row by label in the frame's a11y
/// registry; a string literal in two places would drift.
pub const QUIT_LABEL: &str = "Quit Suramak";

/// The clearance the content always leaves under its last control.
///
/// `floor` is the platform's minimum bottom inset: 0 on desktop (the value is
/// the platform's own inset plus the standard gutter) and
/// [`ANDROID_MIN_CLEARANCE`] on the phone, so a window that has not pushed its
/// insets yet — or an immersive one — still scrolls the Quit row clear of the
/// gesture pill instead of pinning it to the screen's edge.
pub fn bottom_clearance(inset_bottom: f32, floor: f32) -> f32 {
    inset_bottom.max(floor).max(0.0) + GUTTER
}

/// Android's minimum bottom clearance, in points: 24 dp, the gesture handle's
/// own height on every Android release that ships gesture navigation.
///
/// A point is a dp on Android (`pixels_per_point` is `density / 160`), which
/// is why this number is the platform's, not a design choice.
pub const ANDROID_MIN_CLEARANCE: f32 = 24.0;

pub fn ui(
    ui: &mut Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    screen: Rect,
) -> Vec<CompactAction> {
    let mut actions = Vec::new();
    let bands = chrome::bands(screen, state.insets.top, state.insets.bottom, None);
    chrome::background(ui, tokens, screen);
    let now = ui.input(|input| input.time);
    let (back, _) = chrome::app_bar(ui, tokens, motion, bands.app_bar, "Settings", true, None);
    if back {
        state.back(now);
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bands.content)
            .layout(Layout::top_down(Align::Min)),
    );
    let mut clicks: Vec<CompactAction> = Vec::new();
    let mut scroll = ScrollArea::vertical()
        .id_salt("compact-settings-scroll")
        .auto_shrink([false, false])
        .scroll_source(egui::containers::scroll_area::ScrollSource {
            drag: true,
            ..Default::default()
        });
    if let Some(offset) = state.take_settings_scroll() {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    scroll.show(&mut child, |ui| {
        let width = bands.content.width() - GUTTER * 2.0;
        let x = bands.content.left() + GUTTER;
        // M19A: the rows flow from the SCROLLED cursor, not from the outer
        // band's static top. `bands.content.top()` is where the viewport
        // begins; inside `ScrollArea::show` the content Ui's cursor starts
        // `offset` above it, so reading the cursor is what makes the rows move
        // when the finger does (the pattern `library.rs` already uses).
        //
        // The scroll area snaps its content origin to the physical pixel grid
        // (`round_to_pixels` then `round_ui`), which would leave the first row
        // a fraction of a pixel above the band it has always been drawn at and
        // soften every glyph in the shot. Add that remainder back: at offset 0
        // the frame is byte-identical to the pre-M19A render, and while
        // scrolling the remainder is a constant that never changes a row's
        // spacing.
        let snapped = bands
            .content
            .top()
            .round_to_pixels(ui.pixels_per_point())
            .round_ui();
        let top = ui.cursor().top() + (bands.content.top() - snapped);
        let mut y = top + 14.0;

        // ---- Transcription -------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Transcription");
        y += 18.0;
        let card = Rect::from_min_size(pos2(x, y), vec2(width, 168.0));
        chrome::card(ui, tokens, card);
        let (model_label, model_missing) = state.model_label();
        if chrome::setting_row_with_ink(
            ui,
            tokens,
            motion,
            Rect::from_min_size(pos2(x, y), vec2(width, 56.0)),
            "settings-language",
            "Language",
            language_label(&state.settings.language),
            true,
            None,
        )
        .clicked()
        {
            let next = next_language(&state.settings.language);
            state.apply_setting(CompactSetting::Language(next.clone()));
            clicks.push(CompactAction::Setting(CompactSetting::Language(next)));
        }
        // The model row opens the list below rather than a second screen. Its
        // value carries the disk truth (M19B): a configured model with no
        // verified file says so in the warning ink instead of reading "in use".
        let _ = chrome::setting_row_with_ink(
            ui,
            tokens,
            motion,
            Rect::from_min_size(pos2(x, y + 56.0), vec2(width, 56.0)),
            "settings-model",
            "Model",
            &model_label,
            false,
            model_missing.then_some(tokens.warn),
        );
        let keep = state.settings.keep_audio;
        let toggle_row = Rect::from_min_size(pos2(x, y + 112.0), vec2(width, 56.0));
        let response = chrome::setting_row(
            ui,
            tokens,
            motion,
            toggle_row,
            "settings-keep-audio",
            "Keep the recording",
            "Keep the source audio after transcription",
            true,
        );
        if response.clicked() {
            let next = !keep;
            state.apply_setting(CompactSetting::KeepAudio(next));
            clicks.push(CompactAction::Setting(CompactSetting::KeepAudio(next)));
        }
        let toggle = chrome::toggle(
            ui,
            tokens,
            motion,
            pos2(toggle_row.right() - 40.0, toggle_row.center().y),
            "keep-audio-toggle",
            "Keep the recording",
            keep,
        );
        let _ = toggle; // the whole row is the hit area
        y = card.bottom() + 22.0;

        // ---- Models ---------------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Models");
        y += 18.0;
        let tier_note = match state.tier {
            Some(tier) => format!("{} · {}", tier.label(), tier_note(tier)),
            None => "Device memory not reported by the shell".to_string(),
        };
        ui.painter().text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            tier_note,
            FontId::proportional(12.5),
            tokens.ink_soft,
        );
        y += 16.0;
        let offered: Vec<_> = state
            .models
            .iter()
            .filter(|row| row.offered || row.installed)
            .cloned()
            .collect();
        let hidden = state.models.len().saturating_sub(offered.len());
        for row in offered {
            let height = 74.0;
            let rect = Rect::from_min_size(pos2(x, y), vec2(width, height));
            let selected = state.settings.model_id == row.id;
            // M19B: the live job for THIS row (the card and the table read the
            // same `DownloadView` the job reports).
            let live = state.download.clone().filter(|download| {
                download.model == row.id
                    && matches!(
                        download.state,
                        super::model::DownloadState::Running | super::model::DownloadState::Paused
                    )
            });
            let row_state = row.state(selected, live.as_ref());
            let in_use = row_state.is_in_use();
            let response = a11y::touch(
                ui,
                tokens,
                rect,
                ("model-row", &row.id),
                WidgetType::Button,
                &format!("Model {}", row.id),
                row.offered,
            );
            let fill = if in_use {
                tokens.accent_tint
            } else if response.hovered() {
                tokens.surface_hover
            } else {
                tokens.card
            };
            ui.painter().rect(
                rect,
                CornerRadius::same(tokens.radius.card),
                fill,
                Stroke::new(
                    1.0_f32,
                    if in_use {
                        tokens.accent
                    } else {
                        tokens.hairline
                    },
                ),
                StrokeKind::Inside,
            );
            let ink = if row.offered {
                tokens.ink
            } else {
                tokens.ink_faint
            };
            // Only a verified + selected file may say "In use" (M19B); every
            // other state names what is really there.
            let action = match (row_state, live.is_some()) {
                (super::model::ModelState::Downloading, false) => "Resume",
                // The configured model with no file is the one the row must
                // not lie about: a primary Download, not "In use"/"Get".
                (super::model::ModelState::NotDownloaded, _) if selected => "Download",
                _ => row_state.action(),
            };
            let action_width = 92.0;
            // The action sits on the title line; the note below gets the
            // full card width so a manifest sentence never runs under it.
            let action_rect = Rect::from_min_size(
                pos2(rect.right() - action_width - 12.0, rect.top() + 9.0),
                vec2(action_width, 32.0),
            );
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.top() + 18.0),
                Align2::LEFT_CENTER,
                format!(
                    "{}  {}",
                    row.id,
                    if row.recommended {
                        "· recommended"
                    } else {
                        ""
                    }
                ),
                FontId::proportional(16.0),
                ink,
            );
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.top() + 37.0),
                Align2::LEFT_CENTER,
                row.subtitle(),
                FontId::proportional(12.0),
                tokens.ink_soft,
            );
            let note = match (row_state, live.as_ref()) {
                (super::model::ModelState::Downloading, Some(download)) => format!(
                    "Downloading · {}% · {} of {}",
                    download.percent,
                    bytes_label(download.bytes_done),
                    bytes_label(download.bytes_total)
                ),
                (super::model::ModelState::StaleCorrupt, _) => {
                    "The file on this phone is incomplete — download it again.".to_string()
                }
                (super::model::ModelState::NotDownloaded, _) if selected => {
                    "This is the model Suramak is set to, but its file is not here yet.".to_string()
                }
                _ => row.note.clone(),
            };
            ui.painter().text(
                pos2(rect.left() + 14.0, rect.top() + 57.0),
                Align2::LEFT_CENTER,
                truncate(&note, rect.width() - 28.0, 11.5),
                FontId::proportional(11.5),
                if row.offered {
                    tokens.ink_soft
                } else {
                    tokens.ink_faint
                },
            );
            let action_response = a11y::touch(
                ui,
                tokens,
                action_rect,
                ("model-action", &row.id),
                WidgetType::Button,
                &format!("{} {}", action, row.id),
                row.offered && row_state.is_actionable(),
            );
            let primary = row_state == super::model::ModelState::Installed
                || (row_state == super::model::ModelState::NotDownloaded && selected);
            let fills = if in_use {
                tokens.accent_tint
            } else if primary {
                tokens.accent
            } else {
                tokens.control_fill
            };
            ui.painter().rect(
                action_rect,
                CornerRadius::same(tokens.radius.control),
                fills,
                Stroke::new(
                    1.0_f32,
                    if in_use {
                        tokens.accent
                    } else {
                        tokens.hairline
                    },
                ),
                StrokeKind::Inside,
            );
            ui.painter().text(
                action_rect.center(),
                Align2::CENTER_CENTER,
                action,
                FontId::proportional(12.5),
                if in_use {
                    tokens.accent
                } else if primary {
                    tokens.on_accent
                } else {
                    tokens.ink
                },
            );
            if action_response.clicked() && row.offered && row_state.is_actionable() {
                match row_state {
                    super::model::ModelState::Installed => {
                        state.apply_setting(CompactSetting::Model(row.id.clone()));
                        clicks.push(CompactAction::Setting(CompactSetting::Model(
                            row.id.clone(),
                        )));
                    }
                    super::model::ModelState::Downloading if live.is_some() => {
                        clicks.push(CompactAction::CancelDownload);
                    }
                    // A stale/corrupt file is unlinked by the host before the
                    // transfer starts; a bare `.part` resumes where it stopped.
                    super::model::ModelState::NotDownloaded
                    | super::model::ModelState::StaleCorrupt
                    | super::model::ModelState::Downloading => {
                        clicks.push(CompactAction::DownloadModel(row.id.clone()));
                    }
                    super::model::ModelState::InUse => {}
                }
            } else if response.clicked()
                && row.offered
                && row_state == super::model::ModelState::Installed
            {
                state.apply_setting(CompactSetting::Model(row.id.clone()));
                clicks.push(CompactAction::Setting(CompactSetting::Model(
                    row.id.clone(),
                )));
            }
            y += height + 8.0;
        }
        if hidden > 0 {
            ui.painter().text(
                pos2(x, y + 2.0),
                Align2::LEFT_CENTER,
                format!(
                    "{} larger model{} hidden — not offered on this phone",
                    hidden,
                    if hidden == 1 { "" } else { "s" }
                ),
                FontId::proportional(11.5),
                tokens.ink_faint,
            );
            y += 18.0;
        }
        y += 14.0;

        // ---- Appearance ----------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Appearance");
        y += 18.0;
        let chips = Rect::from_min_size(pos2(x, y), vec2(width, MIN_TOUCH));
        ui.painter().rect(
            chips,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
        let chip_w = width / 3.0;
        for (index, appearance) in Appearance::ALL.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(
                    chips.left() + index as f32 * chip_w + 3.0,
                    chips.top() + 3.0,
                ),
                vec2(chip_w - 6.0, chips.height() - 6.0),
            );
            let selected = state.settings.appearance == *appearance;
            let response = a11y::touch(
                ui,
                tokens,
                rect,
                ("appearance", appearance.label()),
                WidgetType::RadioButton,
                &format!("Appearance: {}", appearance.label()),
                true,
            );
            if selected {
                ui.painter().rect(
                    rect,
                    CornerRadius::same(tokens.radius.control),
                    tokens.card,
                    Stroke::new(1.0_f32, tokens.accent),
                    StrokeKind::Inside,
                );
            }
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                appearance.label(),
                FontId::proportional(13.5),
                if selected {
                    tokens.ink
                } else {
                    tokens.ink_soft
                },
            );
            if response.clicked() && !selected {
                state.settings.appearance = *appearance;
                clicks.push(CompactAction::Setting(CompactSetting::Appearance(
                    *appearance,
                )));
            }
        }
        y = chips.bottom() + 22.0;

        // ---- Device / About ------------------------------------------
        chrome::section_label(ui, tokens, pos2(x, y), "Device");
        y += 18.0;
        let device_card = Rect::from_min_size(pos2(x, y), vec2(width, 84.0));
        chrome::card(ui, tokens, device_card);
        let ram = state
            .ram_bytes
            .map(|bytes| format!("{:.0} GB usable", bytes as f64 / 1_000_000_000.0))
            .unwrap_or_else(|| "memory not reported".to_string());
        ui.painter().text(
            pos2(device_card.left() + 14.0, device_card.top() + 24.0),
            Align2::LEFT_CENTER,
            format!("Suramak {} · Android arm64", state.version),
            FontId::proportional(14.0),
            tokens.ink,
        );
        ui.painter().text(
            pos2(device_card.left() + 14.0, device_card.top() + 48.0),
            Align2::LEFT_CENTER,
            ram,
            FontId::proportional(12.5),
            tokens.ink_soft,
        );
        ui.painter().text(
            pos2(device_card.left() + 14.0, device_card.top() + 68.0),
            Align2::LEFT_CENTER,
            DEVICE_NOTE,
            FontId::proportional(11.5),
            tokens.ink_faint,
        );
        // The DEVICE note stays a named label (TalkBack reads the sentence
        // instead of anonymous paint) and a second probe field in the shell's
        // layout line. It is no longer the clearance witness: Diagnostics and
        // the Quit row follow it, so at full scroll Quit is what sits under the
        // gesture pill (M19A2).
        a11y::describe(
            ui,
            Rect::from_min_size(
                pos2(device_card.left() + 14.0, device_card.top() + 60.0),
                vec2(width - 28.0, 16.0),
            ),
            DEVICE_NOTE,
        );
        y = device_card.bottom() + 22.0;

        // ---- Diagnostics (M13g3) --------------------------------------
        // The M13b spike's four checks live here now, one card per group, with
        // the v2.4 crash banner. It stays a push (back returns to Settings) and
        // every read-out is on that screen.
        chrome::section_label(ui, tokens, pos2(x, y), "Diagnostics");
        y += 18.0;
        let diagnostics_row = Rect::from_min_size(pos2(x, y), vec2(width, 58.0));
        let response = chrome::setting_row(
            ui,
            tokens,
            motion,
            diagnostics_row,
            "settings-diagnostics",
            "Diagnostics",
            "Four checks · last crash",
            true,
        );
        if response.clicked() {
            state.open_diagnostics(now);
        }
        y = diagnostics_row.bottom() + 22.0;

        // ---- Quit -----------------------------------------------------
        let quit = Rect::from_min_size(pos2(x, y), vec2(width, 52.0));
        let response = a11y::touch(
            ui,
            tokens,
            quit,
            "settings-quit",
            WidgetType::Button,
            QUIT_LABEL,
            true,
        );
        ui.painter().rect(
            quit,
            CornerRadius::same(tokens.radius.control),
            Color32::TRANSPARENT,
            Stroke::new(1.0_f32, tokens.record_red.gamma_multiply(0.55)),
            StrokeKind::Inside,
        );
        ui.painter().text(
            quit.center(),
            Align2::CENTER_CENTER,
            QUIT_LABEL,
            FontId::proportional(15.0),
            tokens.record_red_text,
        );
        if response.clicked() {
            state.request_quit();
        }
        y = quit.bottom() + 30.0;
        // M19A bottom clearance, M19A2 floor. The viewport ends at the content
        // band's bottom, which already excludes the gesture inset the shell
        // reports — but a phone that has not pushed its insets yet (or an
        // immersive window) reports zero, and a maxed-out scroll then pins the
        // last control to the pill. Padding the CONTENT by the inset (or the
        // platform's floor) plus the standard gutter is what the Quit row
        // scrolls above; without it the scroll range ends exactly at the row's
        // own edge.
        let tail = bottom_clearance(state.insets.bottom, state.clearance_floor);
        ui.allocate_space(vec2(width, (y - top).max(0.0) + tail));
    });

    actions.extend(clicks);
    if state.sheet == Some(Sheet::Quit) {
        // The sheet itself is drawn by the dispatcher.
    }
    actions
}

/// The geometry one `SURAMAK-LAYOUT` line carries (M19A2).
///
/// `quit` is the list's **real last control**: Diagnostics and the Quit row
/// follow the DEVICE card, so at full scroll the Quit row is what sits under
/// the gesture pill. M19A probed the DEVICE note — a middle element that
/// clears the pill even when Quit is clipped under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutProbe {
    pub quit: Rect,
    pub device_note: Rect,
    pub inset_top: f32,
    pub inset_bottom: f32,
    /// The padding the list leaves under its last control, floor included.
    pub clearance: f32,
    /// Where the scroll viewport ends: the content band's bottom, i.e. the
    /// screen's bottom minus the bottom inset.
    pub viewport_bottom: f32,
    pub screen_bottom: f32,
}

impl LayoutProbe {
    /// Reads both probe controls out of the frame's a11y registry.
    ///
    /// `None` when either is missing — which is exactly "the Settings list is
    /// not on screen this frame", since the two are painted together.
    pub fn from_controls(
        controls: &[a11y::Control],
        insets: Insets,
        floor: f32,
        screen: Rect,
    ) -> Option<Self> {
        let find = |label: &str| {
            controls
                .iter()
                .find(|control| control.label == label)
                .map(|control| control.rect)
        };
        let quit = find(QUIT_LABEL)?;
        let device_note = find(DEVICE_NOTE)?;
        Some(Self {
            quit,
            device_note,
            inset_top: insets.top,
            inset_bottom: insets.bottom,
            clearance: bottom_clearance(insets.bottom, floor),
            viewport_bottom: screen.bottom() - insets.bottom,
            screen_bottom: screen.bottom(),
        })
    }

    /// The line's geometry identity, in tenths of a point.
    ///
    /// Two frames with the same key drew the list at the same place; the
    /// settle detector compares these, not floats, so pixel-snap noise cannot
    /// keep a resting list looking busy.
    pub fn key(&self) -> ProbeKey {
        fn tenths(value: f32) -> i32 {
            (value * 10.0).round() as i32
        }
        ProbeKey {
            quit_top: tenths(self.quit.top()),
            quit_bottom: tenths(self.quit.bottom()),
            device_bottom: tenths(self.device_note.bottom()),
            inset_top: tenths(self.inset_top),
            inset_bottom: tenths(self.inset_bottom),
            clearance: tenths(self.clearance),
        }
    }

    /// One machine-readable line. `ts` is the device's Unix time in
    /// milliseconds, so the emulator smoke can prove the line is newer than
    /// the swipe it asserts on; `settled` marks a line written after the
    /// geometry held still, i.e. the resting scroll position.
    pub fn line(&self, timestamp_ms: u128, settled: bool) -> String {
        format!(
            "SURAMAK-LAYOUT ts={timestamp_ms} settled={} route=Settings \
             quit_top={:.1} quit_bottom={:.1} device_top={:.1} device_bottom={:.1} \
             clearance={:.1} viewport_bottom={:.1} content_bottom={:.1} \
             screen_bottom={:.1} inset_bottom={:.1} inset_top={:.1}",
            settled as u8,
            self.quit.top(),
            self.quit.bottom(),
            self.device_note.top(),
            self.device_note.bottom(),
            self.clearance,
            self.viewport_bottom,
            self.viewport_bottom,
            self.screen_bottom,
            self.inset_bottom,
            self.inset_top,
        )
    }
}

/// [`LayoutProbe`]'s comparable geometry ([`LayoutProbe::key`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeKey {
    quit_top: i32,
    quit_bottom: i32,
    device_bottom: i32,
    inset_top: i32,
    inset_bottom: i32,
    clearance: i32,
}

/// The longest a changed probe may go unlogged: four lines a second.
pub const PROBE_LOG_INTERVAL: Duration = Duration::from_millis(250);

/// How long the geometry must hold before it counts as the resting position.
pub const PROBE_SETTLE: Duration = Duration::from_millis(300);

/// Decides when the shell writes a `SURAMAK-LAYOUT` line (M19A2).
///
/// M19A used a one-second wall clock. A swipe that ends in the same second as
/// its last log left the shell's `tail -1` reading a **mid-swipe** line — and
/// under reactive rendering (`ControlFlow::Wait`) the draw that would have
/// written a newer one may never happen at all. This gate logs
///
/// * whenever the geometry **changes**, at most [`PROBE_LOG_INTERVAL`] apart;
/// * once, [`PROBE_SETTLE`] after the geometry stops changing — the resting
///   position the smoke asserts on.
#[derive(Debug, Default)]
pub struct ProbeLog {
    current: Option<ProbeKey>,
    current_since: Option<Instant>,
    /// The key the resting line has already been written for.
    settled: Option<ProbeKey>,
    logged_at: Option<Instant>,
}

impl ProbeLog {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `Some(settled)` when a line should be written now: `false` for a change
    /// line, `true` for the resting line.
    pub fn observe(&mut self, key: ProbeKey, now: Instant) -> Option<bool> {
        if self.current != Some(key) {
            self.current = Some(key);
            self.current_since = Some(now);
            // A new position owes its own resting line, even if this exact
            // key was resting earlier (swipe away and back).
            self.settled = None;
            if self
                .logged_at
                .is_some_and(|at| now.saturating_duration_since(at) < PROBE_LOG_INTERVAL)
            {
                return None;
            }
            self.logged_at = Some(now);
            return Some(false);
        }
        if self.settled == Some(key) {
            return None;
        }
        if now.saturating_duration_since(self.current_since.unwrap_or(now)) < PROBE_SETTLE {
            return None;
        }
        self.settled = Some(key);
        self.logged_at = Some(now);
        Some(true)
    }

    /// When the resting line is still owed, the instant it becomes due.
    ///
    /// The shell asks egui for a repaint at that instant: `ControlFlow::Wait`
    /// stops drawing the moment the finger stops, so without a timer the
    /// resting line would wait for the next unrelated frame.
    pub fn pending(&self) -> Option<Instant> {
        let key = self.current?;
        if self.settled == Some(key) {
            return None;
        }
        Some(self.current_since? + PROBE_SETTLE)
    }
}

fn language_label(language: &str) -> &'static str {
    match language {
        "en" => "English",
        "id" => "Indonesian",
        _ => "Auto-detect",
    }
}

fn next_language(language: &str) -> String {
    match language {
        "auto" => "en".to_string(),
        "en" => "id".to_string(),
        _ => "auto".to_string(),
    }
}

fn tier_note(tier: suramak_core::device::PhoneTier) -> &'static str {
    use suramak_core::device::PhoneTier;
    match tier {
        PhoneTier::High => "the small model is the phone default",
        PhoneTier::Mid => "base is the default; small is available",
        PhoneTier::Low => "base is the default",
        PhoneTier::BelowFloor => "only tiny runs here",
        PhoneTier::Unsupported => "no 64-bit arm64 support",
    }
}

/// Truncates a model note to the width left of the action column, with an
/// ellipsis (the notes come from the manifest and are longer than a phone row).
fn truncate(text: &str, width: f32, font_size: f32) -> String {
    let approximate = (width / (font_size * 0.46)).max(8.0) as usize;
    if text.chars().count() <= approximate {
        return text.to_string();
    }
    let mut out: String = text.chars().take(approximate.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn probe(top: f32) -> LayoutProbe {
        LayoutProbe {
            quit: Rect::from_min_size(pos2(16.0, top), egui::vec2(300.0, 52.0)),
            device_note: Rect::from_min_size(pos2(16.0, top - 200.0), egui::vec2(300.0, 16.0)),
            inset_top: 24.0,
            inset_bottom: 54.0 / 2.6,
            clearance: bottom_clearance(54.0 / 2.6, ANDROID_MIN_CLEARANCE),
            viewport_bottom: 900.0,
            screen_bottom: 920.8,
        }
    }

    #[test]
    fn the_clearance_has_a_floor_and_desktops_keep_the_old_number() {
        assert_eq!(
            bottom_clearance(0.0, 0.0),
            GUTTER,
            "a desktop window with no inset keeps the plain gutter"
        );
        assert_eq!(
            bottom_clearance(0.0, ANDROID_MIN_CLEARANCE),
            ANDROID_MIN_CLEARANCE + GUTTER,
            "an Android window that reports zero still leaves room for the pill"
        );
        assert_eq!(
            bottom_clearance(30.0, ANDROID_MIN_CLEARANCE),
            30.0 + GUTTER,
            "a real inset above the floor is used as reported"
        );
        assert_eq!(
            bottom_clearance(-4.0, ANDROID_MIN_CLEARANCE),
            ANDROID_MIN_CLEARANCE + GUTTER,
            "a negative inset is not trusted"
        );
    }

    #[test]
    fn the_probe_line_carries_the_quit_row_and_the_viewport() {
        let line = probe(800.0).line(1_763_000_000_123, true);
        assert!(
            line.starts_with("SURAMAK-LAYOUT ts=1763000000123 settled=1"),
            "{line}"
        );
        assert!(line.contains("quit_bottom=852.0"), "{line}");
        assert!(line.contains("device_bottom=616.0"), "{line}");
        assert!(line.contains("viewport_bottom=900.0"), "{line}");
        assert!(line.contains("content_bottom=900.0"), "{line}");
        assert!(line.contains("inset_bottom=20.8"), "{line}");
        assert!(line.contains("inset_top=24.0"), "{line}");
        assert!(
            line.contains("clearance=40.0"),
            "the phone floor lifts a 20.8 pt inset to 24 dp + gutter: {line}"
        );
        assert!(
            !probe(800.0).line(0, false).contains("settled=1"),
            "a change line is not a settled line"
        );
    }

    #[test]
    fn a_changed_probe_logs_at_once_but_never_faster_than_four_hertz() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        assert_eq!(
            log.observe(probe(800.0).key(), start),
            Some(false),
            "the first frame of Settings is worth a line"
        );
        assert_eq!(
            log.observe(probe(760.0).key(), start + Duration::from_millis(100)),
            None,
            "a 60 Hz drag must not bury logcat"
        );
        assert_eq!(
            log.observe(probe(720.0).key(), start + Duration::from_millis(260)),
            Some(false),
            "four lines a second is the ceiling"
        );
    }

    #[test]
    fn a_resting_list_logs_once_after_the_settle_window() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        let key = probe(800.0).key();
        assert_eq!(log.observe(key, start), Some(false));
        assert_eq!(
            log.observe(key, start + Duration::from_millis(200)),
            None,
            "not settled yet"
        );
        assert_eq!(
            log.observe(key, start + Duration::from_millis(301)),
            Some(true),
            "the resting position gets its own line"
        );
        assert_eq!(
            log.observe(key, start + Duration::from_millis(900)),
            None,
            "and only one"
        );
    }

    #[test]
    fn a_throttled_change_still_gets_its_resting_line() {
        // The mid-swipe shape the audit found: two changes inside one 250 ms
        // window, so the second is throttled away — the resting line at +300 ms
        // is what `tail -1` must read.
        let mut log = ProbeLog::default();
        let start = Instant::now();
        assert_eq!(log.observe(probe(800.0).key(), start), Some(false));
        let resting = probe(700.0).key();
        assert_eq!(
            log.observe(resting, start + Duration::from_millis(100)),
            None,
            "throttled"
        );
        assert_eq!(
            log.observe(resting, start + Duration::from_millis(399)),
            None,
            "300 ms has not passed since the change at +100 ms"
        );
        assert_eq!(
            log.observe(resting, start + Duration::from_millis(401)),
            Some(true),
            "the last change is still reported, as the resting line"
        );
    }

    #[test]
    fn a_settle_is_owed_until_it_is_written() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        let key = probe(800.0).key();
        assert_eq!(log.pending(), None, "nothing to log before Settings draws");
        log.observe(key, start);
        assert_eq!(
            log.pending(),
            Some(start + PROBE_SETTLE),
            "the shell must wake itself for the resting line"
        );
        log.observe(key, start + Duration::from_millis(301));
        assert_eq!(log.pending(), None, "the line is written; no timer left");
    }

    #[test]
    fn leaving_and_returning_to_settings_logs_again() {
        let mut log = ProbeLog::default();
        let start = Instant::now();
        let key = probe(800.0).key();
        assert_eq!(log.observe(key, start), Some(false));
        log.reset();
        assert_eq!(
            log.observe(key, start + Duration::from_secs(5)),
            Some(false),
            "a fresh visit is a fresh line, even at the same scroll offset"
        );
    }
}
