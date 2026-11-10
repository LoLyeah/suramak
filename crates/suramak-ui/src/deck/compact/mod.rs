//! The compact phone profile: Library → editor → Settings, one pane at a time.
//!
//! This module is the **same Deck** the desktop draws from: [`theme`] tokens,
//! [`crate::deck::motion`] tokens and springs, and
//! [`crate::deck::controls`] painted controls. It adds no second visual
//! language — only the phone's navigation and layout (single column, 48 dp
//! targets, push/pop, sheets).
//!
//! The dispatcher below owns exactly three things: which screen is on top, the
//! in-flight push/pop spring, and the modal sheet layer. The screens own their
//! content; the host owns the store and the platform.

pub mod chrome;
pub mod editor;
pub mod library;
pub mod model;
pub mod settings;

use egui::{vec2, Rect, Ui, UiBuilder};

use crate::deck::motion::{self, Motion};
use crate::deck::theme::{self, AppTheme, Tokens};

pub use model::{
    bytes_label, clock, count_noun, sanitise_stem, snippet, CompactAction, CompactApp,
    CompactEditor, CompactExportFormat, CompactRoute, CompactSetting, CompactSettings, Insets,
    LibraryHit, LibraryRow, ModelRow, SegmentEdit, Sheet,
};

/// Paints one frame of the compact app and returns everything the host must do.
///
/// Call order per frame: advance the navigation spring (requesting a repaint
/// only while it is between endpoints), draw the active screen, then the sheet
/// layer. No idle frame work: once a transition settles the context returns to
/// on-demand repaints.
pub fn ui(ctx: &egui::Context, state: &mut CompactApp) -> Vec<CompactAction> {
    let now = ctx.input(|input| input.time);
    let animating = state.advance(now);
    if animating {
        ctx.request_repaint();
    }
    let tokens = theme::tokens(state.theme);
    let motion = Motion::new(state.reduce_motion);
    let screen = ctx.content_rect();

    let mut actions = Vec::new();
    let mut ui = Ui::new(
        ctx.clone(),
        egui::Id::new("suramak-compact-root"),
        UiBuilder::new()
            .max_rect(screen)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );

    // The navigation spring: the entering screen slides in from the right (push)
    // or the revealed screen slides back from the left (pop). Both are the same
    // spring; only the sign differs, so the two directions cannot drift apart.
    let (dx, opacity) = match state.nav.motion() {
        Some(anim) if !anim.is_done() => match anim.direction {
            crate::deck::nav::Direction::Push => (anim.offset(screen.width()), anim.opacity()),
            crate::deck::nav::Direction::Pop => (
                -anim.offset(screen.width()) * 0.18,
                0.55 + 0.45 * anim.opacity(),
            ),
        },
        _ => (0.0, 1.0),
    };

    let route = state.nav.current();
    let route_rect = screen.translate(vec2(dx, 0.0));
    let mut pane = ui.new_child(
        UiBuilder::new()
            .max_rect(route_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    pane.set_opacity(opacity);
    let mut route_actions = match route {
        CompactRoute::Library => library::ui(&mut pane, &tokens, &motion, state, route_rect),
        CompactRoute::Editor => editor::ui(&mut pane, &tokens, &motion, state, route_rect),
        CompactRoute::Settings => settings::ui(&mut pane, &tokens, &motion, state, route_rect),
    };
    actions.append(&mut route_actions);

    // The sheet layer rides above the screen and absorbs back first
    // (`CompactApp::back`).
    if let Some(sheet) = state.sheet {
        let sheet_rect = chrome::sheet(&mut ui, &tokens, screen);
        match sheet {
            Sheet::Export => {
                if let Some(action) =
                    editor::export_sheet(&mut ui, &tokens, &motion, state, sheet_rect)
                {
                    actions.push(action);
                }
            }
            Sheet::Quit => match editor::quit_sheet(&mut ui, &tokens, &motion, sheet_rect) {
                Some(true) => actions.push(state.confirm_quit()),
                Some(false) => state.sheet = None,
                None => {}
            },
        }
    }

    actions
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
    motion::BASE
}

/// Whether the dispatcher should keep a frame alive (the navigation spring).
pub fn wants_repaint(state: &CompactApp) -> bool {
    state.nav.motion().is_some_and(|anim| !anim.is_done())
}

/// A token ladder for an explicit appearance — used by the screenshot suite and
/// by hosts that have already resolved System.
pub fn tokens_for(theme: AppTheme) -> Tokens {
    theme::tokens(theme)
}
