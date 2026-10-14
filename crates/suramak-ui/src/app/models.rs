//! Models route (M3): the model manager table — manifest rows with install /
//! encoder state, a default marker, and download / delete / set-default actions.
//!
//! M9a: tokens, the unified control system (painted tiers, every state) and the
//! one navigation pattern (Home, top-right) — the composition is unchanged.
//!
//! M9e (P2-3): the table scrolls, and below ~1020 px window width the action
//! cluster **flows to the row's second line** — the right-anchored actions used
//! to land on INSTALLED (`missing` split across the Download button, the
//! ACTION/INSTALLED header mash). The name/state columns keep their positions;
//! the ACTIONS header is dropped in the narrow layout (the buttons are the
//! cluster's own label).
//!
//! All decisions live in `app::model_manager` (egui-free, unit-tested); this view
//! only renders rows and requests effects.

use egui::{pos2, vec2, Align2, CornerRadius, FontFamily, FontId, Rect};

use crate::app::controls::{self, Tier};
use crate::motion::{self, Motion};
use crate::theme::Tokens;

use super::model_manager::{human_size, DownloadState, InstallState, ModelRow};
use super::views::{pane_caption, pane_title, PANE_PAD_X};
use super::{AppState, Effect};

const ROW_HEIGHT: f32 = 46.0;
/// Narrow layout: the row grows to two lines (state line + actions line).
const ROW_HEIGHT_NARROW: f32 = 78.0;
/// The name column's width (the human names truncate here; SIZE starts after).
const NAME_COLUMN_WIDTH: f32 = 195.0;
/// Below this pane width the action cluster wraps to the row's second line
/// (M9e, P2-3). The pane is `window − 23% sidebar`, so this is ≈ a 1020 px
/// window at 1x — the width the critique measured the collisions at.
pub const NARROW_PANE_WIDTH: f32 = 790.0;

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
    let narrow = rect.width() < NARROW_PANE_WIDTH;

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

    // M9e (P2-3): the table region scrolls (Re-check + header + every row), so
    // no row is silently clipped at a small window; the status line stays
    // pinned below it.
    let table_rect = Rect::from_min_max(
        pos2(left, rect.top() + 96.0),
        pos2(right, rect.bottom() - 44.0),
    );
    let rows = state.model_manager.rows.clone();
    let download = state.model_manager.download.clone();
    let epoch = state.model_manager.refresh_epoch;
    // M9c: the download bar eases toward the reported fraction — chunked
    // reports read as one continuous move (a snap under Reduce Motion). When
    // no download runs, the last shown value stands (the bar is fading out).
    let eased = match &download {
        DownloadState::Running { .. } => {
            let target = download.percent().map_or(0.0, |pct| f32::from(pct) / 100.0);
            let dt = ui.ctx().input(|input| input.stable_dt);
            state
                .model_manager
                .progress_ease
                .approach(target, dt, state.reduce_motion)
        }
        _ => state.model_manager.progress_ease.shown(),
    };
    ui.scope_builder(egui::UiBuilder::new().max_rect(table_rect), |ui| {
        egui::ScrollArea::vertical()
            // Own id (see the settings form): without the salt this table and
            // the settings form share an id and a scroll offset.
            .id_salt("models-table")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Content x starts at the table rect's left edge (no horizontal
                // scrolling); rows are painted at that origin.
                let left = ui.max_rect().left();
                let width = ui.available_width().max(1.0);
                let right = left + width;

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
                ui.add_space(6.0);

                // Column header (painted into an allocated 24 pt row so the
                // flow cursor advances; the actions header only exists in the
                // wide layout — the narrow cluster is on its own line).
                let (header, _) = ui.allocate_exact_size(vec2(width, 24.0), egui::Sense::hover());
                header_row(ui.painter(), tokens, header.top(), left, right, narrow);
                ui.add_space(2.0);

                if rows.is_empty() {
                    ui.horizontal(|ui| {
                        super::views::arriving_caption(
                            ui,
                            tokens,
                            motion,
                            "models-loading",
                            "reading the manifest…",
                            false,
                        );
                    });
                }
                for row in &rows {
                    let height = if narrow {
                        ROW_HEIGHT_NARROW
                    } else {
                        ROW_HEIGHT
                    };
                    let (row_rect, _) =
                        ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
                    model_row(
                        ui,
                        tokens,
                        motion,
                        state,
                        effects,
                        row,
                        &download,
                        epoch,
                        eased,
                        row_rect.top(),
                        left,
                        right,
                        narrow,
                    );
                }
                // Bottom reserve (M9e): the floating record pill floats over
                // the window's bottom ~76 pt; when the table is scrolled to
                // its end the last row must clear it (the row's action line
                // otherwise lands under the pill).
                ui.add_space(44.0);
            });
    });

    // Status line for the last action (M9c: fades in base / out fast).
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(
            pos2(left, rect.bottom() - 34.0),
            vec2(right - left, 24.0),
        )),
        |ui| {
            ui.horizontal(|ui| {
                let message = state
                    .model_manager
                    .status
                    .as_ref()
                    .map(|status| match status {
                        Ok(note) => (note.as_str(), false),
                        Err(err) => (err.as_str(), true),
                    });
                super::views::status_caption(ui, tokens, motion, "models-status", message);
            });
        },
    );
}

/// The column header. Narrow layout: the action cluster is on the row's second
/// line, so there is no ACTIONS header (the cluster is self-labelled) — the
/// named ACTION/INSTALLED mash cannot exist.
fn header_row(
    painter: &egui::Painter,
    tokens: &Tokens,
    y: f32,
    left: f32,
    right: f32,
    narrow: bool,
) {
    let font = FontId::new(10.5, FontFamily::Monospace);
    let dim = tokens.ink_soft;
    painter.text(
        pos2(left + 4.0, y),
        Align2::LEFT_TOP,
        "MODEL",
        font.clone(),
        dim,
    );
    painter.text(
        pos2(left + 215.0, y),
        Align2::LEFT_TOP,
        "SIZE",
        font.clone(),
        dim,
    );
    painter.text(
        pos2(left + 305.0, y),
        Align2::LEFT_TOP,
        "INSTALLED",
        font.clone(),
        dim,
    );
    painter.text(
        pos2(left + 395.0, y),
        Align2::LEFT_TOP,
        "ENCODER",
        font.clone(),
        dim,
    );
    if !narrow {
        painter.text(
            pos2(right - 330.0, y),
            Align2::RIGHT_TOP,
            "ACTIONS",
            font,
            dim,
        );
    }
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
    epoch: u64,
    eased: f32,
    y: f32,
    left: f32,
    right: f32,
    narrow: bool,
) {
    let height = if narrow {
        ROW_HEIGHT_NARROW
    } else {
        ROW_HEIGHT
    };
    let rect = Rect::from_min_size(pos2(left, y), vec2(right - left, height - 4.0));
    // M9c row transition: every table load (re-check / delete / download done)
    // is a fresh arrival epoch — the row fades in once over the base token.
    let row_fade = motion.arrive(
        ui.ctx(),
        egui::Id::new(("suramak-model-row", epoch, row.id)),
        motion::BASE,
    );
    ui.set_opacity(row_fade);
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(tokens.radius.row), tokens.card);
    painter.rect_stroke(
        rect,
        CornerRadius::same(tokens.radius.row),
        egui::Stroke::new(tokens.stroke.hairline, tokens.hairline),
        egui::StrokeKind::Inside,
    );

    let small = FontId::new(11.0, FontFamily::Monospace);
    // The human name is the row's primary line; the raw id + role stay as the
    // quiet detail (workorder: the raw id may stay in the manager). The name
    // is truncated to its column so it can never run into SIZE (M9c: the
    // human names are wider than the old raw ids).
    let name = super::views::truncated_galley(
        ui,
        &super::model_manager::display_name(row.id),
        FontId::new(12.5, FontFamily::Proportional),
        tokens.ink,
        NAME_COLUMN_WIDTH,
    );
    painter.galley(pos2(left + 10.0, y + 7.0), name, tokens.ink);
    painter.text(
        pos2(left + 10.0, y + 24.0),
        Align2::LEFT_TOP,
        format!("{} · {}", row.id, row.role),
        FontId::new(10.0, FontFamily::Monospace),
        tokens.ink_soft,
    );
    painter.text(
        pos2(left + 215.0, y + 8.0),
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
        pos2(left + 305.0, y + 8.0),
        Align2::LEFT_TOP,
        row.install.label(),
        small.clone(),
        install_color,
    );
    painter.text(
        pos2(left + 395.0, y + 8.0),
        Align2::LEFT_TOP,
        row.encoder.label(),
        small.clone(),
        tokens.ink_soft,
    );
    if row.is_default {
        painter.text(
            pos2(left + 475.0, y + 8.0),
            Align2::LEFT_TOP,
            "● default",
            small.clone(),
            tokens.accent,
        );
    }

    // Per-row download progress (fed by the runtime's poll; the bar and its
    // percent are eased — no jitter). On completion the bar crossfades out
    // over the base token instead of vanishing. In the narrow layout the bar
    // sits on the state line (line 1), clear of the actions below it.
    let running_this_row = download.active_id() == Some(row.id);
    let bar_fade = motion.factor(
        ui.ctx(),
        egui::Id::new(("suramak-model-bar", row.id)),
        running_this_row,
        motion::BASE,
    );
    if bar_fade > 0.01 {
        let fraction = eased.clamp(0.0, 1.0);
        let bar = Rect::from_min_size(pos2(left + 220.0, y + 27.0), vec2(260.0, 6.0));
        painter.rect_filled(
            bar,
            CornerRadius::same(3),
            tokens.panel_faint.gamma_multiply(bar_fade),
        );
        painter.rect_filled(
            Rect::from_min_size(bar.min, vec2(bar.width() * fraction, bar.height())),
            CornerRadius::same(3),
            tokens.accent.gamma_multiply(bar_fade),
        );
        painter.text(
            pos2(bar.right() + 8.0, y + 24.0),
            Align2::LEFT_TOP,
            format!(
                "{}% — {} / {}",
                (fraction * 100.0).round() as u8,
                human_size(download_bytes_done(download)),
                human_size(download_bytes_total(download, row.size_bytes)),
            ),
            small.clone(),
            tokens.ink_soft.gamma_multiply(bar_fade),
        );
    }

    // Actions: right-anchored on the row's first line (wide) or left-aligned
    // on its own second line (narrow, M9e P2-3) — never over INSTALLED.
    let actions = if narrow {
        Rect::from_min_size(
            pos2(left + 10.0, y + 44.0),
            vec2(right - left - 20.0, ROW_HEIGHT_NARROW - 52.0),
        )
    } else {
        Rect::from_min_size(pos2(right - 320.0, y + 4.0), vec2(300.0, ROW_HEIGHT - 12.0))
    };
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
    ui.set_opacity(1.0);
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

/// Bytes reported so far (0 when no download runs — the bar is fading out).
fn download_bytes_done(download: &DownloadState) -> u64 {
    match download {
        DownloadState::Running { bytes_done, .. } => *bytes_done,
        _ => 0,
    }
}

/// Total bytes for the row (falls back to the manifest size while fading).
fn download_bytes_total(download: &DownloadState, manifest: u64) -> u64 {
    match download {
        DownloadState::Running { bytes_total, .. } if *bytes_total > 0 => *bytes_total,
        _ => manifest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::AppTheme;

    fn frame_at(
        ctx: &egui::Context,
        state: &mut AppState,
        size: (f32, f32),
        time: f64,
    ) -> egui::FullOutput {
        let tokens = crate::theme::tokens(AppTheme::Light);
        let motion = Motion::new(false);
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(size.0, size.1));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let mut effects = Vec::new();
                    super::ui(ui, &tokens, &motion, state, &mut effects);
                });
        })
    }

    fn texts(output: &egui::FullOutput) -> Vec<(String, Rect)> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    clipped.shape.visual_bounding_rect(),
                )),
                _ => None,
            })
            .collect()
    }

    fn find(texts: &[(String, Rect)], label: &str) -> Option<Rect> {
        texts
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, rect)| *rect)
    }

    /// M9e (P2-3): at the 900×600 minimum the pane is narrow — the action
    /// cluster flows to the row's second line. The geometry rule is pure:
    /// `narrow = pane < 790` (≈ a 1020 px window).
    #[test]
    fn the_narrow_threshold_matches_the_1020px_window_rule() {
        // Sidebar = window × 0.23 (clamped 190..360), pane = window − sidebar.
        let pane = |window: f32| window - (window * 0.23).clamp(190.0, 360.0);
        assert!(pane(900.0) < NARROW_PANE_WIDTH, "900×600 is narrow");
        assert!(pane(1020.0) < NARROW_PANE_WIDTH, "~1020 px is the boundary");
        assert!(
            pane(1100.0) > NARROW_PANE_WIDTH,
            "comfortably wide windows keep the wide layout"
        );
        assert!(pane(1280.0) > NARROW_PANE_WIDTH, "1280×800 is wide");
    }

    /// M9e (P2-3) frame-level: at 900×600 the header's column labels do not
    /// overlap, the ACTIONS header is gone (no mash) and the action cluster
    /// sits on the row's second line, clear of the INSTALLED column. At
    /// 1280×800 the layout is unchanged (the ACTIONS header is back).
    #[test]
    fn the_models_columns_do_not_collide_at_the_minimum_and_1280_is_unchanged() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = AppState::new();
        state.on_models_loaded(crate::app::model_manager::rows(
            std::path::Path::new("/nonexistent/models"),
            "auto",
        ));
        assert!(!state.model_manager.rows.is_empty(), "manifest rows");

        // The view sees the **pane** (the window minus the 23% sidebar), so
        // the test frames the pane sizes the real app produces.
        let pane = |window: f32| window - (window * 0.23).clamp(190.0, 360.0);

        // ---- narrow (a 900×600 window ⇒ ≈693 pt pane) -------------------------
        let _ = frame_at(&ctx, &mut state, (pane(900.0), 600.0), 0.0);
        let output = frame_at(&ctx, &mut state, (pane(900.0), 600.0), 0.2);
        let narrow_texts = texts(&output);
        assert!(find(&narrow_texts, "MODEL").is_some());
        assert!(find(&narrow_texts, "INSTALLED").is_some());
        assert!(
            find(&narrow_texts, "ACTIONS").is_none(),
            "the narrow layout drops the ACTIONS header (the named mash)"
        );
        let mut header_rects: Vec<Rect> = ["MODEL", "SIZE", "INSTALLED", "ENCODER"]
            .iter()
            .filter_map(|label| find(&narrow_texts, label))
            .collect();
        assert_eq!(header_rects.len(), 4, "all four narrow header labels paint");
        header_rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
        for pair in header_rects.windows(2) {
            assert!(
                pair[0].right() <= pair[1].left(),
                "header labels overlap: {:?} vs {:?}",
                pair[0],
                pair[1]
            );
        }
        // The Download control's label is on the row's second line, below the
        // first row's state line (row top = 150 + 24 header + 2 gap ≈ 176).
        let download = find(&narrow_texts, "Download").expect("a Download control");
        let installed = find(&narrow_texts, "missing").expect("an INSTALLED value");
        assert!(
            download.top() > installed.top() + 20.0,
            "the narrow cluster must sit on the row's second line: download {download:?} vs installed {installed:?}"
        );

        // ---- wide (a 1280×800 window ⇒ ≈986 pt pane): unchanged --------------
        let _ = frame_at(&ctx, &mut state, (pane(1280.0), 800.0), 0.4);
        let output = frame_at(&ctx, &mut state, (pane(1280.0), 800.0), 0.6);
        let wide_texts = texts(&output);
        assert!(
            find(&wide_texts, "ACTIONS").is_some(),
            "the wide layout keeps its ACTIONS header"
        );
        let download = find(&wide_texts, "Download").expect("a Download control");
        let installed = find(&wide_texts, "missing").expect("an INSTALLED value");
        assert!(
            download.top() < installed.top() + 20.0,
            "the wide cluster shares the row's first line: download {download:?} vs installed {installed:?}"
        );
    }
}
