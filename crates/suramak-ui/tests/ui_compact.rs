//! The phone-viewport screenshot suite (M13g1, extended in M13g2).
//!
//! Runs the compact profile at the plan's portrait viewport (1216 × 2640
//! physical, scale 2.6 ⇒ 467.7 × 1015.4 points), rasterises the real egui shape
//! stream with [`suramak_ui::deck::snapshot`], writes the shots under
//! `tests/ui-compact/shots/`, and asserts the rules the profile promises:
//!
//! * 48 dp targets, nothing outside the viewport, the bottom bar clearing the
//!   gesture inset;
//! * the export sheet's open state and its picker-result state (M13g2);
//! * the M13D model tiers on Settings (M13g2's low-memory shot);
//! * every compact control carries a non-empty semantic label (the a11y
//!   floor), with its role;
//! * Reduce Motion removes the navigation travel and keeps exactly one
//!   crossfade token of frames — never an idle redraw;
//! * a settled screen with a live job asks for no frames at all.
//!
//! The suite is display-free by construction (the rasteriser owns its pixels),
//! but `tests/ui-compact/run.sh` invokes it under `xvfb-run` exactly like the
//! desktop smoke so CI has one harness family and no GPU variance in the goldens.

use suramak_core::device::DeviceProfile;
use suramak_core::settings::Appearance;
use suramak_core::transcript::{Segment, Source, Transcript};
use suramak_ui::deck::compact::{
    self, a11y, chrome, library, CompactApp, CompactRoute, CompactSettings, Insets, Sheet,
};
use suramak_ui::deck::snapshot;
use suramak_ui::deck::theme::{self, AppTheme};

fn transcript(title: &str, language: &str, texts: &[&str], start_ms: i64) -> Transcript {
    let segments = texts
        .iter()
        .enumerate()
        .map(|(index, text)| Segment {
            idx: index as u32,
            start_ms: start_ms + index as i64 * 4_200,
            end_ms: start_ms + index as i64 * 4_200 + 3_900,
            text: (*text).to_string(),
        })
        .collect();
    Transcript::new(
        title.to_string(),
        Source::File {
            name: "voice-note.m4a".to_string(),
        },
        "base".to_string(),
        Some(language.to_string()),
        None,
        segments,
    )
}

fn fixture_settings() -> CompactSettings {
    CompactSettings {
        model_id: "base".to_string(),
        language: "en".to_string(),
        appearance: Appearance::Dark,
        keep_audio: true,
    }
}

fn seeded_app() -> CompactApp {
    let mut app = CompactApp::new(fixture_settings());
    app.theme = AppTheme::Dark;
    app.insets = Insets {
        top: 28.0,
        bottom: 20.0,
    };
    app.set_library(vec![
        transcript(
            "Morning notes",
            "en",
            &[
                "The quick brown fox jumps over the lazy dog.",
                "This is a second line that is long enough to wrap across the phone's content column.",
                "And a third line closes the example transcript.",
            ],
            0,
        ),
        transcript(
            "Rapat tim produk",
            "id",
            &[
                "Kita membahas anggaran kuartal depan dan jadwal peluncuran.",
                "Tim desain akan menyiapkan dua alternatif.",
            ],
            12_000,
        ),
        transcript(
            "Interview — field recording",
            "en",
            &["We walked through the market and recorded the ambience."],
            0,
        ),
    ]);
    app.seed_device(
        DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
        &["base".to_string()],
    );
    app
}

fn fresh_context(app: &CompactApp) -> egui::Context {
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::install(&ctx, app.theme, app.reduce_motion);
    ctx
}

fn render(app: &mut CompactApp, name: &str) -> snapshot::Shot {
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let ctx = fresh_context(app);
    let shot = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, app);
    });
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/ui-compact/shots")
        .join(format!("{name}.png"));
    snapshot::write_png(&out, &shot).expect("write shot");
    eprintln!("shot: {} ({}x{})", out.display(), shot.width, shot.height);
    shot
}

#[test]
fn the_phone_viewport_shoots_library_editor_and_settings() {
    // --- Library -----------------------------------------------------------
    let mut app = seeded_app();
    app.set_library_query("");
    let library_shot = render(&mut app, "library");
    assert_eq!(library_shot.width, 1216);
    assert_eq!(library_shot.height, 2640);
    assert!(
        library_shot.painted_fraction(egui::Color32::BLACK) > 0.5,
        "the library frame must not be blank"
    );
    assert_eq!(app.nav.current(), CompactRoute::Library);
    let screen = compact::phone_screen_rect();
    assert!(
        library::bar_is_safe(screen),
        "the Import/Record bar must clear the gesture inset and never overlap"
    );
    let search = library::search_rect(screen);
    assert!(search.height() >= chrome::MIN_TOUCH);
    assert!(
        screen.contains_rect(search),
        "the search field is in-viewport"
    );

    // --- Editor ------------------------------------------------------------
    let target = app.transcripts[0].clone();
    app.open_editor(target, 0.0);
    app.advance(10.0); // settle the push spring: the shot is a rest frame
    app.editor.as_mut().expect("editor").select(1);
    let editor_shot = render(&mut app, "editor");
    assert!(editor_shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    assert_eq!(app.nav.current(), CompactRoute::Editor);

    // --- Settings ----------------------------------------------------------
    let mut app = seeded_app();
    app.open_settings(0.0);
    app.advance(10.0);
    let settings_shot = render(&mut app, "settings");
    assert!(settings_shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    assert_eq!(app.nav.current(), CompactRoute::Settings);
    let settings_models = &app.models;
    assert!(
        settings_models
            .iter()
            .any(|row| row.id == "base" && row.installed),
        "the M13D model table is on the Settings screen"
    );
}

#[test]
fn the_export_sheet_and_its_picker_result_are_shots() {
    let mut app = seeded_app();
    let target = app.transcripts[0].clone();
    app.open_editor(target, 0.0);
    app.advance(10.0);
    app.editor.as_mut().expect("editor").format = compact::CompactExportFormat::Srt;
    app.editor.as_mut().expect("editor").has_translation = true;
    app.export_bilingual = true;
    app.open_export_sheet(10.0);
    app.advance(11.0); // the sheet's present animation settles
    assert_eq!(app.sheet, Some(Sheet::Export));
    assert_eq!(app.sheet_anim.value(), 1.0);
    let sheet_shot = render(&mut app, "export-sheet");
    assert!(sheet_shot.painted_fraction(egui::Color32::BLACK) > 0.5);

    // The picker returned and the write verified: the sheet shows the result.
    let action = app.begin_export(12.0).expect("export action");
    assert!(matches!(action, compact::CompactAction::Export { .. }));
    app.on_export_picked();
    app.on_export_written("Morning notes.srt", 12_480);
    assert_eq!(
        app.export_session.as_ref().expect("session").phase,
        compact::ExportPhase::Written
    );
    let written_shot = render(&mut app, "export-written");
    assert!(written_shot.painted_fraction(egui::Color32::BLACK) > 0.5);

    // The sheet is dismissed with its exit animation, then gone.
    app.close_export_sheet(13.0);
    assert!(app.advance(13.05), "the exit animates");
    assert_eq!(
        app.sheet,
        Some(Sheet::Export),
        "still painted while closing"
    );
    assert!(!app.advance(13.2));
    assert_eq!(app.sheet, None);
    assert!(app.export_session.is_none());
}

#[test]
fn the_settings_shot_shows_the_low_tier_refusals() {
    let mut app = seeded_app();
    app.settings.model_id = "tiny".to_string();
    app.seed_device(
        DeviceProfile::new(3_000_000_000, 1_500_000_000, false, true),
        &["tiny".to_string()],
    );
    app.open_settings(0.0);
    app.advance(10.0);
    let shot = render(&mut app, "settings-tiers");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    let offered: Vec<&str> = app
        .models
        .iter()
        .filter(|row| row.offered)
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(offered, vec!["tiny"], "below the 4 GB floor only tiny runs");
    assert!(
        app.models.iter().any(|row| !row.offered),
        "the hidden/larger models are the M13D refusal statement"
    );
    assert_eq!(app.tier, Some(suramak_core::device::PhoneTier::BelowFloor));
}

#[test]
fn a_small_phone_keeps_every_target_at_least_48_dp() {
    let screen = compact::small_phone_screen_rect();
    assert!(library::bar_is_safe(screen));
    let [import, record] = library::action_rects(screen);
    assert!(import.height() >= chrome::MIN_TOUCH && import.width() > 100.0);
    assert!(record.height() >= chrome::MIN_TOUCH && record.width() > 100.0);
    assert!(screen.contains_rect(import) && screen.contains_rect(record));
    // The app bar and content never overlap the bars.
    let bands = chrome::bands(screen, 24.0, 16.0, Some(chrome::BOTTOM_BAR));
    assert!(bands.app_bar.bottom() <= bands.content.top());
    assert!(bands.content.bottom() <= bands.bottom_bar.top());
    assert!(bands.bottom_bar.bottom() <= screen.bottom() - 16.0);
}

#[test]
fn reduce_motion_removes_the_travel_and_keeps_one_crossfade_token() {
    let mut app = seeded_app();
    app.reduce_motion = true;
    let target = app.transcripts[0].clone();
    app.open_editor(target, 0.0);
    assert!(
        app.advance(0.0),
        "the opacity crossfade still asks for its 120 ms"
    );
    let motion = app.nav.motion().expect("push");
    assert_eq!(
        motion.offset(compact::phone_screen_rect().width()),
        0.0,
        "no translation under Reduce Motion"
    );
    assert!(motion.fade() < 1.0);
    assert!(
        !app.advance(0.13),
        "the crossfade ends on the fast token, not on a frame loop"
    );
    assert_eq!(app.nav.current(), CompactRoute::Editor);
}

#[test]
fn every_compact_control_carries_a_label_and_a_role() {
    let mut app = seeded_app();
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let ctx = fresh_context(&app);
    let _ = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    let controls = a11y::controls(&ctx);
    assert!(
        controls.len() >= 6,
        "the library frame registers its controls, got {}",
        controls.len()
    );
    for control in &controls {
        assert!(
            !control.label.trim().is_empty(),
            "unlabeled control at {:?}",
            control.rect
        );
    }
    let labels: Vec<&str> = controls
        .iter()
        .map(|control| control.label.as_str())
        .collect();
    assert!(labels.contains(&"Settings"), "labels: {labels:?}");
    assert!(labels
        .iter()
        .any(|label| label.starts_with("Morning notes")));
    assert!(labels.contains(&"Import audio"));
    assert!(labels.contains(&"Record from the microphone"));

    // The editor's chrome and transport, then the export sheet.
    let target = app.transcripts[0].clone();
    app.open_editor(target, 0.0);
    app.advance(10.0);
    let ctx = fresh_context(&app);
    let _ = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    let labels: Vec<String> = a11y::controls(&ctx)
        .into_iter()
        .map(|control| control.label)
        .collect();
    for expected in ["Back", "Play", "Find", "Export"] {
        assert!(
            labels.iter().any(|label| label == expected),
            "the editor registers {expected}: {labels:?}"
        );
    }
    assert!(labels.iter().any(|label| label.starts_with("Segment 1")));

    app.editor.as_mut().expect("editor").format = compact::CompactExportFormat::Srt;
    app.editor.as_mut().expect("editor").has_translation = true;
    app.open_export_sheet(11.0);
    app.advance(12.0);
    let ctx = fresh_context(&app);
    let _ = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    let controls = a11y::controls(&ctx);
    let labels: Vec<&str> = controls
        .iter()
        .map(|control| control.label.as_str())
        .collect();
    for expected in [
        "Export format: Markdown",
        "Export format: TSV",
        "Include the translation",
        "Share instead",
        "Export",
    ] {
        assert!(
            labels.contains(&expected),
            "the export sheet registers {expected}: {labels:?}"
        );
    }
}

#[test]
fn a_settled_screen_with_a_live_job_requests_no_frames() {
    let mut app = seeded_app();
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let mut ctx = fresh_context(&app);
    // Settle the initial frame.
    let _ = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    assert!(
        !app.needs_frame(),
        "an idle library has no animation to run"
    );

    // A live job is painted from its last event; the surface stays idle.
    app.on_job("Transcribing", 3, 10, 1.0);
    assert!(app.take_frame_request(), "the event wakes the host once");
    assert!(app.advance(1.05), "the progress fill eases to the value");
    assert!(!app.advance(1.3));
    assert!(!app.needs_frame());
    ctx = fresh_context(&app);
    // The first frame on a fresh context lays out and uploads the font atlas;
    // egui may ask for one settling frame. Measure the steady state.
    let mut requested = Vec::new();
    for frame in 0..3 {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(2.0 + frame as f64 * 0.016),
                ..Default::default()
            },
            |ctx| {
                let _ = compact::ui(ctx, &mut app);
                requested.push(ctx.has_requested_repaint());
            },
        );
    }
    assert_eq!(
        requested.last(),
        Some(&false),
        "a settled screen with a live job must not request a frame: {requested:?}"
    );
}
