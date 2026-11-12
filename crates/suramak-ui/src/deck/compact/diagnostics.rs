//! Settings → Diagnostics (M13g3, restyled in M20D): the M13b spike's four
//! checks and the v2.4 crash banner, on the same Card plane and 8 pt rhythm as
//! Settings — plus the build vocabulary that no longer belongs on the primary
//! screen (raw model ids, the diarization models, the platform triple).
//!
//! **Coverage is not reduced.** Every counter the spike's tabs exposed is still
//! on screen — the four check groups keep their evidence, and the crash banner
//! is the same `spike-last-crash.txt` the overlay and the share sheet read.
//! What changes is the shape: one grouped card for the checks (no four loose
//! cards), one card for the model/engine facts, no tabs and no animation
//! harness. The read-outs and the banner are what the release rotation test
//! needs, and they are reachable from Settings.
//!
//! The screen is a view. `Clear crash` and `Share log` are actions; the shell
//! owns the file and the share intent.

use egui::{
    pos2, vec2, Align, Align2, CornerRadius, FontId, Layout, Rect, ScrollArea, Stroke, StrokeKind,
    Ui, WidgetType,
};

use crate::deck::material::{self, Plane};
use crate::deck::motion::Motion;
use crate::deck::theme::Tokens;
use crate::deck::type_scale;

use super::a11y;
use super::chrome::{self, GUTTER};
use super::model::{CompactAction, CompactApp, ModelRow};

/// The number of check groups this screen draws. Settings quotes it in the
/// Diagnostics row, and the screen's row array is length-checked against it, so
/// the count cannot drift from what is actually here.
pub const CHECK_COUNT: usize = 4;

/// One line of the "Model files" card (M20D2): the raw manifest id support
/// needs, and the size + state of that file.
///
/// These ids left the primary Settings card in M20D2; Diagnostics is the only
/// surface that still spells them. [`model_file_lines`] is both the paint
/// source and the test path, so the two cannot drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFileLine {
    pub id: String,
    pub value: String,
}

/// One line per **installed** model: the raw id, its pinned file size, and the
/// state word the primary card shows in human terms.
pub fn model_file_lines(models: &[ModelRow], selected: &str) -> Vec<ModelFileLine> {
    models
        .iter()
        .filter(|row| row.installed)
        .map(|row| ModelFileLine {
            id: row.id.clone(),
            value: format!(
                "{} · {}",
                row.tier_line(),
                row.state(row.id == selected, None).label()
            ),
        })
        .collect()
}

/// The 8 pt grid's card padding (the same 16 as the screen gutter).
const PAD: f32 = 16.0;
/// One evidence row: an 11 pt CAPS label and a two-line caption.
const EVIDENCE_ROW: f32 = 72.0;
/// One fact row in the model/engine card.
const FACT_ROW: f32 = 56.0;
/// The gap between sections.
const SECTION_GAP: f32 = 24.0;

/// Draws the Diagnostics route into `screen`.
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
    let (back, _) = chrome::app_bar(ui, tokens, motion, bands.app_bar, "Diagnostics", true, None);
    if back {
        state.back(now);
    }
    let view = state.diagnostics.clone();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bands.content)
            .layout(Layout::top_down(Align::Min)),
    );
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let width = bands.content.width() - GUTTER * 2.0;
            let x = bands.content.left() + GUTTER;
            let mut y = bands.content.top() + 8.0;

            // The v2.4 banner first: whatever killed the last process is the
            // single most important line on this screen.
            if let Some(crash) = &view.crash {
                y = crash_banner(ui, tokens, x, y, width, crash, &mut actions);
                y += SECTION_GAP;
            }

            // ---- Checks: one grouped card, four evidence rows ------------
            let text_group = format!(
                "{} chars · {} IME updates · {} deletions",
                view.text_chars, view.ime_updates, view.deletions
            );
            let lifecycle = format!(
                "{} resumes · {} surface rebuilds · {} frames gated · {} after destroy · probe {} saved / {} restored",
                view.resumes,
                view.surface_recreates,
                view.frames_gated,
                view.frames_after_destroy,
                view.probe_saves,
                view.probe_restores
            );
            let motion_line = match view.avg_fps {
                Some(fps) => format!("{fps:.1} fps avg · {} frames", view.frames),
                None => format!("sampling… · {} frames", view.frames),
            };
            let insets = format!(
                "{} inset pushes · top {:.0} pt / bottom {:.0} pt · {} back",
                view.inset_updates, view.inset_top, view.inset_bottom, view.back_presses
            );
            let checks: [(&str, &str); CHECK_COUNT] = [
                ("Text input", &text_group),
                ("Lifecycle", &lifecycle),
                ("Motion", &motion_line),
                ("Insets & back", &insets),
            ];

            y = section_label(ui, tokens, x, y, "Checks");
            let card = Rect::from_min_size(
                pos2(x, y),
                vec2(width, EVIDENCE_ROW * CHECK_COUNT as f32),
            );
            material::frosted_panel(ui, tokens, card, tokens.radius.card, Plane::Card);
            for (index, (label, value)) in checks.iter().enumerate() {
                let row = Rect::from_min_size(
                    pos2(x, card.top() + index as f32 * EVIDENCE_ROW),
                    vec2(width, EVIDENCE_ROW),
                );
                ui.painter().text(
                    pos2(row.left() + PAD, row.top() + 13.0),
                    Align2::LEFT_TOP,
                    label.to_uppercase(),
                    type_scale::section_label().font,
                    tokens.ink_soft,
                );
                paint_wrapped(
                    ui,
                    pos2(row.left() + PAD, row.top() + 34.0),
                    value,
                    &type_scale::caption(),
                    tokens.ink,
                    row.width() - PAD * 2.0,
                    2,
                );
                a11y::describe(
                    ui,
                    Rect::from_min_size(
                        pos2(row.left() + PAD, row.top()),
                        vec2(row.width() - PAD * 2.0, EVIDENCE_ROW - 1.0),
                    ),
                    &format!("{label}: {value}"),
                );
                if index + 1 < checks.len() {
                    ui.painter().line_segment(
                        [
                            pos2(row.left() + PAD, row.bottom() - 0.5),
                            pos2(row.right() - PAD, row.bottom() - 0.5),
                        ],
                        Stroke::new(1.0_f32, tokens.hairline),
                    );
                }
            }
            y = card.bottom() + SECTION_GAP;

            // ---- Model files: the vocabulary the primary card hides ------
            // M20D2: the Settings card speaks human tier names only. Support
            // still needs the exact manifest ids, so Diagnostics keeps one
            // line per **installed** file — the ids live here and nowhere else
            // on the phone's primary surface.
            y = section_label(ui, tokens, x, y, "Model files");
            let files = model_file_lines(&state.models, &state.settings.model_id);
            let file_rows = files.len().max(1);
            let card = Rect::from_min_size(
                pos2(x, y),
                vec2(width, FACT_ROW * file_rows as f32),
            );
            material::frosted_panel(ui, tokens, card, tokens.radius.card, Plane::Card);
            if files.is_empty() {
                let line = "No model files on this phone.";
                let row = Rect::from_min_size(pos2(x, card.top()), vec2(width, FACT_ROW));
                ui.painter().text(
                    pos2(row.left() + PAD, row.center().y),
                    Align2::LEFT_CENTER,
                    line,
                    type_scale::caption().font,
                    tokens.ink_soft,
                );
                a11y::describe(ui, row, line);
            }
            for (index, line) in files.iter().enumerate() {
                let row = Rect::from_min_size(
                    pos2(x, card.top() + index as f32 * FACT_ROW),
                    vec2(width, FACT_ROW),
                );
                // The raw id is data, so it is mono — but it is not a link:
                // the active row's *state* takes the accent, not its name.
                ui.painter().text(
                    pos2(row.left() + PAD, row.center().y),
                    Align2::LEFT_CENTER,
                    &line.id,
                    type_scale::mono().font,
                    tokens.ink,
                );
                ui.painter().text(
                    pos2(row.right() - PAD, row.center().y),
                    Align2::RIGHT_CENTER,
                    &line.value,
                    type_scale::caption().font,
                    if line.value.contains("in use") {
                        tokens.accent
                    } else {
                        tokens.ink_soft
                    },
                );
                a11y::describe(
                    ui,
                    Rect::from_min_size(
                        pos2(row.left() + PAD, row.top()),
                        vec2(row.width() - PAD * 2.0, FACT_ROW - 1.0),
                    ),
                    &format!("{}: {}", line.id, line.value),
                );
                if index + 1 < files.len() {
                    ui.painter().line_segment(
                        [
                            pos2(row.left() + PAD, row.bottom() - 0.5),
                            pos2(row.right() - PAD, row.bottom() - 0.5),
                        ],
                        Stroke::new(1.0_f32, tokens.hairline),
                    );
                }
            }
            y = card.bottom() + SECTION_GAP;

            // ---- Engine: the rest of the build vocabulary -----------------
            y = section_label(ui, tokens, x, y, "Engine");
            let facts: [(&str, &str); 3] = [
                ("Speech engine", "whisper.cpp"),
                ("Speaker labels", "pyannote 3.0 + CAM++"),
                ("Platform", "Android · arm64"),
            ];
            let card = Rect::from_min_size(
                pos2(x, y),
                vec2(width, FACT_ROW * facts.len() as f32),
            );
            material::frosted_panel(ui, tokens, card, tokens.radius.card, Plane::Card);
            for (index, (label, value)) in facts.iter().enumerate() {
                let row = Rect::from_min_size(
                    pos2(x, card.top() + index as f32 * FACT_ROW),
                    vec2(width, FACT_ROW),
                );
                ui.painter().text(
                    pos2(row.left() + PAD, row.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    type_scale::row_value().font,
                    tokens.ink,
                );
                ui.painter().text(
                    pos2(row.right() - PAD, row.center().y),
                    Align2::RIGHT_CENTER,
                    value,
                    type_scale::caption().font,
                    tokens.ink_soft,
                );
                a11y::describe(
                    ui,
                    Rect::from_min_size(
                        pos2(row.left() + PAD, row.top()),
                        vec2(row.width() - PAD * 2.0, FACT_ROW - 1.0),
                    ),
                    &format!("{label}: {value}"),
                );
                if index + 1 < facts.len() {
                    ui.painter().line_segment(
                        [
                            pos2(row.left() + PAD, row.bottom() - 0.5),
                            pos2(row.right() - PAD, row.bottom() - 0.5),
                        ],
                        Stroke::new(1.0_f32, tokens.hairline),
                    );
                }
            }
            y = card.bottom() + 16.0;

            let note = "The release rotation check: rotate during launch, then read the lifecycle counters and the crash banner here.";
            let note_galley = paint_wrapped(
                ui,
                pos2(x, y),
                note,
                &type_scale::caption(),
                tokens.ink_faint,
                width,
                2,
            );
            a11y::describe(
                ui,
                Rect::from_min_size(pos2(x, y), vec2(width, note_galley.size().y.max(16.0))),
                note,
            );
            ui.allocate_space(vec2(width, (note_galley.size().y + 16.0).max(0.0)));
        });
    actions
}

/// The crash banner: the one alarm surface on the screen (a red-tinted card,
/// not the frosted Card plane), with the block's first lines in mono and the
/// two actions.
#[allow(clippy::too_many_arguments)]
fn crash_banner(
    ui: &mut Ui,
    tokens: &Tokens,
    x: f32,
    y: f32,
    width: f32,
    crash: &super::model::CrashBanner,
    actions: &mut Vec<CompactAction>,
) -> f32 {
    let height = 220.0;
    let rect = Rect::from_min_size(pos2(x, y), vec2(width, height));
    ui.painter().rect(
        rect,
        CornerRadius::same(tokens.radius.card),
        tokens.record_red.gamma_multiply(0.12),
        Stroke::new(1.0_f32, tokens.record_red.gamma_multiply(0.55)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        pos2(rect.left() + PAD, rect.top() + 20.0),
        Align2::LEFT_CENTER,
        crash.title(),
        type_scale::row_title().font,
        tokens.record_red_text,
    );
    let body = truncate_lines(&crash.text, 6);
    let galley = ui.painter().layout(
        body,
        FontId::new(11.0, egui::FontFamily::Monospace),
        tokens.ink,
        rect.width() - PAD * 2.0,
    );
    ui.painter().galley(
        pos2(rect.left() + PAD, rect.top() + 38.0),
        galley,
        tokens.ink,
    );
    a11y::describe(ui, rect, &format!("{}: {}", crash.title(), crash.text));
    let clear = Rect::from_min_size(
        pos2(rect.left() + PAD, rect.bottom() - 48.0),
        vec2(112.0, 48.0),
    );
    let share = Rect::from_min_size(
        pos2(rect.left() + PAD + 120.0, rect.bottom() - 48.0),
        vec2(112.0, 48.0),
    );
    for (hit, id, label, action) in [
        (
            clear,
            "diag-clear",
            "Clear crash",
            CompactAction::ClearCrash,
        ),
        (
            share,
            "diag-share",
            "Share log",
            CompactAction::ShareCrashLog,
        ),
    ] {
        let response = a11y::touch(ui, tokens, hit, id, WidgetType::Button, label, true);
        let pill = Rect::from_center_size(hit.center(), vec2(hit.width(), 40.0));
        ui.painter().rect(
            pill,
            CornerRadius::same(tokens.radius.control),
            tokens.control_fill,
            Stroke::new(1.0_f32, tokens.hairline),
            StrokeKind::Inside,
        );
        ui.painter().text(
            pill.center(),
            Align2::CENTER_CENTER,
            label,
            type_scale::caption().font,
            tokens.ink,
        );
        if response.clicked() {
            actions.push(action);
        }
    }
    rect.bottom()
}

/// A section label and the card top that follows it: label (16) → 8 → card.
fn section_label(ui: &mut Ui, tokens: &Tokens, x: f32, y: f32, label: &str) -> f32 {
    chrome::section_label(ui, tokens, pos2(x, y), label);
    y + 16.0 + 8.0
}

/// Paints `text` wrapped to `width` over at most `rows` lines (the `…` takes
/// over on the last one). Returns the galley so callers can size their block.
fn paint_wrapped(
    ui: &Ui,
    pos: egui::Pos2,
    text: &str,
    token: &type_scale::TypeToken,
    ink: egui::Color32,
    width: f32,
    rows: usize,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, token.format(ink));
    job.wrap.max_width = width.max(40.0);
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley.clone(), ink);
    galley
}

/// The banner's preview: the block's first `lines` lines (the whole text stays
/// in the a11y description and in the file the share route sends).
fn truncate_lines(text: &str, lines: usize) -> String {
    let mut out = String::new();
    for (index, line) in text.lines().take(lines).enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    if text.lines().count() > lines {
        out.push_str("\n…");
    }
    out
}
