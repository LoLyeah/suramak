//! M16I — the shared speaker surface: the row badge and the 8 px lane.
//!
//! Both shells draw these; the desktop editor and the compact editor call the
//! same functions, so a phone build inherits the treatment without its own
//! code. The design is the M16H restrained form: an 18 × 18 chip preceded by a
//! 3 px colour bar on each segment row, and ONE 8 px strip above the waveform.
//! No timeline editor, no per-word colours, no speaker column.
//!
//! Colour is never the only channel: every badge carries a numeral (or the
//! initial of a renamed speaker), and the lane's bands are separated by
//! hairlines. The palette's six slots are token values asserted by the theme
//! contrast test on both ladders.

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Stroke,
    StrokeKind, Ui,
};

use suramak_core::diarization::{badge_text, Speaker, SpeakerTurn};

use super::theme::Tokens;

/// The badge's chip size.
pub const BADGE: f32 = 18.0;
/// The colour bar's width (the badge's leading edge).
pub const BAR_W: f32 = 3.0;
/// The gap between the bar and the chip.
pub const BAR_GAP: f32 = 4.0;
/// The row slot the badge occupies (bar + gap + chip).
pub const SLOT_W: f32 = BAR_W + BAR_GAP + BADGE;
/// The gap between the badge slot and the timecode gutter.
pub const SLOT_GAP: f32 = 6.0;
/// The lane's height — the one thin strip.
pub const LANE_H: f32 = 8.0;
/// The lane → waveform gap.
pub const LANE_GAP: f32 = 4.0;
/// The band alpha (the M16H 30 %).
pub const BAND_ALPHA: f32 = 0.30;
/// The chip fill alpha over the card (the M16H 12 %).
pub const CHIP_ALPHA: f32 = 0.12;

/// A speaker's slot colour on the current ladder.
pub fn color(tokens: &Tokens, slot: u8) -> Color32 {
    tokens.speaker[(slot as usize) % tokens.speaker.len()]
}

/// The 12 % chip wash for a slot (translucent, so it sits over the row).
pub fn chip_fill(tokens: &Tokens, slot: u8) -> Color32 {
    let c = color(tokens, slot);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (CHIP_ALPHA * 255.0) as u8)
}

/// The 30 % lane band for a slot.
pub fn band_fill(tokens: &Tokens, slot: u8) -> Color32 {
    let c = color(tokens, slot);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (BAND_ALPHA * 255.0) as u8)
}

/// The badge rect inside `slot` (the same geometry for drawing and hit-testing).
pub fn badge_rect(slot: Rect) -> Rect {
    Rect::from_min_size(
        pos2(slot.left() + BAR_W + BAR_GAP, slot.center().y - BADGE / 2.0),
        vec2(BADGE, BADGE),
    )
}

/// The bar rect inside `slot`.
pub fn bar_rect(slot: Rect) -> Rect {
    Rect::from_min_size(
        pos2(slot.left(), slot.center().y - BADGE / 2.0),
        vec2(BAR_W, BADGE),
    )
}

/// What one badge shows. Grouped so the draw call stays a five-argument
/// function (clippy's budget) and the caller names each value.
#[derive(Debug, Clone, Copy)]
pub struct BadgeLook<'a> {
    /// The speaker's stored label (`"Speaker 2"` or a user name); the chip
    /// shows its numeral/initial.
    pub label: &'a str,
    /// The speaker's palette slot.
    pub color_slot: u8,
    /// The second speaker of a straddling row (the split marker).
    pub secondary: Option<u8>,
    /// 1 = current, 0.55 = the stale view (the numeral stays legible).
    pub dim: f32,
}

/// Draws one badge into `slot` and returns its interaction response.
///
/// The focus ring is drawn whenever the control has keyboard focus (keyboard
/// use is first-class); the semantic label is `"<name> — click to rename"`.
pub fn badge(
    ui: &mut Ui,
    tokens: &Tokens,
    id: impl std::hash::Hash,
    slot: Rect,
    look: BadgeLook<'_>,
) -> Response {
    let BadgeLook {
        label,
        color_slot,
        secondary,
        dim,
    } = look;
    let response = ui.interact(slot, egui::Id::new(("speaker-badge", id)), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("{label} — click to rename"),
        )
    });
    let alpha = dim.clamp(0.0, 1.0);
    let painter = ui.painter();
    let bar = bar_rect(slot);
    painter.rect_filled(
        bar,
        CornerRadius::same(1),
        color(tokens, color_slot).gamma_multiply(alpha),
    );
    if let Some(second) = secondary {
        // The split marker: the second speaker's 3 px half-bar below the first.
        let half = Rect::from_min_size(pos2(bar.left(), bar.center().y), vec2(BAR_W, BADGE / 2.0));
        painter.rect_filled(
            half,
            CornerRadius::same(1),
            color(tokens, second).gamma_multiply(alpha),
        );
    }
    let chip = badge_rect(slot);
    painter.rect_filled(
        chip,
        CornerRadius::same(5),
        chip_fill(tokens, color_slot).gamma_multiply(alpha),
    );
    painter.text(
        chip.center(),
        Align2::CENTER_CENTER,
        badge_text(label),
        FontId::new(10.5, FontFamily::Proportional),
        tokens.ink.gamma_multiply(alpha),
    );
    if response.hovered() {
        painter.rect_stroke(
            chip,
            CornerRadius::same(5),
            Stroke::new(tokens.stroke.hairline, tokens.hairline_strong),
            StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    if response.has_focus() {
        painter.rect_stroke(
            slot.expand(1.5),
            CornerRadius::same(tokens.radius.control),
            Stroke::new(tokens.stroke.focus, tokens.accent),
            StrokeKind::Outside,
        );
    }
    response
}

/// The row's badge slot: `None` when the transcript has no run (no badge, no
/// reserved width — the plain view is unchanged).
pub fn row_slot(has_run: bool, left: f32, center_y: f32) -> Option<Rect> {
    has_run.then(|| Rect::from_min_size(pos2(left, center_y - BADGE / 2.0), vec2(SLOT_W, BADGE)))
}

/// Where the lane sits inside the waveform band: one 8 px strip at the top.
/// The waveform draws below it (`lane_bottom + LANE_GAP`).
pub fn lane_rect(band: Rect) -> Rect {
    Rect::from_min_size(band.min, vec2(band.width(), LANE_H))
}

/// The lane's two opacity inputs: the stale dim and the appear fade.
#[derive(Debug, Clone, Copy)]
pub struct LaneStyle {
    pub dim: f32,
    pub fade: f32,
}

impl Default for LaneStyle {
    fn default() -> Self {
        Self {
            dim: 1.0,
            fade: 1.0,
        }
    }
}

/// Draws the lane from the time-anchored turns (never a segment join).
///
/// `total_ms` is the audio length; a zero/unknown length draws nothing. Bands
/// are 30 % slot colours with 1 px separators; two turns that overlap in time
/// draw as two 4 px half-height bands (never a blended colour). `style.dim`
/// mutes the whole strip for the stale view.
pub fn lane(
    ui: &Ui,
    tokens: &Tokens,
    rect: Rect,
    speakers: &[Speaker],
    turns: &[SpeakerTurn],
    total_ms: i64,
    style: LaneStyle,
) {
    if total_ms <= 0 || turns.is_empty() {
        return;
    }
    let painter = ui.painter();
    let total = total_ms as f32;
    let x_of = |ms: i64| rect.left() + rect.width() * (ms.max(0) as f32 / total).clamp(0.0, 1.0);
    let alpha = style.dim.clamp(0.0, 1.0) * style.fade.clamp(0.0, 1.0);
    if alpha <= 0.01 {
        return;
    }
    let slot_of = |speaker_id: u32| {
        speakers
            .iter()
            .find(|speaker| speaker.speaker_id == speaker_id)
            .map(|speaker| speaker.color_slot)
            .unwrap_or(0)
    };
    for (index, turn) in turns.iter().enumerate() {
        let left = x_of(turn.start_ms);
        let right = x_of(turn.end_ms);
        if right - left < 0.5 {
            continue;
        }
        // Overlap → half-height band. The first overlapping turn takes the top
        // half, the second the bottom half; a third would re-use the top, which
        // the segmentation model's 2-speaker powerset cannot produce.
        let overlapping: Vec<usize> = turns
            .iter()
            .enumerate()
            .filter(|(_, other)| other.start_ms < turn.end_ms && other.end_ms > turn.start_ms)
            .map(|(other_index, _)| other_index)
            .collect();
        let (top, bottom) = if overlapping.len() > 1 {
            let rank = overlapping
                .iter()
                .position(|other| *other == index)
                .unwrap_or(0);
            if rank % 2 == 0 {
                (rect.top(), rect.top() + LANE_H / 2.0 - 0.5)
            } else {
                (rect.top() + LANE_H / 2.0 + 0.5, rect.bottom())
            }
        } else {
            (rect.top(), rect.bottom())
        };
        let band = Rect::from_min_max(pos2(left, top), pos2(right, bottom));
        painter.rect_filled(
            band,
            CornerRadius::same(1),
            band_fill(tokens, slot_of(turn.speaker_id)).gamma_multiply(alpha),
        );
        // The 1 px separator between adjacent bands (colour is not the only
        // channel, and a band edge must read even at 30 %).
        painter.vline(
            right,
            band.y_range(),
            Stroke::new(1.0_f32, tokens.panel.gamma_multiply(alpha)),
        );
    }
    // A11y: the lane is decorative-but-labelled. The whole strip is one focus
    // target whose spoken label names the speakers and the turn count.
    let names: Vec<String> = speakers
        .iter()
        .map(|speaker| speaker.label.clone())
        .collect();
    ui.ctx()
        .accesskit_node_builder(egui::Id::new("speaker-lane"), |builder| {
            builder.set_label(format!(
                "Speaker lane: {} speaking turns ({})",
                turns.len(),
                names.join(", ")
            ));
            builder.set_role(egui::accesskit::Role::Image);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::diarization::{Speaker, SpeakerTurn};

    #[test]
    fn the_badge_is_a_bar_and_an_eighteen_pixel_chip_inside_its_slot() {
        let slot = Rect::from_min_size(pos2(10.0, 20.0), vec2(SLOT_W, BADGE));
        let bar = bar_rect(slot);
        let chip = badge_rect(slot);
        assert_eq!(bar.width(), BAR_W);
        assert_eq!(bar.height(), BADGE);
        assert_eq!(chip.width(), BADGE);
        assert_eq!(chip.height(), BADGE);
        assert_eq!(bar.left(), slot.left());
        assert_eq!(chip.left(), bar.right() + BAR_GAP);
        assert!(chip.right() <= slot.right());
    }

    #[test]
    fn the_slot_is_only_reserved_when_a_run_exists() {
        assert!(row_slot(false, 0.0, 10.0).is_none());
        let slot = row_slot(true, 0.0, 10.0).unwrap();
        assert_eq!(slot.width(), SLOT_W);
        assert_eq!(slot.center().y, 10.0);
    }

    #[test]
    fn the_lane_paints_one_band_per_turn_inside_its_eight_pixel_strip() {
        let ctx = egui::Context::default();
        let tokens = super::super::theme::tokens(super::super::theme::AppTheme::Light);
        let lane_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, LANE_H));
        let speakers = vec![Speaker::new(0, "Speaker 1", 0), Speaker::new(1, "Budi", 1)];
        let turns = vec![
            SpeakerTurn::new(0, 0, 2_000, 0),
            SpeakerTurn::new(1, 2_000, 4_000, 1),
        ];
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 40.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        ui.allocate_rect(lane_rect, Sense::hover());
                        lane(
                            ui,
                            &tokens,
                            lane_rect,
                            &speakers,
                            &turns,
                            4_000,
                            LaneStyle::default(),
                        );
                    });
            },
        );
        let bands: Vec<Rect> = output
            .shapes
            .iter()
            .filter_map(|clipped| {
                let bounds = clipped.shape.visual_bounding_rect();
                // The bands are the only 8 px-tall fills in this frame.
                (bounds.height() >= LANE_H - 0.5 && bounds.height() <= LANE_H + 0.5)
                    .then_some(bounds)
            })
            .collect();
        assert_eq!(bands.len(), 2, "one band per turn: {bands:?}");
        for band in bands {
            assert!(lane_rect.expand(0.6).contains_rect(band), "{band:?}");
        }
    }

    #[test]
    fn an_empty_lane_or_a_zero_length_audio_draws_nothing() {
        let ctx = egui::Context::default();
        let tokens = super::super::theme::tokens(super::super::theme::AppTheme::Light);
        let lane_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, LANE_H));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 40.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        lane(ui, &tokens, lane_rect, &[], &[], 0, LaneStyle::default());
                    });
            },
        );
        let fills = output
            .shapes
            .iter()
            .filter(|clipped| {
                let bounds = clipped.shape.visual_bounding_rect();
                (bounds.height() - LANE_H).abs() < 0.5 && bounds.width() > 1.0
            })
            .count();
        assert_eq!(fills, 0, "no turns means no bands");
    }
}
