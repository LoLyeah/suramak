//! Record route (M3, live transcript M7; M9a tokens/controls/motion): the live
//! recording view — auto title, elapsed clock, RMS meter, the rolling waveform
//! painter and the growing **provisional** transcript, all in the calm M1
//! composition (protect list: information order unchanged).
//!
//! The runtime owns the cpal session; this view reads [`AppState::record`] (fed
//! by the runtime's meter samples and live decodes) and only ever asks for
//! [`Effect::StopCapture`]. M9a: the cards wear the one card recipe, the Stop
//! action is a secondary painted control (the floating pill is the primary
//! record/stop affordance — no equal-weight duplicates), and each live line
//! fades+rises in as it arrives (base token; a snap under Reduce Motion).
//!
//! M7: the live transcript card is explicitly labeled provisional — the final
//! pass runs on Stop and replaces it; the card auto-scrolls (stick to bottom)
//! and surfaces the backlog note when the decoder had to skip windows.

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Painter, Rect, Stroke};

use suramak_core::transcript::Segment;

use crate::app::controls::{self, Tier};
use crate::motion::{self, Motion};
use crate::theme::Tokens;

use super::views::{paint_card, pane_caption, pane_title, PANE_PAD_X};
use super::{format_elapsed, AppState, Effect, RecordState, RecordingSource, RECORD_WAVEFORM_BARS};

/// M9d item 2: the living dot's opacity-breath period (seconds). Subtle and
/// slow — well above the 1 s floor, so it reads as "alive", never as a blink.
pub const DOT_BREATH_PERIOD: f32 = 1.6;

/// The living dot's opacity at `time` (seconds since app start): a subtle
/// breath between 0.7 and 1.0 over [`DOT_BREATH_PERIOD`]; `1.0` (static) under
/// Reduce Motion. Pure so the breath's range/period are pinned by a test.
pub fn dot_breath(time: f32, reduce: bool) -> f32 {
    if reduce {
        return 1.0;
    }
    let phase = std::f32::consts::TAU * time / DOT_BREATH_PERIOD;
    0.85 + 0.15 * phase.sin()
}

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

    match &state.record {
        RecordState::Recording(recording) => {
            let title = recording.title.clone();
            let elapsed = format_elapsed(recording.elapsed);
            let rms = recording.rms;
            let waveform = recording.waveform.clone();
            let device = recording.device.clone();
            let sources = recording.sources.clone();
            let provisional = recording.provisional.clone();
            let arrivals = recording.provisional_arrivals.clone();
            let provisional_skipped = recording.provisional_skipped;
            let stopping = recording.stopping;
            recording_body(
                ui,
                tokens,
                motion,
                state,
                effects,
                rect,
                left,
                right,
                &title,
                &elapsed,
                rms,
                &waveform,
                device.as_deref(),
                &sources,
                &provisional,
                &arrivals,
                provisional_skipped,
                stopping,
            );
        }
        RecordState::Failed { message } => {
            let message = message.clone();
            pane_title(ui, "Recording failed", tokens);
            // An error state: its text fades in on arrival (base; a snap under
            // Reduce Motion).
            super::views::arriving_caption(ui, tokens, motion, "record-failed", &message, true);
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                if controls::nav_home(ui, tokens, motion, "record-nav").clicked() {
                    effects.extend(state.leave_record());
                }
            });
        }
        RecordState::Idle => {
            // Reached only through a route change without a session; stay calm and
            // offer the way back rather than showing a dead screen.
            pane_title(ui, "Not recording", tokens);
            pane_caption(
                ui,
                "Press the floating Record control (bottom center) to record.",
                tokens,
                false,
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(PANE_PAD_X - 8.0);
                if controls::nav_home(ui, tokens, motion, "record-nav").clicked() {
                    effects.extend(state.back_home());
                }
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn recording_body(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    rect: Rect,
    left: f32,
    right: f32,
    title: &str,
    elapsed: &str,
    rms: f32,
    waveform: &[f32],
    device: Option<&str>,
    sources: &[RecordingSource],
    provisional: &[Segment],
    arrivals: &[std::time::Instant],
    provisional_skipped: u64,
    stopping: bool,
) {
    let painter = ui.painter().clone();

    ui.horizontal(|ui| {
        ui.add_space(PANE_PAD_X - 8.0);
        ui.vertical(|ui| {
            pane_title(ui, title, tokens);
            // The caption crossfades from the arming line to the live-sources
            // line the moment the runtime reports them (base token; a snap
            // under Reduce Motion) — the recording lifecycle's arming → live
            // transition, seen rather than teleported. Both lines paint into
            // the same preallocated row: the text measure never shifts.
            let (caption_row, _) = ui.allocate_exact_size(
                vec2(ui.available_width().max(120.0), 18.0),
                egui::Sense::hover(),
            );
            let device_f = motion.factor(
                ui.ctx(),
                egui::Id::new("suramak-record-device"),
                device.is_some(),
                motion::BASE,
            );
            let caption = match device {
                Some(device) => {
                    format!("recording from {device} — transcribed on this machine, never uploaded")
                }
                None => "starting capture…".to_string(),
            };
            let alpha = if device.is_some() { device_f } else { 1.0 };
            ui.painter().text(
                caption_row.left_center(),
                Align2::LEFT_CENTER,
                caption,
                FontId::new(12.0, FontFamily::Proportional),
                tokens.ink_soft.gamma_multiply(alpha),
            );
        });
    });

    // ---- metering card ------------------------------------------------------
    let card = Rect::from_min_size(pos2(left, rect.top() + 96.0), vec2(right - left, 176.0));
    paint_card(&painter, card, tokens);

    // Elapsed clock (mono, prominent but calm).
    painter.text(
        pos2(card.left() + 22.0, card.top() + 20.0),
        Align2::LEFT_TOP,
        elapsed,
        FontId::new(30.0, FontFamily::Monospace),
        tokens.ink,
    );
    // The status word crossfades with the recording state (and reads at ink,
    // not the near-invisible dim caption of the critique). M9d: the word reads
    // in the **text-safe** red (WCAG AA at 12 px on the light card).
    let stopping_f = motion.factor(
        ui.ctx(),
        egui::Id::new("suramak-record-stopping"),
        stopping,
        motion::BASE,
    );
    let status = if stopping_f > 0.5 {
        "finishing…"
    } else {
        "recording"
    };
    let status_color =
        motion::lerp_color(tokens.ink_soft, tokens.record_red_text, 1.0 - stopping_f);
    let status_galley = painter.layout_no_wrap(
        status.to_owned(),
        FontId::new(12.0, FontFamily::Monospace),
        status_color,
    );
    let status_left = card.right() - 22.0 - status_galley.size().x;
    let status_top = card.top() + 30.0;
    // M9d item 2 — the living dot (owner-confirmed: "a living dot + the real
    // level meter only"): a small red dot beside the status word with a subtle
    // opacity breath (0.7↔1.0 over a 1.6 s period — one-shot-friendly, and
    // nothing else pulses; the waveform stays honest). Reduce Motion: a static
    // dot. The breath samples the shared clock, so it rides the recording
    // cadence (100 ms) without asking for a single extra frame.
    let breath = dot_breath(ui.ctx().input(|input| input.time) as f32, motion.reduce);
    let dot_color = motion::lerp_color(tokens.record_red, tokens.ink_soft, stopping_f);
    painter.circle_filled(
        pos2(status_left - 9.0, status_top + 7.0),
        3.0,
        dot_color.gamma_multiply(breath),
    );
    painter.galley(pos2(status_left, status_top), status_galley, status_color);

    // RMS meter.
    let meter_label = pos2(card.left() + 22.0, card.top() + 66.0);
    painter.text(
        meter_label,
        Align2::LEFT_TOP,
        "level",
        FontId::new(11.0, FontFamily::Proportional),
        tokens.ink_soft,
    );
    let meter = Rect::from_min_size(
        pos2(card.left() + 22.0, card.top() + 82.0),
        vec2(card.width() - 44.0, 8.0),
    );
    painter.rect_filled(meter, CornerRadius::same(4), tokens.panel_faint);
    let fraction = rms.clamp(0.0, 1.0);
    if fraction > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(meter.min, vec2(meter.width() * fraction, meter.height())),
            CornerRadius::same(4),
            tokens.accent,
        );
    }

    // Rolling waveform.
    let wave = Rect::from_min_size(
        pos2(card.left() + 22.0, card.top() + 108.0),
        vec2(card.width() - 44.0, 46.0),
    );
    draw_waveform(&painter, wave, waveform, tokens);

    // ---- per-source rows (M6) -----------------------------------------------
    // One row per live source: label, mute, gain. Gain/mute are applied by the
    // mixing engine when the recording is finalised (documented: they apply to
    // the whole recording). Composition unchanged (protect list).
    let mut controls_top = card.bottom() + 18.0;
    if !sources.is_empty() {
        let rows = sources.len() as f32;
        let sources_card = Rect::from_min_size(
            pos2(left, card.bottom() + 14.0),
            vec2(right - left, 30.0 + rows * 26.0),
        );
        paint_card(&painter, sources_card, tokens);
        painter.text(
            pos2(sources_card.left() + 18.0, sources_card.top() + 9.0),
            Align2::LEFT_TOP,
            "sources — gain and mute are applied to the whole recording",
            FontId::new(11.0, FontFamily::Proportional),
            tokens.ink_soft,
        );
        for (index, source) in sources.iter().enumerate() {
            let row = Rect::from_min_size(
                pos2(
                    sources_card.left() + 18.0,
                    sources_card.top() + 28.0 + index as f32 * 26.0,
                ),
                vec2(sources_card.width() - 36.0, 24.0),
            );
            let mut gain = source.gain;
            let mut muted = source.muted;
            ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
                ui.horizontal(|ui| {
                    let label = ui.add_sized(
                        [220.0, 20.0],
                        egui::Label::new(
                            egui::RichText::new(&source.label)
                                .family(FontFamily::Monospace)
                                .size(11.5)
                                .color(tokens.ink),
                        )
                        .truncate(),
                    );
                    // M9d item 5 (review §7.1): the truncated row carries the
                    // full source label as its hover text.
                    label.on_hover_text(&source.label);
                    if ui
                        .add_enabled(!stopping, egui::Checkbox::new(&mut muted, "mute"))
                        .changed()
                    {
                        state.set_source_muted(index, muted);
                    }
                    let slider = egui::Slider::new(
                        &mut gain,
                        RecordingSource::MIN_GAIN..=RecordingSource::MAX_GAIN,
                    )
                    .fixed_decimals(2)
                    .suffix("×");
                    if ui.add_enabled(!stopping, slider).changed() {
                        state.set_source_gain(index, gain);
                    }
                });
            });
        }
        controls_top = sources_card.bottom() + 18.0;
    }

    // ---- controls -----------------------------------------------------------
    // Stop is the view's secondary action (the floating pill is the primary
    // record/stop affordance — the critique's equal-weight duplicate is gone).
    let controls = Rect::from_min_size(pos2(left, controls_top), vec2(right - left, 30.0));
    let mut stop_clicked = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(controls), |ui| {
        ui.horizontal(|ui| {
            // M9e (P1-1): Stop is disabled while the quit confirm is up — the
            // confirm is modal and a stray click must not finalize the very
            // recording the confirm is asking about.
            let stop = controls::button(
                ui,
                tokens,
                motion,
                "record-stop",
                "Stop",
                Tier::Secondary,
                !stopping && state.quit_prompt.is_none(),
            )
            .on_hover_text("Stop the recording and run the final transcription pass");
            if stop.clicked() {
                stop_clicked = true;
            }
            ui.add_space(10.0);
            // The meter line in plain words (M9c copy sweep: the old
            // "N of the last 96 meter samples" telemetry string is gone).
            pane_caption(ui, "the meter shows the last 10 seconds", tokens, false);
        });
    });
    if stop_clicked {
        effects.extend(state.stop_recording());
    }

    // Source-resolution warnings ("device not found — using the default") must be
    // visible while recording (Addendum A: fall back with a *visible* warning).
    // M9c: the line fades in base / out fast (called every frame, also empty).
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, controls_top + 38.0),
            vec2(right - left, 20.0),
        )),
        |ui| {
            ui.horizontal(|ui| {
                let message = state
                    .status
                    .as_ref()
                    .map(|status| (status.text.as_str(), status.error));
                super::views::status_caption(ui, tokens, motion, "record-status", message);
            });
        },
    );

    // ---- live provisional transcript (M7) ------------------------------------
    draw_provisional(
        ui,
        tokens,
        motion,
        left,
        right,
        rect,
        controls_top + 64.0,
        provisional,
        arrivals,
        provisional_skipped,
        stopping,
    );
}

/// The live transcript card: clearly labeled **provisional**, auto-scrolling as
/// words arrive (each line fades+rises on arrival), with the backlog note when
/// windows had to be skipped.
///
/// The final pass runs on Stop and replaces this list — the label says so.
#[allow(clippy::too_many_arguments)]
fn draw_provisional(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    left: f32,
    right: f32,
    rect: Rect,
    top: f32,
    provisional: &[Segment],
    arrivals: &[std::time::Instant],
    skipped: u64,
    stopping: bool,
) {
    let height = rect.bottom() - 14.0 - top;
    if height < 80.0 {
        // Too little room (small window): the rest of the view keeps working.
        return;
    }
    let painter = ui.painter().clone();
    let card = Rect::from_min_size(pos2(left, top), vec2(right - left, height));
    paint_card(&painter, card, tokens);

    // The clear provisional label (M7): what this list is, and what replaces it.
    painter.text(
        pos2(card.left() + 18.0, card.top() + 9.0),
        Align2::LEFT_TOP,
        "live transcript — provisional · the final pass runs on Stop",
        FontId::new(11.0, FontFamily::Proportional),
        tokens.ink_soft,
    );

    // The backlog note: subtle, only when the decoder had to skip windows.
    let mut list_top = card.top() + 26.0;
    if skipped > 0 {
        painter.text(
            pos2(card.left() + 18.0, card.top() + 24.0),
            Align2::LEFT_TOP,
            format!(
                "{} skipped to keep up — live updates may lag; the final pass is unaffected",
                super::count_noun(skipped as usize, "window")
            ),
            FontId::new(11.0, FontFamily::Proportional),
            tokens.ink_soft,
        );
        list_top = card.top() + 44.0;
    }

    let inner = Rect::from_min_max(
        pos2(card.left() + 18.0, list_top),
        pos2(card.right() - 18.0, card.bottom() - 10.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            // Auto-scroll: the list sticks to the newest words as they arrive
            // (the user can still scroll back; egui re-sticks at the bottom).
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if provisional.is_empty() {
                    // The stopping state's hint fades in (base token; a snap
                    // under Reduce Motion) — the finishing hand-off is seen.
                    let hint_f = motion.factor(
                        ui.ctx(),
                        egui::Id::new("suramak-record-hint"),
                        stopping,
                        motion::BASE,
                    );
                    let hint = if stopping {
                        "finishing the recording — the final pass starts next"
                    } else {
                        "words appear here while you speak"
                    };
                    let alpha = if stopping { hint_f } else { 1.0 };
                    ui.label(
                        egui::RichText::new(hint)
                            .size(12.0)
                            .color(tokens.ink_soft.gamma_multiply(alpha)),
                    );
                    return;
                }
                let wrap = ui.available_width().max(80.0);
                let now = std::time::Instant::now();
                let token = motion.duration(motion::BASE);
                for (index, segment) in provisional.iter().enumerate() {
                    // Each line fades + rises in once, timed from its arrival
                    // instant (base token; a snap under Reduce Motion). The
                    // layout width never changes — the rise is a paint-time
                    // translation only.
                    let arrival = arrivals.get(index).copied().unwrap_or(now);
                    let progress =
                        motion::arrival_progress(now.saturating_duration_since(arrival), token);
                    let galley = ui.painter().layout(
                        segment.text.clone(),
                        FontId::new(13.0, FontFamily::Proportional),
                        tokens.ink,
                        wrap,
                    );
                    ui.set_opacity(progress);
                    let (row, _) = ui.allocate_exact_size(
                        vec2(wrap, galley.size().y + 3.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().galley(
                        pos2(row.left(), row.top() + motion::rise(progress, 3.0)),
                        galley,
                        tokens.ink,
                    );
                    ui.set_opacity(1.0);
                }
            });
    });
}

/// Rolling waveform painter: one centered bar per meter sample (oldest first,
/// newest at the right edge), in the token waveform greys.
fn draw_waveform(painter: &Painter, rect: Rect, values: &[f32], tokens: &Tokens) {
    painter.hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(tokens.stroke.hairline, tokens.hairline),
    );
    if values.is_empty() {
        return;
    }
    let step = rect.width() / RECORD_WAVEFORM_BARS as f32;
    let bar_width = (step - 2.0).max(1.0);
    for (index, value) in values.iter().enumerate() {
        let offset_from_right = (values.len() - index) as f32;
        let x = rect.right() - step * offset_from_right + step / 2.0;
        let half = (value.clamp(0.0, 1.0) * rect.height() * 0.5).max(1.0);
        let bar = Rect::from_center_size(pos2(x, rect.center().y), vec2(bar_width, half * 2.0));
        painter.rect_filled(bar, CornerRadius::same(1), tokens.wave);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M9d item 2: the living dot's breath is subtle (0.7↔1.0), slow (period
    /// ≥ 1 s), periodic, and **static** under Reduce Motion. Nothing else in
    /// the record view pulses — the meter and the waveform are data.
    #[test]
    fn the_living_dot_breaths_subtly_and_slowly() {
        let period = DOT_BREATH_PERIOD;
        assert!(period >= 1.0, "the period clears the 1 s floor: {period}");
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for step in 0..=4000 {
            let value = dot_breath(step as f32 / 1000.0, false);
            min = min.min(value);
            max = max.max(value);
        }
        assert!(min >= 0.699, "the breath never dips under 0.7: {min}");
        assert!(max <= 1.0001, "the breath never exceeds 1.0: {max}");
        assert!(
            max - min > 0.25,
            "the breath is a real breath, not a flicker: {min}..{max}"
        );
        // Periodic: one period later the value is back.
        let at = 0.37;
        assert!((dot_breath(at, false) - dot_breath(at + DOT_BREATH_PERIOD, false)).abs() < 1e-3);
        // Reduce Motion: a static dot, at every instant.
        for step in 0..200 {
            assert_eq!(dot_breath(step as f32 * 0.05, true), 1.0);
        }
    }
}
