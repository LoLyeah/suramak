//! The one dropdown (M14e) — a trigger plus a **sheet with a real backdrop**.
//!
//! The owner's report ("the drop down not blurring the bg so it cluttered") was
//! exact: egui's stock `ComboBox` popup is a bare translucent frame, and on the
//! macOS ladder its fill is a glass wash, so the Settings rows behind it bled
//! through the list. This module replaces that popup with:
//!
//! 1. a **scrim** over the page (the whole window dimmed by `tokens.scrim`),
//! 2. an **opaque** sheet (`tokens.sheet` — #fff / #1c1c1e family; never the
//!    translucent card wash) with the card shadow and a hairline,
//! 3. quiet rows with a hover wash, the selected row tinted and marked.
//!
//! egui cannot blur, so the scrim is a dim rather than a frosted backdrop — the
//! honest ceiling here (a real material would need an `NSVisualEffectView`
//! behind the sheet, the `suramak-glass` family, which is out of this slice).
//!
//! The geometry ([`sheet_rect`]) is pure and unit-tested; the widget only paints
//! and reports the pick.

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Stroke, StrokeKind,
};

use crate::motion::{self, Motion};
use crate::theme::Tokens;

use super::controls::{self, Tier};

/// One row's height in the sheet.
pub const ITEM_HEIGHT: f32 = 26.0;
/// The sheet's inner padding (top/bottom and sides).
pub const SHEET_PAD: f32 = 5.0;
/// Gap between the trigger and the sheet.
pub const SHEET_GAP: f32 = 4.0;

/// Where a dropdown's sheet lands: under the trigger when there is room, above
/// it otherwise, always inside `viewport` and never narrower than the trigger.
///
/// Pure — the frame-level test pins the placement rule without an egui pass.
pub fn sheet_rect(anchor: Rect, viewport: Rect, item_count: usize, width: f32) -> Rect {
    // A list taller than the window is clamped to it (the sheet never spills
    // past the viewport, either edge).
    let height =
        (SHEET_PAD * 2.0 + ITEM_HEIGHT * item_count.max(1) as f32).min(viewport.height().max(1.0));
    let width = width.max(anchor.width()).min(viewport.width().max(1.0));
    let mut x = anchor.left();
    if x + width > viewport.right() {
        x = viewport.right() - width;
    }
    x = x.max(viewport.left());
    let below = anchor.bottom() + SHEET_GAP;
    let y = if below + height <= viewport.bottom() {
        below
    } else {
        (anchor.top() - SHEET_GAP - height).max(viewport.top())
    };
    Rect::from_min_size(pos2(x, y), vec2(width, height))
}

/// The sheet's width for `labels`: the widest label plus padding, floored at
/// the trigger's width (the caller may widen it further).
pub fn sheet_width(ui: &egui::Ui, labels: &[String]) -> f32 {
    let font = FontId::new(12.0, FontFamily::Proportional);
    labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(label.clone(), font.clone(), Color32::PLACEHOLDER)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        + SHEET_PAD * 2.0
        + 28.0
}

/// A dropdown control: the trigger (a secondary button) plus, while open, the
/// scrim + sheet. Returns the newly chosen value (`None` when nothing changed).
///
/// `options` are `(value, label)` pairs; `selected` is the current value's
/// label. The open state lives in egui's temp data (UI-only, never persisted).
#[allow(clippy::too_many_arguments)]
pub fn dropdown(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    id_salt: impl std::hash::Hash,
    selected_value: &str,
    selected_label: &str,
    options: &[(String, String)],
    width: f32,
) -> Option<String> {
    let ctx = ui.ctx().clone();
    let id = ui.id().with(&id_salt);
    let open_id = id.with("open");
    let mut open = ctx.data(|data| data.get_temp::<bool>(open_id).unwrap_or(false));

    let trigger = controls::button_sized(
        ui,
        tokens,
        motion,
        id.with("trigger"),
        &format!("{selected_label}   ▾"),
        Tier::Secondary,
        true,
        vec2(width, 26.0),
    );
    if trigger.clicked() {
        open = !open;
        ctx.data_mut(|data| data.insert_temp(open_id, open));
    }
    // The trigger is a combo box, not a plain button (screen readers).
    trigger.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, selected_label)
    });

    if !open {
        return None;
    }

    let viewport = ctx.viewport_rect();
    let labels: Vec<String> = options.iter().map(|(_, label)| label.clone()).collect();
    let sheet = sheet_rect(
        trigger.rect,
        viewport,
        options.len(),
        sheet_width(ui, &labels).max(width),
    );

    // ---- backdrop -----------------------------------------------------------
    // A full-window scrim: the page behind the sheet is dimmed, so the list
    // reads as a sheet rather than as text floating over other text. Its click
    // is absorbed (and closes the sheet) — nothing behind it can be hit.
    let scrim = egui::Area::new(id.with("scrim"))
        .order(egui::Order::Middle)
        .fixed_pos(viewport.min)
        .movable(false)
        .constrain(false)
        .fade_in(false)
        .show(&ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(viewport.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, tokens.scrim);
            response
        });
    let dismissed = scrim.inner.clicked();

    // ---- sheet --------------------------------------------------------------
    let mut chosen: Option<String> = None;
    egui::Area::new(id.with("sheet"))
        .order(egui::Order::Foreground)
        .fixed_pos(sheet.min)
        .movable(false)
        .constrain(false)
        .fade_in(false)
        .show(&ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(sheet.size(), egui::Sense::hover());
            ui.painter().add(
                tokens
                    .card_shadow
                    .as_shape(rect, CornerRadius::same(tokens.radius.card)),
            );
            ui.painter()
                .rect_filled(rect, CornerRadius::same(tokens.radius.card), tokens.sheet);
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(tokens.radius.card),
                Stroke::new(tokens.stroke.hairline, tokens.hairline),
                StrokeKind::Inside,
            );

            let mut y = rect.top() + SHEET_PAD;
            for (value, label) in options {
                let row = Rect::from_min_size(
                    pos2(rect.left() + SHEET_PAD, y),
                    vec2(rect.width() - SHEET_PAD * 2.0, ITEM_HEIGHT),
                );
                let response =
                    ui.interact(row, id.with(("row", value.as_str())), egui::Sense::click());
                let selected_row = value == selected_value;
                let hover = motion.factor(
                    &ctx,
                    id.with(("row-hover", value.as_str())),
                    response.hovered(),
                    motion::FAST,
                );
                if hover > 0.01 {
                    ui.painter().rect_filled(
                        row,
                        CornerRadius::same(tokens.radius.row),
                        motion::lerp_color(Color32::TRANSPARENT, tokens.surface_hover, hover),
                    );
                }
                if selected_row {
                    ui.painter().rect_filled(
                        row,
                        CornerRadius::same(tokens.radius.row),
                        tokens.selection_bg,
                    );
                }
                ui.painter().text(
                    pos2(row.left() + 8.0, row.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::new(12.0, FontFamily::Proportional),
                    tokens.ink,
                );
                if selected_row {
                    // A painted marker, not a glyph: the bundled fonts have no
                    // check mark (a "✓" painted as nothing — the visual pass
                    // caught it), and a 3 pt accent bar is the quieter cue.
                    let mark = Rect::from_min_size(
                        pos2(row.right() - 6.0, row.center().y - 7.0),
                        vec2(3.0, 14.0),
                    );
                    ui.painter()
                        .rect_filled(mark, CornerRadius::same(2), tokens.accent);
                }
                if response.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() {
                    chosen = Some(value.clone());
                }
                y += ITEM_HEIGHT;
            }
        });

    if let Some(value) = &chosen {
        ctx.data_mut(|data| data.insert_temp(open_id, false));
        return Some(value.clone());
    }
    // A click on the scrim or Esc closes without a choice.
    if dismissed || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|data| data.insert_temp(open_id, false));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one frame with the dropdown, feeding `events`, and returns the
    /// painted shapes.
    fn frame(
        ctx: &egui::Context,
        tokens: &Tokens,
        motion: &Motion,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            events,
            ..Default::default()
        };
        let options = vec![
            ("auto".to_string(), "Auto (best installed)".to_string()),
            ("small".to_string(), "Small (fast)".to_string()),
        ];
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    ui.add_space(10.0);
                    let _ = dropdown(
                        ui,
                        tokens,
                        motion,
                        "test-dropdown",
                        "small",
                        "Small (fast)",
                        &options,
                        220.0,
                    );
                });
        })
    }

    /// M14e (the owner's dropdown finding): opening the list paints a
    /// **full-window scrim** behind an **opaque sheet** — the page no longer
    /// bleeds through the options. The sheet is the `sheet` token (never the
    /// translucent card wash) and covers the trigger's width.
    #[test]
    fn the_open_sheet_paints_a_scrim_and_an_opaque_panel() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let tokens = crate::theme::tokens(crate::theme::AppTheme::Dark);
        let motion = Motion::new(false);

        let _ = frame(&ctx, &tokens, &motion, 0.0, vec![]);
        // Click the trigger (the first widget in the panel, at ~(8,10)).
        let at = pos2(60.0, 22.0);
        let _ = frame(
            &ctx,
            &tokens,
            &motion,
            0.05,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let _ = frame(
            &ctx,
            &tokens,
            &motion,
            0.10,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        let output = frame(&ctx, &tokens, &motion, 0.20, vec![]);

        let mut scrim: Option<Rect> = None;
        let mut sheet: Option<Rect> = None;
        let mut marker: Option<Rect> = None;
        for clipped in &output.shapes {
            if let egui::Shape::Rect(rect) = &clipped.shape {
                if rect.fill == tokens.scrim && rect.rect.width() >= 899.0 {
                    scrim = Some(rect.rect);
                }
                if rect.fill == tokens.sheet {
                    sheet = Some(rect.rect);
                }
                // The selected row's marker: a narrow accent bar (the fonts
                // have no check mark — this is painted, not typed).
                if rect.fill == tokens.accent && rect.rect.width() <= 4.0 {
                    marker = Some(rect.rect);
                }
            }
        }
        let scrim = scrim.expect("an open dropdown must dim the page behind it");
        assert!(
            scrim.width() >= 899.0 && scrim.height() >= 599.0,
            "the scrim covers the whole window: {scrim:?}"
        );
        let sheet = sheet.expect("the sheet paints with the opaque sheet token");
        assert!(
            sheet.width() >= 220.0,
            "the sheet is at least the trigger's width: {sheet:?}"
        );
        assert!(
            scrim.contains_rect(sheet),
            "the sheet sits inside its scrim: {sheet:?} vs {scrim:?}"
        );
        // The options paint above the sheet.
        let labels: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(
            labels.iter().any(|label| label == "Auto (best installed)"),
            "the options paint in the sheet: {labels:?}"
        );
        // The selected option (value "small") carries its painted marker, and
        // the marker sits inside the sheet.
        let marker = marker.expect("the selected row carries its marker");
        assert!(
            sheet.contains_rect(marker),
            "the selection marker sits inside the sheet: {marker:?} vs {sheet:?}"
        );
    }

    fn viewport() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0))
    }

    #[test]
    fn the_sheet_hangs_under_the_trigger_and_covers_its_width() {
        let anchor = Rect::from_min_size(pos2(120.0, 200.0), vec2(220.0, 26.0));
        let sheet = sheet_rect(anchor, viewport(), 3, 180.0);
        assert_eq!(sheet.top(), anchor.bottom() + SHEET_GAP);
        assert_eq!(sheet.left(), anchor.left());
        assert_eq!(
            sheet.width(),
            220.0,
            "never narrower than the trigger it hangs from"
        );
        assert_eq!(
            sheet.height(),
            SHEET_PAD * 2.0 + ITEM_HEIGHT * 3.0,
            "one row per option plus the padding"
        );
        // …and the sheet stays inside the viewport it was given.
        assert!(viewport().contains_rect(sheet));
    }

    #[test]
    fn the_sheet_flips_above_the_trigger_when_there_is_no_room_below() {
        let anchor = Rect::from_min_size(pos2(100.0, 560.0), vec2(200.0, 26.0));
        let sheet = sheet_rect(anchor, viewport(), 6, 200.0);
        assert_eq!(
            sheet.bottom(),
            anchor.top() - SHEET_GAP,
            "a sheet that cannot fit below flips above"
        );
        assert!(viewport().contains_rect(sheet));
    }

    #[test]
    fn the_sheet_is_pushed_inside_the_viewport_on_the_right_and_top() {
        // A trigger near the right edge: the sheet slides left to fit.
        let anchor = Rect::from_min_size(pos2(840.0, 40.0), vec2(120.0, 26.0));
        let sheet = sheet_rect(anchor, viewport(), 2, 240.0);
        assert!(sheet.right() <= viewport().right() + 0.01);
        // A trigger flush with the top and a list taller than the window: the
        // sheet is clamped to the viewport instead of spilling out of it.
        let anchor = Rect::from_min_size(pos2(0.0, 2.0), vec2(120.0, 26.0));
        let sheet = sheet_rect(anchor, viewport(), 40, 200.0);
        assert!(sheet.top() >= viewport().top());
        assert!(
            sheet.bottom() <= viewport().bottom() + 0.01,
            "a tall list must not spill past the viewport: {sheet:?}"
        );
        assert_eq!(sheet.height(), viewport().height());
    }
}
