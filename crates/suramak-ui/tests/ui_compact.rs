//! The phone-viewport screenshot suite (M13g1).
//!
//! Runs the compact profile at the plan's portrait viewport (1216 × 2640
//! physical, scale 2.6 ⇒ 467.7 × 1015.4 points), rasterises the real egui shape
//! stream with [`suramak_ui::deck::snapshot`], writes the three shots under
//! `tests/ui-compact/shots/`, and asserts the layout rules the profile promises:
//! 48 dp targets, nothing outside the viewport, the bottom bar clearing the
//! gesture inset, and the same three screens reachable by push/pop.
//!
//! The suite is display-free by construction (the rasteriser owns its pixels),
//! but `tests/ui-compact/run.sh` invokes it under `xvfb-run` exactly like the
//! desktop smoke so CI has one harness family and no GPU variance in the goldens.

use suramak_core::device::DeviceProfile;
use suramak_core::settings::Appearance;
use suramak_core::transcript::{Segment, Source, Transcript};
use suramak_ui::deck::compact::{
    self, chrome, library, CompactApp, CompactRoute, CompactSettings, Insets,
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

fn render(app: &mut CompactApp, name: &str) -> snapshot::Shot {
    let (_, _, scale) = compact::phone_viewport();
    let screen = compact::phone_screen_rect();
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::install(&ctx, app.theme, app.reduce_motion);
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
fn reduce_motion_settles_the_push_on_the_first_frame() {
    let mut app = seeded_app();
    app.reduce_motion = true;
    let target = app.transcripts[0].clone();
    app.open_editor(target, 0.0);
    assert!(!app.advance(0.0), "a hard cut asks for no extra frame");
    assert!(app.nav.motion().expect("push").is_done());
    assert_eq!(app.nav.current(), CompactRoute::Editor);
}

#[test]
fn the_light_ladder_renders_too() {
    let mut app = seeded_app();
    app.theme = AppTheme::Light;
    let shot = render(&mut app, "library-light");
    assert!(shot.painted_fraction(egui::Color32::BLACK) > 0.5);
}
