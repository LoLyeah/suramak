//! Models route (M3): the model manager table — manifest rows with install /
//! encoder state, a default marker, and download / delete / set-default actions.
//!
//! M9a: tokens, the unified control system (painted tiers, every state) and the
//! one navigation pattern (Home, top-right) — the composition is unchanged.
//!
//! All decisions live in `app::model_manager` (egui-free, unit-tested); this view
//! only renders rows and requests effects.

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect};

use crate::app::controls::{self, Tier};
use crate::motion::Motion;
use crate::theme::Tokens;

use super::model_manager::{human_size, DownloadState, InstallState, ModelRow};
use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect};

const ROW_HEIGHT: f32 = 46.0;

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
            pane_title(ui, "Models", tokens);
            pane_caption(
                ui,
                "whisper.cpp models — downloaded from HuggingFace, sha256-verified, kept on this machine",
                tokens,
                false,
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.add_space(PANE_PAD_X - 8.0);
            if controls::nav_home(ui, tokens, motion, "models-nav").clicked() {
                effects.extend(state.back_home());
            }
        });
    });

    let mut y = rect.top() + 96.0;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 20.0))),
        |ui| {
            ui.horizontal(|ui| {
                let recheck = controls::button(
                    ui,
                    tokens,
                    motion,
                    "models-recheck",
                    "Re-check",
                    Tier::Tertiary,
                    true,
                )
                .on_hover_text("Re-read the manifest and the models folder");
                if recheck.clicked() {
                    effects.push(Effect::RefreshModels);
                }
            });
        },
    );
    y += 30.0;

    // Column header.
    header_row(ui, tokens, y, left, right);
    y += 24.0;

    let rows = state.model_manager.rows.clone();
    let download = state.model_manager.download.clone();
    if rows.is_empty() {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(left, y), vec2(right - left, 22.0))),
            |ui| {
                ui.horizontal(|ui| {
                    pane_caption(ui, "reading the manifest…", tokens, false);
                });
            },
        );
    }
    for row in &rows {
        if y + ROW_HEIGHT > rect.bottom() - 40.0 {
            break;
        }
        model_row(
            ui, tokens, motion, state, effects, row, &download, y, left, right,
        );
        y += ROW_HEIGHT;
    }

    // Status line for the last action.
    if let Some(status) = &state.model_manager.status {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(left, rect.bottom() - 34.0),
                vec2(right - left, 24.0),
            )),
            |ui| {
                ui.horizontal(|ui| match status {
                    Ok(note) => pane_caption(ui, note, tokens, false),
                    Err(err) => pane_caption(ui, err, tokens, true),
                });
            },
        );
    }
}

fn header_row(ui: &mut egui::Ui, tokens: &Tokens, y: f32, left: f32, right: f32) {
    let painter = ui.painter();
    let font = FontId::new(10.5, FontFamily::Monospace);
    let dim = tokens.ink_soft;
    painter.text(
        pos2(left + 4.0, y),
        Align2::LEFT_TOP,
        "ID",
        font.clone(),
        dim,
    );
    painter.text(
        pos2(left + 120.0, y),
        Align2::LEFT_TOP,
        "SIZE",
        font.clone(),
        dim,
    );
    painter.text(
        pos2(left + 200.0, y),
        Align2::LEFT_TOP,
        "INSTALLED",
        font.clone(),
        dim,
    );
    painter.text(
        pos2(left + 300.0, y),
        Align2::LEFT_TOP,
        "ENCODER",
        font.clone(),
        dim,
    );
    painter.text(
        pos2(right - 330.0, y),
        Align2::RIGHT_TOP,
        "ACTIONS",
        font,
        dim,
    );
}

#[allow(clippy::too_many_arguments)]
fn model_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    motion: &Motion,
    state: &mut AppState,
    effects: &mut Vec<Effect>,
    row: &ModelRow,
    download: &DownloadState,
    y: f32,
    left: f32,
    right: f32,
) {
    let rect = Rect::from_min_size(pos2(left, y), vec2(right - left, ROW_HEIGHT - 4.0));
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(tokens.radius.row), tokens.card);
    painter.rect_stroke(
        rect,
        CornerRadius::same(tokens.radius.row),
        egui::Stroke::new(tokens.stroke.hairline, tokens.hairline),
        egui::StrokeKind::Inside,
    );

    let mono = FontId::new(12.0, FontFamily::Monospace);
    let small = FontId::new(11.0, FontFamily::Monospace);
    painter.text(
        pos2(left + 10.0, y + 8.0),
        Align2::LEFT_TOP,
        row.id,
        mono.clone(),
        tokens.ink,
    );
    painter.text(
        pos2(left + 10.0, y + 24.0),
        Align2::LEFT_TOP,
        row.role,
        FontId::new(10.0, FontFamily::Proportional),
        tokens.ink_soft,
    );
    painter.text(
        pos2(left + 120.0, y + 8.0),
        Align2::LEFT_TOP,
        human_size(row.size_bytes),
        small.clone(),
        tokens.ink_soft,
    );
    let install_color = match row.install {
        InstallState::Installed => tokens.ink,
        InstallState::Partial => tokens.accent,
        InstallState::Missing => tokens.ink_soft,
    };
    painter.text(
        pos2(left + 200.0, y + 8.0),
        Align2::LEFT_TOP,
        row.install.label(),
        small.clone(),
        install_color,
    );
    painter.text(
        pos2(left + 300.0, y + 8.0),
        Align2::LEFT_TOP,
        row.encoder.label(),
        small.clone(),
        tokens.ink_soft,
    );
    if row.is_default {
        painter.text(
            pos2(left + 390.0, y + 8.0),
            Align2::LEFT_TOP,
            "● default",
            small.clone(),
            tokens.accent,
        );
    }

    // Per-row download progress (fed by the runtime's poll).
    if let DownloadState::Running {
        id,
        bytes_done,
        bytes_total,
    } = download
    {
        if id == row.id {
            let fraction = if *bytes_total == 0 {
                0.0
            } else {
                (*bytes_done as f32 / *bytes_total as f32).clamp(0.0, 1.0)
            };
            let bar = Rect::from_min_size(pos2(left + 220.0, y + 27.0), vec2(260.0, 6.0));
            painter.rect_filled(bar, CornerRadius::same(3), tokens.panel_faint);
            painter.rect_filled(
                Rect::from_min_size(bar.min, vec2(bar.width() * fraction, bar.height())),
                CornerRadius::same(3),
                tokens.accent,
            );
            painter.text(
                pos2(bar.right() + 8.0, y + 24.0),
                Align2::LEFT_TOP,
                format!(
                    "{}% — {} / {}",
                    download.percent().unwrap_or(0),
                    human_size(*bytes_done),
                    human_size(*bytes_total)
                ),
                small.clone(),
                tokens.ink_soft,
            );
        }
    }

    // Actions.
    let actions = Rect::from_min_size(pos2(right - 320.0, y + 4.0), vec2(300.0, ROW_HEIGHT - 12.0));
    let downloaded = download.active_id().is_some();
    let mut download_clicked = false;
    let mut delete_clicked = false;
    let mut default_clicked = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(actions), |ui| {
        ui.horizontal(|ui| {
            let download = controls::button(
                ui,
                tokens,
                motion,
                ("model-download", row.id),
                "Download",
                Tier::Secondary,
                row.install != InstallState::Installed && !downloaded,
            );
            if download.clicked() {
                download_clicked = true;
            }
            let delete = controls::button(
                ui,
                tokens,
                motion,
                ("model-delete", row.id),
                "Delete",
                Tier::Tertiary,
                row.install != InstallState::Missing && !downloaded,
            );
            if delete.clicked() {
                delete_clicked = true;
            }
            let set_default = controls::button(
                ui,
                tokens,
                motion,
                ("model-default", row.id),
                "Set default",
                Tier::Tertiary,
                row.install == InstallState::Installed && !row.is_default && !downloaded,
            );
            if set_default.clicked() {
                default_clicked = true;
            }
        });
    });
    if download_clicked {
        effects.extend(state.start_model_download(row.id));
    }
    if delete_clicked {
        effects.extend(state.delete_model(row.id));
    }
    if default_clicked {
        effects.extend(state.set_default_model(row.id));
    }
}
