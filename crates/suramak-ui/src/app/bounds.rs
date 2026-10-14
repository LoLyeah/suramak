//! Painted-bounds audit (M9c) — the debug seam behind the invariant **no
//! painted control draws outside its allocated rect** (the focus ring is the
//! one sanctioned excess: its path sits at `rect.expand(FOCUS_RING_EXPAND)`
//! and, with `StrokeKind::Outside`, its ink reaches that path plus the focus
//! stroke's width).
//!
//! `controls.rs` records every control body it paints (id + allocated rect +
//! kind) while the audit is on — a test seam ([`set_enabled`]); production
//! builds never record. Tests then run a real frame and check every painted
//! shape against the recorded rects with [`violations`]. Independently of the
//! recording, `paint_glyph` / `button_sized` / `capsule_*` / `segmented`
//! carry hard `debug_assert`s (glyph ink, labels and padding must fit their
//! rects), so the invariant is enforced at the source on every debug build.

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
use egui::epaint::ClippedShape;
use egui::{Id, Rect};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether controls record their rects this run (the test seam below; a
/// production build never records — the `debug_assert`s still enforce the
/// invariant at every paint).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Test seam: force recording on/off.
#[cfg(test)]
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// One painted control body: its widget id, allocated rect and kind label.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlRect {
    pub id: Id,
    pub rect: Rect,
    pub kind: &'static str,
}

fn slot() -> Id {
    Id::new("suramak-bounds-audit")
}

/// Records one control body (no-op unless the audit is enabled).
pub fn record(ctx: &egui::Context, id: Id, rect: Rect, kind: &'static str) {
    if !enabled() {
        return;
    }
    ctx.data_mut(|data| {
        let mut list = data
            .get_temp::<Vec<ControlRect>>(slot())
            .unwrap_or_default();
        list.push(ControlRect { id, rect, kind });
        data.insert_temp(slot(), list);
    });
}

/// Drains the rects recorded since the last call (call after one frame).
#[cfg(test)]
pub fn take(ctx: &egui::Context) -> Vec<ControlRect> {
    ctx.data_mut(|data| {
        data.remove_temp::<Vec<ControlRect>>(slot())
            .unwrap_or_default()
    })
}

/// Every painted shape that no recorded control rect (expanded by `allow`)
/// contains — the audit's failure list, empty when the invariant holds.
#[cfg(test)]
pub fn violations(shapes: &[ClippedShape], records: &[ControlRect], allow: f32) -> Vec<String> {
    let mut out = Vec::new();
    for clipped in shapes {
        let bounds = clipped.shape.visual_bounding_rect();
        if bounds.is_negative() || !bounds.is_finite() {
            continue;
        }
        let contained = records
            .iter()
            .any(|record| record.rect.expand(allow).contains_rect(bounds));
        if !contained {
            out.push(format!(
                "shape {bounds:?} is outside every recorded control rect (+{allow} pt)"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::controls::{self, Glyph, Tier};
    use crate::motion::Motion;
    use crate::theme::{tokens, AppTheme};
    use egui::{pos2, vec2};

    /// The audit's `ENABLED` flag is process-global, so the two tests that
    /// drive it must never interleave (a parallel run had one test's teardown
    /// blank the other's recordings — the empty-list failure mode). Poisoning
    /// is ignored so one failure never cascades.
    static AUDIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn audit_serial() -> std::sync::MutexGuard<'static, ()> {
        AUDIT_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn showcase_frame(ctx: &egui::Context, tokens: &crate::theme::Tokens) -> egui::FullOutput {
        let motion = Motion::new(false);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0))),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    // One of every painted control kind, laid out with room to
                    // spare so a shape escaping its own rect cannot hide inside
                    // a neighbour's allowance.
                    ui.add_space(10.0);
                    let _ = controls::button(
                        ui,
                        tokens,
                        &motion,
                        "audit-secondary",
                        "Export…",
                        Tier::Secondary,
                        true,
                    );
                    ui.add_space(10.0);
                    let _ = controls::button(
                        ui,
                        tokens,
                        &motion,
                        "audit-capsule",
                        "Polish",
                        Tier::Capsule,
                        true,
                    );
                    ui.add_space(10.0);
                    let _ = controls::glyph_button(
                        ui,
                        tokens,
                        &motion,
                        "audit-trash",
                        Glyph::Trash,
                        Tier::Tertiary,
                        true,
                        "Delete this transcript",
                    );
                    ui.add_space(10.0);
                    let _ = controls::capsule_glyph_button(
                        ui,
                        tokens,
                        &motion,
                        "audit-capsule-trash",
                        Glyph::Trash,
                        true,
                        "Delete this transcript",
                    );
                    ui.add_space(10.0);
                    let _ = controls::glyph_label_button(
                        ui,
                        tokens,
                        &motion,
                        "audit-nav",
                        Glyph::ChevronLeft,
                        "Home",
                        true,
                    );
                    ui.add_space(10.0);
                    let mut selection = 0u8;
                    let options = [(0u8, "Markdown"), (1u8, "Text")];
                    let _ = controls::segmented(
                        ui,
                        tokens,
                        &motion,
                        "audit-seg",
                        &mut selection,
                        &options,
                    );
                    ui.add_space(10.0);
                    let _ =
                        controls::transport(ui, tokens, &motion, "audit-transport", false, true);
                });
        })
    }

    /// Every painted shape of a real control frame stays inside the rect its
    /// control allocated. This is the invariant the owner's title-line question
    /// asked for, swept over the whole control system.
    #[test]
    fn no_control_shape_escapes_its_allocated_rect() {
        let _serial = audit_serial();
        set_enabled(true);
        let ctx = egui::Context::default();
        let tokens = tokens(AppTheme::Light);
        let output = showcase_frame(&ctx, &tokens);
        let records = take(&ctx);
        set_enabled(false);
        assert!(
            records.len() >= 7,
            "the showcase recorded every control kind: {records:?}"
        );
        let found = violations(&output.shapes, &records, 0.01);
        assert!(found.is_empty(), "painted bounds violations: {found:#?}");
    }

    /// The focus ring is the one sanctioned excess: with focus requested it is
    /// the only paint outside a control's rect, and it stays within
    /// `rect.expand(FOCUS_RING_EXPAND + focus stroke)`.
    #[test]
    fn the_focus_ring_is_the_only_sanctioned_excess() {
        let _serial = audit_serial();
        set_enabled(true);
        let ctx = egui::Context::default();
        let tokens = tokens(AppTheme::Light);
        let _ = showcase_frame(&ctx, &tokens);
        let records = take(&ctx);
        // Focus the first recorded control (the secondary button) and repaint.
        let focused = records.first().expect("a recorded control").id;
        ctx.memory_mut(|memory| memory.request_focus(focused));
        let output = showcase_frame(&ctx, &tokens);
        let records = take(&ctx);
        set_enabled(false);
        // The ring paints outside the rect (proving this test can see it)…
        let strict = violations(&output.shapes, &records, 0.01);
        assert!(
            !strict.is_empty(),
            "the focused control must draw its ring outside its rect"
        );
        // …but never past the sanctioned path + stroke.
        let sanctioned = controls::FOCUS_RING_EXPAND + tokens.stroke.focus;
        let allowed = violations(&output.shapes, &records, sanctioned);
        assert!(
            allowed.is_empty(),
            "the ring escaped the sanctioned {sanctioned} pt: {allowed:#?}"
        );
    }
}
