//! The compact phone profile: Library → editor → Settings, one pane at a time.
//!
//! This module is the **same Deck** the desktop draws from: [`theme`] tokens,
//! [`crate::deck::motion`] tokens and springs, and
//! [`crate::deck::controls`] painted controls. It adds no second visual
//! language — only the phone's navigation and layout (single column, 48 dp
//! targets, push/pop, sheets).
//!
//! The dispatcher below owns exactly four things: which screen is on top, the
//! in-flight push/pop (two layers), the modal sheet layer, and the frame
//! budget. The screens own their content; the host owns the store and the
//! platform.
//!
//! **Frame budget (M13g2).** A frame is requested only while an animation is
//! between endpoints ([`CompactApp::advance`]) — never while a job runs and
//! nothing changed. A worker event wakes the surface once
//! ([`CompactApp::take_frame_request`]); a repeated poll with the same value is
//! not an event. This is what keeps a ten-minute transcription from redrawing
//! the screen at 60 Hz.

pub mod a11y;
pub mod chrome;
pub mod diagnostics;
pub mod editor;
pub mod first_run;
pub mod input;
pub mod library;
pub mod model;
pub mod motion;
pub mod onboarding;
pub mod settings;
pub mod speaker_sheets;
pub mod voice_note;

use egui::{vec2, Rect, Ui, UiBuilder};

use crate::deck::motion::Motion;
use crate::deck::theme::{self, AppTheme, Tokens};

pub use model::{
    bytes_label, clock, count_noun, row_key, sanitise_stem, snippet, word_at, CompactAction,
    CompactApp, CompactEditor, CompactExportFormat, CompactRoute, CompactSetting, CompactSettings,
    CompactSpeakers, CrashBanner, DiagnosticsView, DownloadState, DownloadView, ExportPhase,
    ExportSession, FlingProbe, Insets, JobView, LibraryFacets, LibraryHit, LibraryRow, ModelRow,
    ModelState, SegmentEdit, Sheet, SpeakerSheetState, TurnScope, TurnStripState, UndoToast,
    VoiceNoteView, UNDO_TOAST_SECS,
};

/// Paints one frame of the compact app and returns everything the host must do.
///
/// Call order per frame: advance every spring/fade (requesting a repaint only
/// while one is between endpoints), paint the outgoing layer, paint the
/// arriving layer, then the sheet layer above both. No idle frame work: once a
/// transition settles the context returns to on-demand repaints.
pub fn ui(ctx: &egui::Context, state: &mut CompactApp) -> Vec<CompactAction> {
    let now = ctx.input(|input| input.time);
    let animating = state.advance(now);
    if animating {
        ctx.request_repaint();
    }
    // M22B: an armed share offer opens on this frame, and its 1.2 s window is
    // the only idle reason to wake the surface — scheduled exactly once at the
    // deadline, not by a permanent frame request.
    state.begin_voice_note(now);
    if let Some(deadline) = state.voice_note_deadline() {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            (deadline - now).max(0.0),
        ));
    }
    // M23A: the Undo toast expires on the same mechanism — one scheduled wake
    // at its deadline, never a permanent frame request.
    if let Some(deadline) = state.undo_deadline() {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            (deadline - now).max(0.0),
        ));
    }
    let mut actions = Vec::new();
    if let Some(action) = state.take_voice_note_auto(now) {
        actions.push(action);
    }
    a11y::begin_frame(ctx);
    let tokens = theme::tokens(state.theme);
    let motion = Motion::new(state.reduce_motion);
    let screen = ctx.content_rect();

    let mut ui = Ui::new(
        ctx.clone(),
        egui::Id::new("suramak-compact-root"),
        UiBuilder::new()
            .max_rect(screen)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    // The window fill under every layer: during a push/pop the two panes do
    // not yet meet, and the exposed strip must be the window, not the
    // renderer's clear colour.
    ui.painter()
        .rect_filled(screen, egui::CornerRadius::ZERO, tokens.window);

    // The navigation: two layers while a transition is in flight (the screen
    // leaving beneath, the one arriving on top), one static layer otherwise.
    let in_flight = state.nav.motion().filter(|anim| !anim.is_done());
    if let Some(anim) = in_flight {
        let layers = anim.layers(screen.width());
        let (outgoing, incoming) = state.nav.layer_routes();
        if let (Some(route), Some(layer)) = (outgoing, layers.outgoing) {
            let mut pane = layer_ui(
                &mut ui,
                screen.translate(vec2(layer.dx, 0.0)),
                layer.alpha,
                0,
                false,
            );
            let mut layer_actions = draw_route(
                &mut pane,
                route,
                &tokens,
                &motion,
                state,
                screen.translate(vec2(layer.dx, 0.0)),
            );
            actions.append(&mut layer_actions);
        }
        let mut pane = layer_ui(
            &mut ui,
            screen.translate(vec2(layers.incoming.dx, 0.0)),
            layers.incoming.alpha,
            1,
            true,
        );
        let mut layer_actions = draw_route(
            &mut pane,
            incoming,
            &tokens,
            &motion,
            state,
            screen.translate(vec2(layers.incoming.dx, 0.0)),
        );
        actions.append(&mut layer_actions);
    } else {
        let route = state.nav.current();
        let mut pane = layer_ui(&mut ui, screen, 1.0, 1, true);
        let mut route_actions = draw_route(&mut pane, route, &tokens, &motion, state, screen);
        actions.append(&mut route_actions);
    }

    // The sheet layer rides above both screens and absorbs back first
    // (`CompactApp::back`). Its present/dismiss animation is the slide + scrim
    // pair; under Reduce Motion the travel is zero and only the fade runs.
    if let Some(sheet) = state.sheet {
        let fade = state.sheet_anim.value().max(0.001);
        // The plan's sheet motion is translateY(100% → 0): the body's slide is
        // the sheet's own present progress, and Reduce Motion removes the
        // travel (the fade is the whole transition then).
        let slide = if state.reduce_motion {
            1.0
        } else {
            state.sheet_anim.value()
        };
        let mut sheet_ui = layer_ui(&mut ui, screen, fade, 2, state.sheet_anim.is_present());
        let sheet_rect = if sheet == Sheet::VoiceNote {
            chrome::micro_sheet(&mut sheet_ui, &tokens, screen, slide)
        } else if sheet.is_speaker_sheet() {
            chrome::sheet_sized(
                &mut sheet_ui,
                &tokens,
                screen,
                slide,
                sheet.speaker_height(),
            )
        } else if sheet == Sheet::Export {
            chrome::sheet_sized(&mut sheet_ui, &tokens, screen, slide, 480.0)
        } else {
            chrome::sheet(&mut sheet_ui, &tokens, screen, slide)
        };
        match sheet {
            Sheet::Export => {
                actions.extend(editor::export_sheet(
                    &mut sheet_ui,
                    &tokens,
                    &motion,
                    state,
                    sheet_rect,
                ));
            }
            Sheet::Speakers => {
                actions.extend(speaker_sheets::speakers_sheet(
                    &mut sheet_ui,
                    &tokens,
                    &motion,
                    state,
                    sheet_rect,
                ));
            }
            Sheet::Speaker => {
                actions.extend(speaker_sheets::speaker_sheet(
                    &mut sheet_ui,
                    &tokens,
                    &motion,
                    state,
                    sheet_rect,
                ));
            }
            Sheet::TurnStrip => {
                actions.extend(speaker_sheets::turn_strip(
                    &mut sheet_ui,
                    &tokens,
                    state,
                    sheet_rect,
                ));
            }
            Sheet::Quit => match editor::quit_sheet(&mut sheet_ui, &tokens, &motion, sheet_rect) {
                Some(true) => actions.push(state.confirm_quit()),
                Some(false) => state.close_export_sheet(now),
                None => {}
            },
            Sheet::Onboarding => {
                let choice = onboarding::sheet(&mut sheet_ui, &tokens, &motion, sheet_rect);
                match choice {
                    onboarding::OnboardingChoice::Start => {
                        actions.push(state.finish_onboarding(now, false));
                    }
                    onboarding::OnboardingChoice::Skip => {
                        actions.push(state.finish_onboarding(now, true));
                    }
                    onboarding::OnboardingChoice::None => {}
                }
            }
            Sheet::VoiceNote => {
                let choice = state.voice_note.as_mut().and_then(|view| {
                    voice_note::sheet(&mut sheet_ui, &tokens, &motion, view, now, sheet_rect)
                });
                match choice {
                    Some(voice_note::VoiceNoteChoice::Accept) => {
                        let pill = state.voice_note.as_ref().and_then(|view| view.pill);
                        actions.push(state.accept_voice_note(pill, now));
                    }
                    Some(voice_note::VoiceNoteChoice::Dismiss) => {
                        state.dismiss_voice_note(now);
                        actions.push(CompactAction::VoiceNoteDismiss);
                    }
                    None => {}
                }
            }
        }
    }

    // M23A: the Undo toast is a snackbar — above the sheet layer, so Undo is
    // reachable while the Speakers sheet is still open and stays visible as
    // the sheet slides away.
    if state.undo_toast.is_some() {
        let bands = chrome::bands(
            screen,
            state.insets.top,
            state.insets.bottom,
            Some(chrome::BOTTOM_BAR),
        );
        actions.extend(speaker_sheets::undo_toast(
            &mut ui,
            &tokens,
            state,
            bands.bottom_bar,
        ));
    }

    actions
}

/// One painted layer: a child `Ui` clipped to the screen with an opacity and,
/// when it is not the active screen, interaction disabled.
fn layer_ui(ui: &mut Ui, rect: Rect, opacity: f32, salt: u8, interactive: bool) -> Ui {
    let mut pane = ui.new_child(
        UiBuilder::new()
            .id_salt(("compact-layer", salt))
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    pane.set_opacity(opacity);
    if !interactive {
        pane.disable();
    }
    pane
}

/// Dispatches one route. `rect` is the pane's screen-space rect (already
/// translated by the layer transform), which the screens use for their bands.
fn draw_route(
    pane: &mut Ui,
    route: CompactRoute,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut CompactApp,
    rect: Rect,
) -> Vec<CompactAction> {
    match route {
        CompactRoute::Library => library::ui(pane, tokens, motion, state, rect),
        CompactRoute::Editor => editor::ui(pane, tokens, motion, state, rect),
        CompactRoute::Settings => settings::ui(pane, tokens, motion, state, rect),
        CompactRoute::Diagnostics => diagnostics::ui(pane, tokens, motion, state, rect),
    }
}

/// The viewport helper used by the screenshot suite: a portrait phone viewport
/// at the density the M13 plan pins (1216 × 2640 physical, scale 2.6).
pub fn phone_viewport() -> (f32, f32, f32) {
    (1216.0, 2640.0, 2.6)
}

/// The logical screen rect for that physical viewport.
pub fn phone_screen_rect() -> Rect {
    let (w, h, scale) = phone_viewport();
    Rect::from_min_size(egui::Pos2::ZERO, vec2(w / scale, h / scale))
}

/// A secondary reference viewport (360 × 800 dp) — the plan's small-phone
/// profile, used by the hit-target tests.
pub fn small_phone_screen_rect() -> Rect {
    Rect::from_min_size(egui::Pos2::ZERO, vec2(360.0, 800.0))
}

/// The motion token the navigation transitions use (asserted by a test).
pub fn base_token() -> f32 {
    crate::deck::motion::BASE
}

/// Whether the dispatcher should keep a frame alive (any compact animation).
pub fn wants_repaint(state: &CompactApp) -> bool {
    state.needs_frame()
}

/// A token ladder for an explicit appearance — used by the screenshot suite and
/// by hosts that have already resolved System.
pub fn tokens_for(theme: AppTheme) -> Tokens {
    theme::tokens(theme)
}
