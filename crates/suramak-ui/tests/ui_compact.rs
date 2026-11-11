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
use suramak_core::store::WordAlignment;
use suramak_core::transcript::{Segment, Source, Transcript, WordTiming};
use suramak_ui::deck::compact::{
    self, a11y, chrome, library, CompactApp, CompactRoute, CompactSettings, CrashBanner,
    DiagnosticsView, DownloadState, Insets, Sheet,
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

/// A tiny M15.6 fixture: four words inside the second segment.
fn words_for_second_segment() -> WordAlignment {
    let words = [
        ("This", 4_200, 4_500),
        ("is", 4_500, 4_650),
        ("a", 4_650, 4_750),
        ("second", 4_750, 5_300),
        ("line", 5_300, 5_700),
    ];
    let mut alignment = WordAlignment::default();
    alignment.current.insert(
        1,
        words
            .iter()
            .enumerate()
            .map(|(index, (text, start, end))| WordTiming {
                segment_idx: 1,
                word_idx: index as u32,
                start_ms: *start,
                end_ms: *end,
                text: (*text).to_string(),
            })
            .collect(),
    );
    alignment
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
fn the_phone_editor_inherits_the_speaker_badges_and_lane() {
    // M16I: the phone draws the same shared Deck components as the desktop —
    // one 8 px lane above the list, a badge on every row. No Android-specific
    // rendering exists; this shot is the inheritance proof.
    use suramak_core::diarization::{Speaker, SpeakerTurn};
    let mut app = seeded_app();
    let target = app.transcripts[0].clone();
    app.open_editor(target.clone(), 0.0);
    app.advance(10.0);
    let speakers = vec![Speaker::new(0, "Speaker 1", 0), Speaker::new(1, "Budi", 1)];
    let turns = vec![
        SpeakerTurn::new(0, 0, 4_200, 0),
        SpeakerTurn::new(1, 4_200, 8_400, 1),
        SpeakerTurn::new(2, 8_400, 12_000, 0),
    ];
    let projection = suramak_core::diarization::speaker_for_segments(&target.segments, &turns);
    app.editor.as_mut().expect("editor").speakers = Some(compact::CompactSpeakers {
        speakers,
        turns,
        per_segment: projection,
        stale: false,
        audio_ms: 12_000,
    });
    let shot = render(&mut app, "editor-speakers");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    // The lane is inside the content column, under the meta line, and the
    // badges sit inside the row's leading slot (nothing overflows the viewport).
    let screen = compact::phone_screen_rect();
    let bands = chrome::bands(
        screen,
        app.insets.top,
        app.insets.bottom,
        Some(chrome::BOTTOM_BAR),
    );
    assert!(screen.contains_rect(bands.content));
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

    // The same layout on the light token ladder (the second appearance is part
    // of the release surface, not a debug toggle).
    let mut light = seeded_app();
    light.theme = AppTheme::Light;
    let light_shot = render(&mut light, "library-light");
    assert!(light_shot.painted_fraction(egui::Color32::BLACK) > 0.5);

    // --- Editor ------------------------------------------------------------
    let target = app.transcripts[0].clone();
    app.open_editor(target, 0.0);
    app.advance(10.0); // settle the push spring: the shot is a rest frame
    app.editor.as_mut().expect("editor").select(1);
    app.editor
        .as_mut()
        .expect("editor")
        .set_words(words_for_second_segment());
    app.editor.as_mut().expect("editor").start_playback();
    app.on_playhead(4_900); // the "second" chip is current
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
fn the_launch_surface_is_the_library_and_the_first_run_card_leads() {
    // The launch surface: MainActivity's initial route is the Library, not the
    // spike's diagnostics. With a model installed it is the transcript list.
    let mut app = seeded_app();
    let shot = render(&mut app, "launch-library");
    assert_eq!(app.nav.current(), CompactRoute::Library);
    assert!(app.sheet.is_none(), "no sheet on a settled launch");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);

    // No model yet: the same route leads with M13D's card, never an empty list.
    let mut fresh = CompactApp::new(fixture_settings());
    fresh.theme = AppTheme::Dark;
    fresh.insets = Insets {
        top: 28.0,
        bottom: 20.0,
    };
    fresh.seed_device(
        DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
        &[],
    );
    fresh.set_free_space(Some(9_000_000_000));
    assert!(fresh.model_card_needed());
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let ctx = fresh_context(&fresh);
    let shot = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut fresh);
    });
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/ui-compact/shots")
        .join("first-run-download.png");
    snapshot::write_png(&out, &shot).expect("write shot");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    let labels: Vec<String> = a11y::controls(&ctx)
        .into_iter()
        .map(|control| control.label)
        .collect();
    assert!(
        labels.iter().any(|label| label == "Download base"),
        "the card offers the recommendation: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == "Download small"),
        "small is the next offered model: {labels:?}"
    );
    assert_eq!(
        fresh.recommended_model().map(|row| row.id.as_str()),
        Some("base")
    );
    assert!(
        !labels.iter().any(|label| label.contains("tiny")),
        "the card never headlines tiny on a 6 GB phone: {labels:?}"
    );

    // Indonesian at the same tier: still base, with the tiny warning on screen.
    let mut indonesian = CompactApp::new(CompactSettings {
        language: "id".to_string(),
        ..fixture_settings()
    });
    indonesian.theme = AppTheme::Dark;
    indonesian.seed_device(
        DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
        &[],
    );
    assert!(indonesian.language_note().unwrap().contains("tiny"));
    assert_eq!(
        indonesian.recommended_model().map(|row| row.id.as_str()),
        Some("base")
    );
}

#[test]
fn the_first_run_download_shows_the_jobs_percent_and_the_resume() {
    let mut app = CompactApp::new(fixture_settings());
    app.theme = AppTheme::Dark;
    app.seed_device(
        DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
        &[],
    );
    app.set_free_space(Some(9_000_000_000));
    app.on_download(
        "base",
        61_000_000,
        143_000_000,
        42,
        DownloadState::Running,
        "starting",
        0.0,
    );
    assert_eq!(app.download.as_ref().unwrap().percent, 42);
    let ctx = fresh_context(&app);
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let shot = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/ui-compact/shots")
        .join("download-progress.png");
    snapshot::write_png(&out, &shot).expect("write shot");
    let labels: Vec<String> = a11y::controls(&ctx)
        .into_iter()
        .map(|control| control.label)
        .collect();
    assert!(labels.iter().any(|label| label == "Pause the download"));
    assert!(
        labels.iter().any(|label| label.contains("42%")),
        "the card reads the job's percent: {labels:?}"
    );

    // Relaunch with the `.part`: the same number, offered as a resume.
    let mut resumed = CompactApp::new(fixture_settings());
    resumed.theme = AppTheme::Dark;
    resumed.seed_device(
        DeviceProfile::new(6_000_000_000, 2_000_000_000, false, true),
        &[],
    );
    resumed.set_free_space(Some(9_000_000_000));
    resumed.on_download_resume("base", 61_000_000, 143_000_000, 42);
    let ctx = fresh_context(&resumed);
    let shot = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut resumed);
    });
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/ui-compact/shots")
        .join("download-resume.png");
    snapshot::write_png(&out, &shot).expect("write shot");
    let labels: Vec<String> = a11y::controls(&ctx)
        .into_iter()
        .map(|control| control.label)
        .collect();
    assert!(
        labels.iter().any(|label| label == "Resume base"),
        "{labels:?}"
    );
}

#[test]
fn the_onboarding_sheet_is_a_shot_with_the_deck_spring() {
    let mut app = seeded_app();
    app.first_run_seen = false;
    assert!(app.maybe_begin_onboarding(0.0));
    app.advance(1.0); // the spring lands exactly
    assert_eq!(app.sheet, Some(Sheet::Onboarding));
    assert_eq!(app.sheet_anim.curve(), compact::motion::SheetCurve::Spring);
    let shot = render(&mut app, "onboarding");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    let ctx = fresh_context(&app);
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let _ = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    let labels: Vec<String> = a11y::controls(&ctx)
        .into_iter()
        .map(|control| control.label)
        .collect();
    for expected in ["Start using Suramak", "Skip the introduction"] {
        assert!(labels.iter().any(|label| label == expected), "{labels:?}");
    }

    // Reduce Motion removes the travel but keeps the sheet and its fade.
    let mut app = seeded_app();
    app.reduce_motion = true;
    app.maybe_begin_onboarding(0.0);
    assert!(app.advance(0.05));
    assert_eq!(app.sheet_anim.rise(true), 0.0);
    assert!(app.sheet_anim.value() > 0.0);
}

#[test]
fn the_empty_library_offers_the_share_hint() {
    let mut app = seeded_app();
    app.set_library(Vec::new());
    assert!(app.rows.is_empty() && !app.model_card_needed());
    let shot = render(&mut app, "library-empty");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    let ctx = fresh_context(&app);
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let _ = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    let labels: Vec<String> = a11y::controls(&ctx)
        .into_iter()
        .map(|control| control.label)
        .collect();
    assert!(
        labels
            .iter()
            .any(|label| label.contains("Share a voice note") && label.contains("Import")),
        "the empty state names both working entries: {labels:?}"
    );
}

#[test]
fn the_diagnostics_card_keeps_the_four_checks_and_the_crash_banner() {
    let mut app = seeded_app();
    app.open_settings(0.0);
    app.advance(10.0);
    app.open_diagnostics(10.0);
    app.advance(11.0);
    assert_eq!(app.nav.current(), CompactRoute::Diagnostics);
    app.set_diagnostics(DiagnosticsView {
        text_chars: 128,
        ime_updates: 14,
        deletions: 3,
        resumes: 4,
        surface_recreates: 2,
        frames_gated: 17,
        frames_after_destroy: 0,
        probe_saves: 3,
        probe_restores: 3,
        avg_fps: Some(59.6),
        frames: 2_400,
        inset_updates: 6,
        inset_top: 28.0,
        inset_bottom: 20.0,
        back_presses: 2,
        crash: Some(CrashBanner {
            live: false,
            text: "=== Suramak spike crash #1 ===\nwhen: 2026-11-11\nwhere: src/app.rs:10:5\npanic: rotation probe\n--- last 3 platform events ---\n[0.10s] info onCreate"
                .to_string(),
        }),
    });
    let shot = render(&mut app, "diagnostics");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
    let ctx = fresh_context(&app);
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let _ = snapshot::render(&ctx, screen, scale, |ctx| {
        let _ = compact::ui(ctx, &mut app);
    });
    let labels: Vec<String> = a11y::controls(&ctx)
        .into_iter()
        .map(|control| control.label)
        .collect();
    for expected in ["Clear crash", "Share log", "Back"] {
        assert!(labels.iter().any(|label| label == expected), "{labels:?}");
    }
    assert!(
        labels.iter().any(|label| label.contains("128 chars")),
        "the text counter is on the card: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("CRASH — LAST LAUNCH")),
        "the v2.4 banner is on the card: {labels:?}"
    );
    assert_eq!(app.back(12.0), suramak_ui::deck::nav::BackOutcome::Popped);
    assert_eq!(app.nav.current(), CompactRoute::Settings);
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

// ---------------------------------------------------------------------------
// M19A: the Settings list scrolls, the rows follow the finger, a cancelled
// touch never toggles, and the DEVICE card clears the gesture pill.
// ---------------------------------------------------------------------------

/// A live egui context + app, so a test can drive real touch frames instead of
/// painting one still. The touch options are the phone's (12 pt / 0.5 s), which
/// is what makes the wobble and drag cases mean anything.
struct Harness {
    ctx: egui::Context,
    app: CompactApp,
    pointer: compact::input::TouchPointer,
    screen: egui::Rect,
    time: f64,
}

impl Harness {
    fn new(app: CompactApp) -> Self {
        let ctx = fresh_context(&app);
        compact::input::apply_touch_options(&ctx);
        Self {
            ctx,
            app,
            pointer: compact::input::TouchPointer::new(),
            screen: compact::phone_screen_rect(),
            time: 10.0,
        }
    }

    fn raw(&mut self) -> egui::RawInput {
        self.time += 0.05;
        egui::RawInput {
            screen_rect: Some(self.screen),
            time: Some(self.time),
            ..Default::default()
        }
    }

    fn run(&mut self, raw: egui::RawInput) -> Vec<compact::CompactAction> {
        let app = &mut self.app;
        let mut actions = Vec::new();
        let _ = self.ctx.run(raw, |ctx| {
            actions = compact::ui(ctx, app);
        });
        actions
    }

    fn frame(&mut self) -> Vec<compact::CompactAction> {
        let raw = self.raw();
        self.run(raw)
    }

    /// One frame carrying one raw finger event; the shell's synthesiser turns
    /// it into pointer events exactly as the phone does.
    fn touch(
        &mut self,
        id: u64,
        phase: egui::TouchPhase,
        pos: egui::Pos2,
    ) -> Vec<compact::CompactAction> {
        let mut raw = self.raw();
        raw.events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId::from(id),
            phase,
            pos,
            force: None,
        });
        let _ = self.pointer.prepare(&mut raw);
        self.run(raw)
    }

    fn control(&self, label: &str) -> egui::Rect {
        a11y::controls(&self.ctx)
            .into_iter()
            .find(|control| control.label == label)
            .unwrap_or_else(|| {
                let labels: Vec<String> = a11y::controls(&self.ctx)
                    .into_iter()
                    .map(|control| control.label)
                    .collect();
                panic!("no control labeled {label:?}; got {labels:?}")
            })
            .rect
    }

    fn shot(&mut self, name: &str) -> snapshot::Shot {
        let (_, _, scale) = compact::phone_viewport();
        let screen = self.screen;
        let ctx = self.ctx.clone();
        let app = &mut self.app;
        let shot = snapshot::render(&ctx, screen, scale, |ctx| {
            let _ = compact::ui(ctx, app);
        });
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/ui-compact/shots")
            .join(format!("{name}.png"));
        snapshot::write_png(&out, &shot).expect("write shot");
        shot
    }
}

/// A phone with enough model rows that Settings really overflows: 12 GB offers
/// four models, so the list needs ~130 pt of scroll and the DEVICE card sits at
/// the end of it.
fn overflowing_settings() -> CompactApp {
    let mut app = seeded_app();
    app.settings.model_id = "base".to_string();
    app.seed_device(
        DeviceProfile::new(12_000_000_000, 8_000_000_000, false, true),
        &["base".to_string()],
    );
    app.open_settings(0.0);
    app.advance(10.0);
    app
}

#[test]
fn settings_rows_follow_the_scroll_offset() {
    let mut h = Harness::new(overflowing_settings());
    h.frame(); // the first paint registers the widgets
    h.frame();

    let bands = chrome::bands(h.screen, h.app.insets.top, h.app.insets.bottom, None);
    let language = h.control("Language");
    assert!(
        (language.top() - (bands.content.top() + 32.0)).abs() < 0.5,
        "at offset 0 the rows must sit exactly where the static-top layout put \
         them (the golden shots pin this): {:?} vs {}",
        language,
        bands.content.top() + 32.0
    );
    let note = h.control(compact::settings::DEVICE_NOTE);

    // The one-shot offset is the shell-facing affordance; the same code path a
    // drag drives.
    h.app.settings_scroll = Some(120.0);
    h.frame();
    h.frame();

    let language_scrolled = h.control("Language");
    let note_scrolled = h.control(compact::settings::DEVICE_NOTE);
    let moved = language.top() - language_scrolled.top();
    assert!(
        (moved - 120.0).abs() < 2.0,
        "the rows must move with the scroll offset, moved {moved}"
    );
    assert!(
        ((note.top() - note_scrolled.top()) - moved).abs() < 0.5,
        "every row moves together"
    );
    assert!(
        note_scrolled.bottom() <= bands.content.bottom(),
        "the DEVICE card is inside the viewport once scrolled"
    );
}

#[test]
fn a_wobble_tap_still_toggles_the_row() {
    let mut h = Harness::new(overflowing_settings());
    h.frame();
    h.frame();
    let row = h.control("Keep the recording");
    assert!(h.app.settings.keep_audio, "seeded on");

    let start = row.center();
    h.touch(1, egui::TouchPhase::Start, start);
    // A thumb rolls 10 pt between press and release; egui's 6 pt mouse default
    // would drop this tap. 12 pt keeps it.
    h.touch(1, egui::TouchPhase::Move, start + egui::vec2(6.0, 0.0));
    h.touch(1, egui::TouchPhase::Move, start + egui::vec2(10.0, 2.0));
    let actions = h.touch(1, egui::TouchPhase::End, start + egui::vec2(10.0, 2.0));

    assert!(
        actions.contains(&compact::CompactAction::Setting(
            compact::CompactSetting::KeepAudio(false)
        )),
        "a 10 pt wobble is still a tap: {actions:?}"
    );
    assert!(!h.app.settings.keep_audio, "the toggle fired");
}

#[test]
fn a_vertical_drag_on_a_row_scrolls_without_toggling() {
    let mut h = Harness::new(overflowing_settings());
    h.frame();
    h.frame();
    let row = h.control("Keep the recording");
    let language = h.control("Language");
    let start = row.center();

    h.touch(1, egui::TouchPhase::Start, start);
    let mut actions = Vec::new();
    for step in 1..=5 {
        actions.extend(h.touch(
            1,
            egui::TouchPhase::Move,
            start - egui::vec2(0.0, 20.0 * step as f32),
        ));
    }
    actions.extend(h.touch(1, egui::TouchPhase::End, start - egui::vec2(0.0, 100.0)));

    assert!(
        h.app.settings.keep_audio,
        "a drag that starts on a row must not toggle it: {actions:?}"
    );
    assert!(
        !actions.iter().any(|action| matches!(
            action,
            compact::CompactAction::Setting(compact::CompactSetting::KeepAudio(_))
        )),
        "no toggle action from a drag: {actions:?}"
    );
    let moved = language.top() - h.control("Language").top();
    assert!(
        moved > 60.0,
        "the 100 pt drag scrolls the list, moved {moved} pt"
    );
}

#[test]
fn a_cancelled_touch_never_toggles_the_row() {
    let mut h = Harness::new(overflowing_settings());
    h.frame();
    h.frame();
    let row = h.control("Keep the recording");

    h.touch(1, egui::TouchPhase::Start, row.center());
    // The system takes the gesture away (back-swipe / shade): cancel, never up.
    h.touch(1, egui::TouchPhase::Cancel, row.center());
    assert!(
        h.app.settings.keep_audio,
        "a cancelled touch must never click whatever sat under the finger"
    );
    // And the pointer must not be left *down*: a stranded primary button is the
    // bug egui-winit's own cancel path still has (it drops the id, not the
    // press). A later tap on the same row must work from a clean state.
    let (down, has_pointer) = h
        .ctx
        .input(|input| (input.pointer.any_down(), input.pointer.has_pointer()));
    assert!(!down, "the cancel released the pointer");
    assert!(!has_pointer, "PointerGone cleared the hover state");

    let row = h.control("Keep the recording");
    h.touch(2, egui::TouchPhase::Start, row.center());
    let actions = h.touch(2, egui::TouchPhase::End, row.center());
    assert!(
        actions.contains(&compact::CompactAction::Setting(
            compact::CompactSetting::KeepAudio(false)
        )),
        "the next tap after a cancel still fires: {actions:?}"
    );
}

#[test]
fn the_device_note_clears_a_54_px_bottom_inset() {
    let mut app = overflowing_settings();
    // The Pixel-class gesture bar, in points: 54 physical px at scale 2.6.
    let inset = 54.0 / 2.6;
    app.insets.bottom = inset;
    let mut h = Harness::new(app);
    h.frame();
    h.frame();

    // Scroll to the end: a big one-shot offset is clamped to the content's
    // height, so the next frame paints the true bottom.
    h.app.settings_scroll = Some(100_000.0);
    h.frame();
    h.frame();

    let bands = chrome::bands(h.screen, h.app.insets.top, h.app.insets.bottom, None);
    let note = h.control(compact::settings::DEVICE_NOTE);
    let quit = h.control("Quit Suramak");
    let tail = compact::settings::bottom_clearance(inset);
    assert!(
        (tail - (inset + chrome::GUTTER)).abs() < 0.01,
        "inset + gutter"
    );
    assert!(
        note.bottom() <= bands.content.bottom() - tail,
        "the DEVICE note clears the gesture area by the padding: note {} content {} tail {}",
        note.bottom(),
        bands.content.bottom(),
        tail
    );
    assert!(
        (bands.content.bottom() - quit.bottom() - (30.0 + tail)).abs() < 1.0,
        "the scroll range ends with the inset + gutter under the last control: \
         quit bottom {} content bottom {}",
        quit.bottom(),
        bands.content.bottom()
    );
    assert!(quit.bottom() < bands.content.bottom());

    let shot = h.shot("settings-bottom-inset");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
}
