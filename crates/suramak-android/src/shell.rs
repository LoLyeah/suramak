//! The compact product shell (M13g3): the visible phone surface.
//!
//! The M13b spike answered four questions with four tabs. With the questions
//! answered, the product's launch screen is the **compact Library**: this
//! module draws `suramak_ui::deck::compact::ui` into the GameActivity window,
//! backed by the one `CompactApp` the JNI host owns (`suramak_android::
//! compact_host`). The four checks did not disappear — their counters and the
//! v2.4 crash banner are the Settings → Diagnostics card, fed from the same
//! [`crate::ui::SpikeState`] and [`crate::diag`] the spike used.
//!
//! **One state machine, two callers.** Kotlin's `UiBridge` ops (deep link,
//! back, export) and this shell both address the same `CompactApp` behind
//! `CompactHost::with_app`. A tap on the shell therefore cannot disagree with
//! a notification landing or a back gesture.
//!
//! **What Kotlin still owns** is only what Android forbids Rust to do: the
//! `ACTION_CREATE_DOCUMENT` picker, the share chooser, the SAF picker, the
//! foreground services and Activity finish. The shell asks for those with an
//! explicit `Intent` to `MainActivity` (the Application context from
//! `ndk-context` is enough for `startActivity` with `NEW_TASK`; the Activity is
//! `singleTask`, so `onNewIntent` is the one handler). No Rust code calls an
//! Activity method — the v2.3 lesson stands.
//!
//! **The transport.** The compact editor's playhead is a clock in `CompactApp`
//! (`advance`): the word chips follow it today and an audio host can drive the
//! same `on_playhead` later. There is no audio engine in this build; the report
//! names that boundary rather than pretending the speaker is playing.

use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, RawInput};
use jni::objects::{JObject, JValue};
use jni::JavaVM;

use suramak_ui::deck::compact::{
    self, CompactAction, CompactRoute, CompactSetting, DiagnosticsView,
};

use suramak_android_spike::diag;
use suramak_android_spike::ui::SpikeState;

/// How often the model job's progress is read while a card is on screen.
const DOWNLOAD_POLL: Duration = Duration::from_millis(500);

/// The intent actions `MainActivity` answers (constants mirrored there).
const ACTION_IMPORT: &str = "app.suramak.action.COMPACT_IMPORT";
const ACTION_RECORD: &str = "app.suramak.action.COMPACT_RECORD";
const ACTION_STOP_RECORDING: &str = "app.suramak.action.COMPACT_STOP_RECORDING";
const ACTION_EXPORT: &str = "app.suramak.action.COMPACT_EXPORT";
const ACTION_SHARE: &str = "app.suramak.action.COMPACT_SHARE";
const ACTION_DOWNLOAD: &str = "app.suramak.action.COMPACT_DOWNLOAD";
const ACTION_QUIT: &str = "app.suramak.action.COMPACT_QUIT";
const ACTION_FINISH: &str = "app.suramak.action.COMPACT_FINISH";
const ACTION_SHARE_CRASH: &str = "app.suramak.action.COMPACT_SHARE_CRASH";

/// One intent extra (only the shapes the compact verbs need).
enum Extra<'a> {
    Str(&'a str),
    Bool(bool),
}

/// The window's compact surface, owned by the frame loop.
pub struct CompactShell {
    /// The IME buffer already folded into egui events (prefix diffing).
    ime_applied: String,
    /// The last model-progress poll.
    last_poll: Option<Instant>,
    /// The last microphone-status poll.
    last_mic_poll: Option<Instant>,
    /// The compact host was created after this shell's first frame.
    started_host: bool,
    logged_library_ready: bool,
    logged_host_missing: bool,
    /// The crash revision the diagnostics view was built from.
    crash_rev: u64,
    /// A status line the shell owns (a refused intent, a missing host).
    shell_status: Option<String>,
    /// M19A: primary-touch → pointer synthesis, across frames.
    touch_pointer: compact::input::TouchPointer,
    /// M19A: the raw-input log (`SURAMAK_INPUT_LOG`, on in debug builds).
    input_log: bool,
    /// The surface's pixels-per-point, for the input log's units.
    ppp: f32,
    /// The route the last frame painted (the navigation marker's edge).
    last_route: Option<CompactRoute>,
    /// When the Settings layout probe was last logged.
    last_layout_log: Option<Instant>,
    /// The resume counter the touch tracker was last reset for.
    last_resumes: u64,
}

impl Default for CompactShell {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactShell {
    pub fn new() -> Self {
        Self {
            ime_applied: String::new(),
            last_poll: None,
            last_mic_poll: None,
            started_host: false,
            logged_library_ready: false,
            logged_host_missing: false,
            crash_rev: u64::MAX,
            shell_status: None,
            touch_pointer: compact::input::TouchPointer::new(),
            input_log: compact::input::input_log_enabled(),
            ppp: 0.0,
            last_route: None,
            last_layout_log: None,
            last_resumes: 0,
        }
    }

    /// Folds the soft keyboard's buffer into this frame's input events.
    ///
    /// `winit` 0.30.13 consumes android-activity's `TextEvent` and drops it, so
    /// the Kotlin `stateChanged` bridge is the only source of soft-keyboard
    /// text (`platform::take_ime_text`). The spike applied it to a probe field;
    /// the product surface needs it in egui's own input queue, so the delta
    /// against the last buffer becomes `Event::Text` / Backspace. A selection
    /// replacement is treated as a suffix rewrite: the common prefix stays and
    /// the rest is backspaced then retyped (egui's text widgets then run their
    /// normal caret logic).
    pub fn prime_input(&mut self, raw: &mut RawInput, state: &SpikeState) {
        let target = state.text.as_str();
        if target != self.ime_applied {
            let old: Vec<char> = self.ime_applied.chars().collect();
            let new: Vec<char> = target.chars().collect();
            let mut common = 0;
            while common < old.len() && common < new.len() && old[common] == new[common] {
                common += 1;
            }
            for _ in common..old.len() {
                raw.events.push(Event::Key {
                    key: Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
            }
            if common < new.len() {
                raw.events
                    .push(Event::Text(new[common..].iter().collect::<String>()));
            }
            self.ime_applied = target.to_string();
        }

        // M19A Step 0 / the synthesis. The description is taken BEFORE the
        // fold-in so it reports what the integration delivered (the question
        // Step 0 asks), with `synth=` naming what this shell had to add.
        let described = self
            .input_log
            .then(|| compact::input::describe_frame(raw, self.ppp));
        let synthesized = self.touch_pointer.prepare(raw);
        if let Some(line) = described {
            log::info!("SURAMAK-INPUT {line} synth={synthesized}");
        }
    }

    /// Draws one frame. Returns the actions the host must run ([] when the
    /// compact host has not been created yet).
    pub fn draw(&mut self, ctx: &egui::Context, state: &SpikeState) -> Vec<CompactAction> {
        // M19A: the phone's click window is thumb-sized (12 pt / 0.5 s). The
        // check is a read, so a recreated context — a surface rebuild builds a
        // fresh egui Context — self-heals on the next frame.
        if !compact::input::touch_options_applied(ctx) {
            compact::input::apply_touch_options(ctx);
        }
        self.ppp = ctx.pixels_per_point();
        // A touch that was live when the Activity went away never gets its
        // End/Cancel; holding the old id would make every later Start look like
        // a second finger. A resume is the edge that proves it is gone.
        if state.resumes != self.last_resumes {
            self.last_resumes = state.resumes;
            self.touch_pointer.reset();
        }
        let Some(host) = crate::compact_host() else {
            // One line per generation: the host is registered by MainActivity's
            // UiBridge.init BEFORE super.onCreate, so a persistent None here
            // means the bridge reply was not ok (its text is in logcat) — a
            // silent placeholder once hid exactly that on the CI emulator.
            if !self.logged_host_missing {
                self.logged_host_missing = true;
                log::warn!(
                    "compact shell: the host is not initialised yet — drawing the startup placeholder (see MainActivity's 'compact UI host:' line for the init reply)"
                );
            }
            draw_starting(ctx);
            self.shell_status = Some("The compact host is still starting.".to_string());
            return Vec::new();
        };
        let now = ctx.input(|input| input.time);
        let ppp = ctx.pixels_per_point().max(0.1);
        let diagnostics = self.diagnostics(state);
        // Read the resume offer BEFORE taking the app lock: `download_state`
        // locks the same host the draw is about to borrow.
        let resume = if self.started_host {
            None
        } else {
            read_resume_offer()
        };
        let mut actions = Vec::new();
        let mut layout_probe = None;
        let drew = host.with_app(|app| {
            app.insets.top = (state.insets.top as f32 / ppp).max(0.0);
            app.insets.bottom = (state.insets.bottom as f32 / ppp).max(0.0);
            app.reduce_motion = platform_reduce_motion();
            app.set_diagnostics(diagnostics);
            if !self.started_host {
                self.started_host = true;
                // A `.part` from a killed run becomes the resume offer at the
                // same percent the job will start from.
                if let Some(resume) = resume {
                    app.on_download_resume(
                        resume.model,
                        resume.bytes_done,
                        resume.bytes_total,
                        resume.percent,
                    );
                }
                app.maybe_begin_onboarding(now);
            }
            if let Some(status) = self.shell_status.take() {
                app.status = Some((status, true));
            }
            actions = compact::ui(ctx, app);
            // M19A: one logcat line per navigation change is the smoke's proof
            // that a tap landed (`compact route: Settings`), and the Settings
            // probe below is its bottom-clearance evidence.
            let route = app.nav.current();
            if self.last_route != Some(route) {
                self.last_route = Some(route);
                log::info!("compact route: {route:?}");
            }
            if self.input_log {
                layout_probe = Some((
                    compact::a11y::controls(ctx)
                        .into_iter()
                        .find(|control| control.label == compact::settings::DEVICE_NOTE)
                        .map(|control| control.rect),
                    app.insets.bottom,
                ));
            }
        });
        if drew.is_none() {
            draw_starting(ctx);
            return Vec::new();
        }
        self.log_layout_probe(ctx, layout_probe);
        // One logcat marker for the first real Library draw (the CI smoke
        // waits for it before screencapping: the startup placeholder and the
        // Library look alike in a pixel histogram, and the host's first draw
        // on a slow software-rendered emulator can lag the first GPU frame
        // by tens of seconds).
        if !self.logged_library_ready {
            self.logged_library_ready = true;
            log::info!("compact library surface: ready");
        }
        actions
    }

    /// One line a second while Settings is up, naming where the DEVICE note
    /// ended up relative to the content band and the gesture inset (M19A's
    /// bottom-clearance proof; the emulator smoke reads the last line).
    fn log_layout_probe(&mut self, ctx: &egui::Context, probe: Option<(Option<egui::Rect>, f32)>) {
        let Some((Some(rect), inset)) = probe else {
            return;
        };
        let now = Instant::now();
        if self
            .last_layout_log
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(1))
        {
            return;
        }
        self.last_layout_log = Some(now);
        let screen = ctx.content_rect();
        log::info!(
            "SURAMAK-LAYOUT route=Settings device_top={:.1} device_bottom={:.1} \
             content_bottom={:.1} screen_bottom={:.1} inset_bottom={:.1}",
            rect.top(),
            rect.bottom(),
            screen.bottom() - inset,
            screen.bottom(),
            inset
        );
    }

    /// Runs the actions the last frame produced, after the app borrow has ended.
    pub fn dispatch(&mut self, actions: Vec<CompactAction>) {
        for action in actions {
            self.dispatch_one(action);
        }
        self.poll_download();
        self.poll_recording();
    }

    fn dispatch_one(&mut self, action: CompactAction) {
        let host = match crate::compact_host() {
            Some(host) => host,
            None => return,
        };
        match action {
            CompactAction::OpenTranscript { id, segment } => {
                let reply = host.dispatch(
                    &serde_json::json!({
                        "op": "open_transcript",
                        "transcript_id": id.to_string(),
                        "segment": segment,
                    })
                    .to_string(),
                );
                log_reply("open transcript", &reply);
            }
            CompactAction::PlayTranscript(id) => {
                let reply = host.dispatch(
                    &serde_json::json!({
                        "op": "open_transcript",
                        "transcript_id": id.to_string(),
                    })
                    .to_string(),
                );
                log_reply("play transcript", &reply);
                host.with_app(|app| app.start_playback());
            }
            CompactAction::ReloadLibrary => {
                let reply = host.dispatch(r#"{"op":"library"}"#);
                log_reply("reload library", &reply);
            }
            CompactAction::SaveSegment(edit) => {
                let reply = host.dispatch(
                    &serde_json::json!({
                        "op": "save_segment",
                        "idx": edit.idx,
                        "text": edit.text,
                    })
                    .to_string(),
                );
                log_reply("save segment", &reply);
            }
            CompactAction::Setting(setting) => self.apply_setting(host, setting),
            CompactAction::DownloadModel(model) => {
                send_intent(
                    ACTION_DOWNLOAD,
                    &[
                        ("model", Extra::Str(&model)),
                        ("language", Extra::Str("auto")),
                    ],
                );
                self.last_poll = None;
            }
            CompactAction::CancelDownload => {
                let reply = crate::host().dispatch(r#"{"op":"cancel"}"#);
                log_reply("cancel download", &reply);
            }
            CompactAction::Import => send_intent(ACTION_IMPORT, &[]),
            CompactAction::Record => send_intent(ACTION_RECORD, &[]),
            CompactAction::StopRecording => send_intent(ACTION_STOP_RECORDING, &[]),
            CompactAction::Export {
                format, bilingual, ..
            } => send_intent(
                ACTION_EXPORT,
                &[
                    ("format", Extra::Str(format.extension())),
                    ("bilingual", Extra::Bool(bilingual)),
                ],
            ),
            CompactAction::ShareExport {
                format, bilingual, ..
            } => send_intent(
                ACTION_SHARE,
                &[
                    ("format", Extra::Str(format.extension())),
                    ("bilingual", Extra::Bool(bilingual)),
                ],
            ),
            CompactAction::OnboardingSeen { skipped } => {
                let reply = host.dispatch(
                    &serde_json::json!({
                        "op": "setting",
                        "setting": "first_run_seen",
                        "value": true,
                    })
                    .to_string(),
                );
                log_reply("dismiss intro", &reply);
                log::info!("intro dismissed (skipped={skipped})");
            }
            CompactAction::ClearCrash => match diag::clear_crash() {
                Ok(()) => log::info!("crash record cleared from Diagnostics"),
                Err(err) => log::warn!("crash clear failed: {err}"),
            },
            CompactAction::ShareCrashLog => send_intent(ACTION_SHARE_CRASH, &[]),
            CompactAction::Quit => send_intent(ACTION_QUIT, &[]),
            CompactAction::Finish => send_intent(ACTION_FINISH, &[]),
            CompactAction::Playback { playing } => {
                log::info!("transport: playing={playing} (word chips follow the preview clock)");
            }
            CompactAction::Seek { position_ms } => {
                log::info!("transport: seek {position_ms} ms (word chip / segment)");
            }
            other => log::info!("compact action not wired on this build: {other:?}"),
        }
    }

    fn apply_setting(&mut self, host: &crate::compact_host::CompactHost, setting: CompactSetting) {
        let (name, value) = match setting {
            CompactSetting::Model(id) => ("model", serde_json::Value::String(id)),
            CompactSetting::Language(language) => ("language", serde_json::Value::String(language)),
            CompactSetting::Appearance(appearance) => (
                "appearance",
                serde_json::Value::String(match appearance {
                    suramak_core::settings::Appearance::Light => "light".to_string(),
                    suramak_core::settings::Appearance::Dark => "dark".to_string(),
                    suramak_core::settings::Appearance::System => "system".to_string(),
                }),
            ),
            CompactSetting::KeepAudio(keep) => ("keep_audio", serde_json::Value::Bool(keep)),
            CompactSetting::FirstRunSeen(seen) => ("first_run_seen", serde_json::Value::Bool(seen)),
        };
        let reply = host.dispatch(
            &serde_json::json!({
                "op": "setting",
                "setting": name,
                "value": value,
            })
            .to_string(),
        );
        log_reply("setting", &reply);
    }

    /// The mic host's recording state, so the Library's Record becomes Stop
    /// while a take is live. Half-second poll; a repeated value is a no-op in
    /// `CompactApp::set_recording`.
    fn poll_recording(&mut self) {
        let now = Instant::now();
        if self
            .last_mic_poll
            .is_some_and(|last| now.duration_since(last) < DOWNLOAD_POLL)
        {
            return;
        }
        self.last_mic_poll = Some(now);
        let Some(host) = crate::mic_host() else {
            return;
        };
        let reply = host.dispatch(r#"{"op":"status"}"#);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply) else {
            return;
        };
        if let Some(recording) = value.get("recording").and_then(|v| v.as_bool()) {
            if let Some(compact) = crate::compact_host() {
                compact.with_app(|app| app.set_recording(recording));
            }
        }
    }

    /// The model job's progress, read from the same host the notification's
    /// `ModelDownloadJobService` polls. One poll per half second while a card
    /// is up; the repeated percent is a no-op in `CompactApp::on_download`.
    fn poll_download(&mut self) {
        let host = match crate::compact_host() {
            Some(host) => host,
            None => return,
        };
        let needs = host.with_app(|app| app.model_card_needed() || app.download.is_some());
        if !needs.unwrap_or(false) {
            return;
        }
        let now = Instant::now();
        if self
            .last_poll
            .is_some_and(|last| now.duration_since(last) < DOWNLOAD_POLL)
        {
            return;
        }
        self.last_poll = Some(now);
        let reply = crate::host().dispatch(r#"{"op":"progress"}"#);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&reply) else {
            return;
        };
        let state_name = value
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("idle");
        let model = value
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let done = value
            .get("bytes_done")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total = value
            .get("bytes_total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let percent = value.get("percent").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if model.is_empty() || total == 0 {
            return;
        }
        let now = 0.0;
        match state_name {
            "running" => {
                host.with_app(|app| {
                    app.on_download(
                        model,
                        done,
                        total,
                        percent,
                        compact::DownloadState::Running,
                        message,
                        now,
                    );
                });
            }
            "done" => {
                host.with_app(|app| {
                    app.on_download(
                        model,
                        total,
                        total,
                        100,
                        compact::DownloadState::Done,
                        "sha256 verified",
                        now,
                    );
                });
                let reply = host.dispatch(r#"{"op":"refresh_models"}"#);
                log_reply("download complete", &reply);
            }
            "cancelled" => {
                host.with_app(|app| {
                    app.on_download_resume(model, done, total, percent);
                });
            }
            "failed" => {
                host.with_app(|app| {
                    app.on_download(
                        model,
                        done,
                        total,
                        percent,
                        compact::DownloadState::Failed,
                        message,
                        now,
                    );
                });
            }
            _ => {}
        }
    }

    /// The four-check snapshot: every counter the spike's tabs carried, plus
    /// the v2.4 crash record.
    fn diagnostics(&mut self, state: &SpikeState) -> DiagnosticsView {
        let rev = diag::crash_revision();
        let crash = if rev != self.crash_rev {
            self.crash_rev = rev;
            diag::crash_for_display()
        } else {
            // A cheap re-read keeps the banner correct without caching a stale
            // copy in the shell (the record is one string).
            diag::crash_for_display()
        };
        DiagnosticsView {
            text_chars: state.chars_received,
            ime_updates: state.ime_updates,
            deletions: state.deletions_seen,
            resumes: state.resumes,
            surface_recreates: state.surface_recreates,
            frames_gated: state.frames_gated,
            frames_after_destroy: state.frames_after_destroy,
            probe_saves: state.probe_saves,
            probe_restores: state.probe_restores,
            avg_fps: state.avg_fps(),
            frames: state.frames,
            inset_updates: state.insets.updates,
            inset_top: (state.insets.top as f32 / 2.6).max(0.0),
            inset_bottom: (state.insets.bottom as f32 / 2.6).max(0.0),
            back_presses: state.back_presses,
            crash: crash.map(|view| compact::CrashBanner {
                live: view.live,
                text: view.text,
            }),
        }
    }
}

/// A resume offer read from `CompactHost::download_state`.
struct ResumeOffer {
    model: String,
    bytes_done: u64,
    bytes_total: u64,
    percent: u8,
}

fn read_resume_offer() -> Option<ResumeOffer> {
    let reply = crate::compact_host()?.dispatch(r#"{"op":"download_state"}"#);
    let value: serde_json::Value = serde_json::from_str(&reply).ok()?;
    if !value
        .get("known")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || value
            .get("installed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return None;
    }
    let bytes_done = value
        .get("bytes_done")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if bytes_done == 0 {
        return None;
    }
    Some(ResumeOffer {
        model: value.get("model")?.as_str()?.to_string(),
        bytes_done,
        bytes_total: value.get("bytes_total")?.as_u64()?,
        percent: value.get("percent").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
    })
}

/// Whether the OS asked for reduced motion (the Kotlin shim's animator scale
/// is 0, or the accessibility toggle is on). Read through `platform`.
fn platform_reduce_motion() -> bool {
    suramak_android_spike::platform::reduce_motion()
}

fn draw_starting(ctx: &egui::Context) {
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(
        screen,
        egui::CornerRadius::ZERO,
        egui::Color32::from_rgb(11, 11, 14),
    );
    painter.text(
        egui::pos2(screen.center().x, screen.top() + screen.height() * 0.44),
        egui::Align2::CENTER_CENTER,
        "Suramak",
        egui::FontId::proportional(24.0),
        egui::Color32::from_rgb(220, 224, 232),
    );
    painter.text(
        egui::pos2(
            screen.center().x,
            screen.top() + screen.height() * 0.44 + 28.0,
        ),
        egui::Align2::CENTER_CENTER,
        "Opening the library…",
        egui::FontId::proportional(13.0),
        egui::Color32::from_rgb(150, 154, 164),
    );
}

fn log_reply(label: &str, reply: &str) {
    let ok = serde_json::from_str::<serde_json::Value>(reply)
        .ok()
        .and_then(|value| value.get("ok").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    if ok {
        log::info!("compact {label}: ok");
    } else {
        log::warn!("compact {label}: {reply}");
    }
}

/// Sends one explicit intent to `MainActivity` (Application context,
/// `FLAG_ACTIVITY_NEW_TASK`; the Activity is `singleTask`, so `onNewIntent`
/// receives it). Every failure is a log line — the UI's own state is intact.
fn send_intent(action: &str, extras: &[(&str, Extra<'_>)]) {
    if let Err(err) = send_intent_inner(action, extras) {
        log::warn!("compact intent {action} failed: {err}");
    }
}

fn send_intent_inner(action: &str, extras: &[(&str, Extra<'_>)]) -> Result<(), String> {
    let context = ndk_context::android_context();
    // SAFETY: ndk-context holds the JavaVM pointer for the process lifetime.
    let vm =
        unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|err| format!("JavaVM: {err}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|err| format!("attach: {err}"))?;
    let context = unsafe { JObject::from_raw(context.context().cast()) };
    let action_string = env
        .new_string(action)
        .map_err(|err| format!("action string: {err}"))?;
    let intent = env
        .new_object(
            "android/content/Intent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action_string)],
        )
        .map_err(|err| format!("Intent: {err}"))?;
    let package = env
        .new_string("app.suramak.spike")
        .map_err(|err| format!("package: {err}"))?;
    let class = env
        .new_string("app.suramak.spike.MainActivity")
        .map_err(|err| format!("class: {err}"))?;
    env.call_method(
        &intent,
        "setClassName",
        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&package), JValue::Object(&class)],
    )
    .map_err(|err| format!("setClassName: {err}"))?;
    // FLAG_ACTIVITY_NEW_TASK (0x10000000): an Application context may start an
    // Activity only with it; `singleTask` then delivers via onNewIntent.
    env.call_method(
        &intent,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(0x1000_0000)],
    )
    .map_err(|err| format!("addFlags: {err}"))?;
    for (key, value) in extras {
        let key = env
            .new_string(key)
            .map_err(|err| format!("extra key: {err}"))?;
        match value {
            Extra::Str(text) => {
                let text = env
                    .new_string(text)
                    .map_err(|err| format!("extra value: {err}"))?;
                env.call_method(
                    &intent,
                    "putExtra",
                    "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                    &[JValue::Object(&key), JValue::Object(&text)],
                )
                .map_err(|err| format!("putExtra: {err}"))?;
            }
            Extra::Bool(flag) => {
                env.call_method(
                    &intent,
                    "putExtra",
                    "(Ljava/lang/String;Z)Landroid/content/Intent;",
                    &[JValue::Object(&key), JValue::Bool(*flag as u8)],
                )
                .map_err(|err| format!("putExtra: {err}"))?;
            }
        }
    }
    env.call_method(
        &context,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intent)],
    )
    .map_err(|err| format!("startActivity: {err}"))?;
    Ok(())
}

impl suramak_android_spike::surface_api::CompactSurface for CompactShell {
    fn prime_input(&mut self, raw: &mut RawInput, state: &SpikeState) {
        CompactShell::prime_input(self, raw, state);
    }

    fn draw(&mut self, ctx: &egui::Context, state: &SpikeState) -> Vec<CompactAction> {
        CompactShell::draw(self, ctx, state)
    }

    fn dispatch(&mut self, actions: Vec<CompactAction>) {
        CompactShell::dispatch(self, actions);
    }
}
