//! The compact surface's accessibility floor (M13g2).
//!
//! Every tappable thing on the phone is a painted egui shape, not a platform
//! view, so the accessibility tree only knows what the code tells it. This
//! module is the one place that does the telling:
//!
//! * [`touch`] creates the interaction, attaches a semantic label/role to the
//!   response (screen readers + `AccessKit`), paints the Deck focus ring when
//!   the control has keyboard focus, and records the control in the frame's
//!   audit;
//! * [`begin_frame`] / [`controls`] expose that audit to the phone-viewport
//!   suite, which asserts the floor mechanically instead of by inspection.
//!
//! The focus ring is the same geometry as the shared control kit's
//! (`controls::FOCUS_RING_EXPAND` + `tokens.stroke.focus`), so a keyboard user
//! — or Switch Access, which drives focus — sees one ring language.
//!
//! Keyboard reach on a phone is not the point (there is no Tab key); the
//! semantics are. Android's TalkBack reads `WidgetInfo`; Switch Access drives
//! the same focus state the ring paints.

use egui::{Context, Rect, Response, Sense, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType};

use crate::deck::controls::FOCUS_RING_EXPAND;
use crate::deck::theme::Tokens;

/// One interactive control as the frame saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub label: String,
    pub role: WidgetType,
    pub rect: Rect,
    pub enabled: bool,
}

/// The per-frame registry.
#[derive(Debug, Clone, Default)]
pub struct Audit {
    pub controls: Vec<Control>,
}

fn slot() -> egui::Id {
    egui::Id::new("suramak-compact-a11y-audit")
}

/// Clears the registry at the top of a frame.
pub fn begin_frame(ctx: &Context) {
    ctx.data_mut(|data| data.insert_temp(slot(), Audit::default()));
}

/// Records one control (used by painted buttons the shared kit owns).
pub fn record(ctx: &Context, rect: Rect, role: WidgetType, label: &str, enabled: bool) {
    ctx.data_mut(|data| {
        let mut audit = data.get_temp::<Audit>(slot()).unwrap_or_default();
        audit.controls.push(Control {
            label: label.to_string(),
            role,
            rect,
            enabled,
        });
        data.insert_temp(slot(), audit);
    });
}

/// The controls recorded this frame (the suite's mechanical floor).
pub fn controls(ctx: &Context) -> Vec<Control> {
    ctx.data(|data| data.get_temp::<Audit>(slot()))
        .map(|audit| audit.controls)
        .unwrap_or_default()
}

/// Paints the Deck focus ring at `rect` when `response` has focus.
///
/// Returns whether the ring was drawn (the suite asserts it once).
pub fn focus_ring(
    ui: &Ui,
    tokens: &Tokens,
    rect: Rect,
    response: &Response,
    enabled: bool,
) -> bool {
    if !enabled || !response.has_focus() {
        return false;
    }
    ui.painter().rect_stroke(
        rect.expand(FOCUS_RING_EXPAND),
        egui::CornerRadius::same(tokens.radius.control.saturating_add(2)),
        Stroke::new(tokens.stroke.focus, tokens.accent),
        StrokeKind::Outside,
    );
    true
}

/// A labeled touch target: the one constructor every compact control uses.
///
/// `role` is the semantics TalkBack announces (`Button`, `Checkbox`,
/// `RadioButton`); `label` is its spoken name. The control is recorded for the
/// audit whether or not it is enabled, so a disabled row is still describable.
pub fn touch(
    ui: &mut Ui,
    tokens: &Tokens,
    rect: Rect,
    id: impl std::hash::Hash,
    role: WidgetType,
    label: &str,
    enabled: bool,
) -> Response {
    let response = ui.interact(rect, ui.id().with(id), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(role, enabled, label));
    focus_ring(ui, tokens, rect, &response, enabled);
    record(ui.ctx(), rect, role, label, enabled);
    response
}

/// A control whose body is drawn by the shared painted kit (which already sets
/// its `WidgetInfo` and focus ring): record it for the audit.
pub fn record_button(ui: &Ui, rect: Rect, label: &str, enabled: bool) {
    record(ui.ctx(), rect, WidgetType::Button, label, enabled);
}

/// A label for a painted, non-interactive read-out (progress, meta lines). It
/// is not a control; TalkBack reads it from the parent surface's description.
pub fn describe(ui: &Ui, rect: Rect, label: &str) {
    record(ui.ctx(), rect, WidgetType::Label, label, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::theme::{self, AppTheme};

    fn ctx() -> Context {
        let ctx = Context::default();
        theme::install_fonts(&ctx);
        theme::install(&ctx, AppTheme::Dark, false);
        ctx
    }

    #[test]
    fn a_touch_no_record_is_not_a_control() {
        let ctx = ctx();
        begin_frame(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |_| {});
        });
        assert!(controls(&ctx).is_empty());
    }

    #[test]
    fn a_labeled_touch_is_recorded_and_painted() {
        let ctx = ctx();
        let tokens = theme::tokens(AppTheme::Dark);
        begin_frame(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(120.0, 48.0));
                let response = touch(
                    ui,
                    &tokens,
                    rect,
                    "unit",
                    WidgetType::Button,
                    "Open transcript",
                    true,
                );
                assert!(!response.has_focus(), "nothing focused it");
            });
        });
        let controls = controls(&ctx);
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].label, "Open transcript");
        assert_eq!(controls[0].role, WidgetType::Button);
        assert!(controls[0].enabled);
    }

    #[test]
    fn a_focused_control_paints_the_deck_ring() {
        let ctx = ctx();
        let tokens = theme::tokens(AppTheme::Dark);
        let mut painted = false;
        begin_frame(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(120.0, 48.0));
                let response = touch(
                    ui,
                    &tokens,
                    rect,
                    "focus",
                    WidgetType::Button,
                    "Export",
                    true,
                );
                response.request_focus();
                painted = focus_ring(ui, &tokens, rect, &response, true);
            });
        });
        assert!(painted, "a focused control draws the ring");
    }
}
