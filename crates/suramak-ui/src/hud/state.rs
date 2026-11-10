//! The Compact HUD's one-window state machine (M16B).
//!
//! Pure logic, no egui and no AppKit: every transition is unit-tested headless
//! (the macOS-only window/renderer lives in [`super::window`], the platform
//! adapters in [`super::panel`]).
//!
//! ## The six states and their invariants
//!
//! | state | window | capture | repaint |
//! |---|---|---|---|
//! | `Idle` | hidden | none | none (the M9 budget rule) |
//! | `Recording` | visible | live | 100 ms: the living dot + elapsed |
//! | `Processing` | visible | finalized | 120 ms: progress |
//! | `Result` | visible | none | none (a static card) |
//! | `Cancelled` | visible | none | 100 ms until it auto-hides |
//! | `PermissionFailure` | visible | none | none |
//!
//! The **always-visible recording indicator** is a structural invariant: the
//! only state with a live capture is `Recording`, and `Recording` is always
//! visible. Closing/dismissing the HUD while recording does not merely hide the
//! window — it emits [`HudAction::AbortCapture`], so the M6 capture session is
//! stopped and dropped exactly once (no leaked microphone, no invisible
//! recording). The main window's floating pill remains the second indicator.
//!
//! ## Why the machine owns the actions
//!
//! The UI thread executes [`HudAction`]s (start/stop the existing M6 capture
//! path, cancel the job, write the clipboard, post ⌘V, open System Settings).
//! Keeping the decisions here means the tests prove the capture lifecycle even
//! though the real devices, Carbon and AppKit only exist on macOS.

use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::app::{Job, RecordState, SaveOutcome};
use suramak_core::transcript::Transcript;

use super::settings::InsertionPreference;
use super::InsertError;

/// How often the recording state asks for a frame (the living dot + elapsed).
pub const RECORDING_TICK: Duration = Duration::from_millis(100);
/// How often the processing state asks for a frame (the indeterminate bar).
pub const PROCESSING_TICK: Duration = Duration::from_millis(120);
/// How long a cancelled/refused card stays up before hiding itself.
pub const CANCELLED_DWELL: Duration = Duration::from_millis(1_800);

/// The six named states (the workorder's lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudPhase {
    Idle,
    Recording,
    Processing,
    Result,
    Cancelled,
    PermissionFailure,
}

impl HudPhase {
    /// The user-facing state name (also the accessibility label's prefix).
    pub fn label(self) -> &'static str {
        match self {
            HudPhase::Idle => "Idle",
            HudPhase::Recording => "Recording",
            HudPhase::Processing => "Processing",
            HudPhase::Result => "Result",
            HudPhase::Cancelled => "Cancelled",
            HudPhase::PermissionFailure => "Permission needed",
        }
    }
}

/// Which permission a failure card is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    /// Microphone / audio capture permission (or no usable input at all).
    Microphone,
    /// macOS Accessibility, required for the synthetic ⌘V of Insert.
    Accessibility,
}

/// Live recording sub-state.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingHud {
    pub started_at: Instant,
    /// Mirrors the app's recording clock (updated by `sync_from_app`, so the
    /// HUD and the main window's pill never disagree).
    pub elapsed: Duration,
    /// The rolling RMS (0..=1) for the living dot's breathing.
    pub level: f32,
    /// The runtime is finalizing; a second Stop is a no-op.
    pub stopping: bool,
}

/// Transcribing sub-state.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessingHud {
    pub started_at: Instant,
    /// The transcript id that was already in `last_insert` when this run
    /// started — a result is only accepted when the id differs, so a previous
    /// dictation can never be shown as this run's result.
    pub baseline: Option<Uuid>,
    /// The worker's last reported percentage.
    pub progress: Option<u8>,
    /// The worker's stage label (`decoding`, `transcribing`).
    pub stage: Option<String>,
}

/// The result card.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultHud {
    pub text: String,
    pub transcript: Option<Uuid>,
    /// The text is on the clipboard (Copy, or the copy half of an Insert).
    pub copied: bool,
    /// The synthetic ⌘V was posted.
    pub inserted: bool,
    /// Keep pins the card; Dismiss closes it. Neither deletes the transcript —
    /// the existing in-app flow already saved it to the Library.
    pub kept: bool,
    /// A non-fatal note (Insert failed, permission missing, …).
    pub note: Option<String>,
}

/// A cancelled / refused run.
#[derive(Debug, Clone, PartialEq)]
pub struct CancelledHud {
    pub reason: String,
    /// True when the user cancelled; false for a refusal or an empty result.
    pub by_user: bool,
    pub at: Instant,
}

/// A capture or insertion permission failure, with recovery.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionHud {
    pub kind: PermissionKind,
    pub message: String,
    /// Text preserved from a result whose Insert was refused, so the
    /// permission detour never loses the transcript.
    pub retry_text: Option<String>,
    pub at: Instant,
}

/// The whole HUD state.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum HudState {
    #[default]
    Idle,
    Recording(RecordingHud),
    Processing(ProcessingHud),
    Result(ResultHud),
    Cancelled(CancelledHud),
    PermissionFailure(PermissionHud),
}

impl HudState {
    pub fn idle() -> Self {
        Self::Idle
    }

    pub fn phase(&self) -> HudPhase {
        match self {
            HudState::Idle => HudPhase::Idle,
            HudState::Recording(_) => HudPhase::Recording,
            HudState::Processing(_) => HudPhase::Processing,
            HudState::Result(_) => HudPhase::Result,
            HudState::Cancelled(_) => HudPhase::Cancelled,
            HudState::PermissionFailure(_) => HudPhase::PermissionFailure,
        }
    }

    /// The card's header title. A user cancellation and a refusal read
    /// differently even though both live in the `Cancelled` state.
    pub fn title(&self) -> &'static str {
        match self {
            HudState::Cancelled(cancelled) if !cancelled.by_user => "Not started",
            _ => self.phase().label(),
        }
    }

    /// Whether the HUD window should be on screen.
    pub fn is_visible(&self) -> bool {
        !matches!(self, HudState::Idle)
    }

    /// Whether a capture session is live (the indicator invariant's antecedent).
    pub fn capture_live(&self) -> bool {
        matches!(self, HudState::Recording(_))
    }

    /// The requested repaint interval. `None` is the M9 budget rule: an idle,
    /// result or permission card does not repaint itself.
    pub fn cadence(&self) -> Option<Duration> {
        match self {
            HudState::Recording(_) | HudState::Cancelled(_) => Some(RECORDING_TICK),
            HudState::Processing(_) => Some(PROCESSING_TICK),
            HudState::Idle | HudState::Result(_) | HudState::PermissionFailure(_) => None,
        }
    }

    /// Starts recording from `Idle` (or a terminal card). Returns the capture
    /// action; the UI opens the existing M6 path.
    fn start(&mut self, now: Instant) -> Vec<HudAction> {
        if self.capture_live() {
            return Vec::new();
        }
        *self = HudState::Recording(RecordingHud {
            started_at: now,
            elapsed: Duration::ZERO,
            level: 0.0,
            stopping: false,
        });
        vec![HudAction::StartCapture]
    }

    /// Stops a live recording into `Processing`. `baseline` is the transcript id
    /// already in `last_insert` at this moment (see [`ProcessingHud::baseline`]).
    fn stop(&mut self, now: Instant, baseline: Option<Uuid>) -> Vec<HudAction> {
        let HudState::Recording(recording) = self else {
            return Vec::new();
        };
        if recording.stopping {
            return Vec::new();
        }
        *self = HudState::Processing(ProcessingHud {
            started_at: now,
            baseline,
            progress: None,
            stage: None,
        });
        vec![HudAction::StopCapture]
    }

    /// The global hotkey. Idle → start; Recording → stop; Processing → swallowed
    /// (the chord re-press must never queue a second job); Result → focus the
    /// card (the keyboard-only path to Insert / Copy); Cancelled → a fresh
    /// recording; a permission card → retry, or return to the preserved result.
    pub fn hotkey(&mut self, now: Instant, baseline: Option<Uuid>) -> Vec<HudAction> {
        match self {
            HudState::Idle | HudState::Cancelled(_) => self.start(now),
            HudState::Recording(_) => self.stop(now, baseline),
            HudState::Processing(_) => Vec::new(),
            HudState::Result(_) => vec![HudAction::FocusHud],
            HudState::PermissionFailure(permission) => {
                if permission.kind == PermissionKind::Accessibility {
                    if let Some(text) = permission.retry_text.take() {
                        *self = HudState::Result(ResultHud {
                            text,
                            transcript: None,
                            copied: false,
                            inserted: false,
                            kept: false,
                            note: Some(
                                "Insert needs Accessibility — Copy is ready; retry after grant"
                                    .to_string(),
                            ),
                        });
                        return vec![HudAction::FocusHud];
                    }
                }
                self.start(now)
            }
        }
    }

    /// A button or keyboard intent from the card. `baseline` is the transcript
    /// id already in `last_insert` (see [`ProcessingHud::baseline`]) — the Stop
    /// button needs it exactly like the chord does, or a previous dictation
    /// could be mistaken for this run's result.
    pub fn on_intent(
        &mut self,
        intent: HudIntent,
        chrome: &HudChrome,
        baseline: Option<Uuid>,
        now: Instant,
    ) -> Vec<HudAction> {
        match intent {
            HudIntent::Stop => self.stop(now, baseline),
            HudIntent::Cancel | HudIntent::Dismiss => match self {
                HudState::Recording(_) => {
                    *self = HudState::Cancelled(CancelledHud {
                        reason: "Recording cancelled — the microphone is closed".to_string(),
                        by_user: true,
                        at: now,
                    });
                    vec![HudAction::AbortCapture]
                }
                HudState::Processing(_) => {
                    *self = HudState::Cancelled(CancelledHud {
                        reason: "Transcription cancelled".to_string(),
                        by_user: true,
                        at: now,
                    });
                    vec![HudAction::CancelJob]
                }
                HudState::Result(_) | HudState::Cancelled(_) | HudState::PermissionFailure(_) => {
                    *self = HudState::Idle;
                    Vec::new()
                }
                HudState::Idle => Vec::new(),
            },
            HudIntent::Copy => {
                let HudState::Result(result) = self else {
                    return Vec::new();
                };
                if result.text.is_empty() {
                    return Vec::new();
                }
                result.copied = true;
                result.note = None;
                vec![HudAction::Copy(result.text.clone())]
            }
            HudIntent::Insert => {
                let HudState::Result(result) = self else {
                    return Vec::new();
                };
                if result.text.is_empty() {
                    return Vec::new();
                }
                if !chrome.accessibility {
                    // A race: the button is disabled while the permission is
                    // missing. Preserve the text and explain the way out.
                    let text = result.text.clone();
                    *self = HudState::PermissionFailure(PermissionHud {
                        kind: PermissionKind::Accessibility,
                        message: INSERT_NEEDS_ACCESSIBILITY.to_string(),
                        retry_text: Some(text),
                        at: now,
                    });
                    return vec![HudAction::OpenAccessibilitySettings];
                }
                let text = result.text.clone();
                result.copied = true;
                result.inserted = true;
                result.note = None;
                // Clipboard first, then the keystroke: the UI executes actions
                // in order, so the paste reads the text we own.
                vec![HudAction::Copy(text.clone()), HudAction::Insert(text)]
            }
            HudIntent::Keep => {
                if let HudState::Result(result) = self {
                    result.kept = true;
                }
                Vec::new()
            }
            HudIntent::Retry => match self {
                HudState::PermissionFailure(permission)
                    if permission.kind == PermissionKind::Microphone =>
                {
                    self.start(now)
                }
                HudState::PermissionFailure(permission) => {
                    if let Some(text) = permission.retry_text.take() {
                        *self = HudState::Result(ResultHud {
                            text,
                            transcript: None,
                            copied: false,
                            inserted: false,
                            kept: false,
                            note: None,
                        });
                        vec![HudAction::FocusHud]
                    } else {
                        self.start(now)
                    }
                }
                _ => Vec::new(),
            },
            HudIntent::OpenSettings => match self {
                HudState::PermissionFailure(permission) => match permission.kind {
                    PermissionKind::Microphone => vec![HudAction::OpenMicrophoneSettings],
                    PermissionKind::Accessibility => vec![HudAction::OpenAccessibilitySettings],
                },
                HudState::Result(result) if !chrome.accessibility => {
                    vec![HudAction::OpenAccessibilitySettings]
                }
                _ => Vec::new(),
            },
        }
    }

    /// Attaches a non-fatal note to the result card (a copy/paste failure).
    /// Other states have nowhere to show it; the Settings status carries it.
    pub fn set_note(&mut self, note: impl Into<String>) {
        if let HudState::Result(result) = self {
            result.note = Some(note.into());
        }
    }

    /// Mirrors a recording that was started outside the HUD (the floating pill):
    /// the global chord should attach to it rather than refuse or duplicate it.
    pub fn attach_to_recording(&mut self, elapsed: Duration, level: f32, started_at: Instant) {
        if matches!(self, HudState::Idle | HudState::Cancelled(_)) {
            *self = HudState::Recording(RecordingHud {
                started_at,
                elapsed,
                level: level.clamp(0.0, 1.0),
                stopping: false,
            });
        }
    }

    /// Refuses a start the app cannot honor (a job is already running, capture
    /// is not compiled in). A visible, self-hiding card — never silence.
    pub fn refuse(&mut self, reason: impl Into<String>, now: Instant) {
        *self = HudState::Cancelled(CancelledHud {
            reason: reason.into(),
            by_user: false,
            at: now,
        });
    }

    /// Capture could not start (or finalize). Permission-sounding messages get
    /// the recovery card; everything else is a named cancellation.
    pub fn on_capture_failed(&mut self, message: String, now: Instant) {
        if let Some(kind) = permission_from_message(&message) {
            *self = HudState::PermissionFailure(PermissionHud {
                kind,
                message,
                retry_text: None,
                at: now,
            });
        } else {
            *self = HudState::Cancelled(CancelledHud {
                reason: message,
                by_user: false,
                at: now,
            });
        }
    }

    /// A finished transcript. An empty/whitespace result is a cancellation with
    /// the honest reason ("nothing was heard"), not a blank result card.
    /// With [`InsertionPreference::InsertPrimary`] and Accessibility granted, the
    /// result is copied and inserted immediately.
    pub fn on_transcript(
        &mut self,
        text: String,
        transcript: Option<Uuid>,
        chrome: &HudChrome,
        now: Instant,
    ) -> Vec<HudAction> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            *self = HudState::Cancelled(CancelledHud {
                reason: "Nothing was heard — the recording was silent".to_string(),
                by_user: false,
                at: now,
            });
            return Vec::new();
        }
        let text = trimmed.to_string();
        let mut result = ResultHud {
            text: text.clone(),
            transcript,
            copied: false,
            inserted: false,
            kept: false,
            note: None,
        };
        let mut actions = Vec::new();
        if chrome.insertion == InsertionPreference::InsertPrimary {
            if chrome.accessibility {
                result.copied = true;
                result.inserted = true;
                actions.push(HudAction::Copy(text.clone()));
                actions.push(HudAction::Insert(text));
            } else {
                result.note = Some(INSERT_NEEDS_ACCESSIBILITY.to_string());
            }
        }
        *self = HudState::Result(result);
        actions
    }

    /// The platform's answer to an Insert action.
    pub fn on_insert_result(
        &mut self,
        result: Result<(), InsertError>,
        now: Instant,
    ) -> Vec<HudAction> {
        let HudState::Result(card) = self else {
            return Vec::new();
        };
        match result {
            Ok(()) => {
                card.inserted = true;
                card.note = None;
                Vec::new()
            }
            Err(InsertError::PermissionDenied) => {
                let text = card.text.clone();
                *self = HudState::PermissionFailure(PermissionHud {
                    kind: PermissionKind::Accessibility,
                    message: INSERT_NEEDS_ACCESSIBILITY.to_string(),
                    retry_text: Some(text),
                    at: now,
                });
                vec![HudAction::OpenAccessibilitySettings]
            }
            Err(err) => {
                card.inserted = false;
                card.note = Some(err.to_string());
                Vec::new()
            }
        }
    }

    /// Pulls the HUD back in line with the app's own recording/job state. This
    /// is how a capture failure (the runtime's `record_failed`) and a finished
    /// transcript (the runtime's `on_transcript_saved`) reach the card.
    pub fn sync_from_app(
        &mut self,
        record: &RecordState,
        job: &Job,
        last_insert: Option<&SaveOutcome>,
        chrome: &HudChrome,
        now: Instant,
    ) -> Vec<HudAction> {
        match self {
            HudState::Recording(recording) => match record {
                RecordState::Recording(live) => {
                    recording.elapsed = live.elapsed;
                    recording.level = live.rms.clamp(0.0, 1.0);
                    recording.stopping = live.stopping;
                    Vec::new()
                }
                RecordState::Failed { message } => {
                    self.on_capture_failed(message.clone(), now);
                    Vec::new()
                }
                RecordState::Idle => {
                    // A capture that vanished without our Stop — treat it as a
                    // cancellation; there is nothing left to transcribe.
                    *self = HudState::Cancelled(CancelledHud {
                        reason: "The recording ended before it could be transcribed".to_string(),
                        by_user: false,
                        at: now,
                    });
                    Vec::new()
                }
            },
            HudState::Processing(processing) => {
                if let Some(outcome) = last_insert {
                    let id = outcome.transcript.id;
                    if processing.baseline != Some(id) {
                        let text = transcript_text(&outcome.transcript);
                        return self.on_transcript(text, Some(id), chrome, now);
                    }
                }
                if let Job::Running(run) = job {
                    processing.progress = Some(run.pct);
                    processing.stage = run.stage.map(|stage| match stage {
                        suramak_asr::worker::JobStage::Decoding => "decoding".to_string(),
                        suramak_asr::worker::JobStage::Transcribing => "transcribing".to_string(),
                    });
                }
                if let Job::Failed { message, .. } = job {
                    *self = HudState::Cancelled(CancelledHud {
                        reason: message.clone(),
                        by_user: false,
                        at: now,
                    });
                } else if let RecordState::Failed { message } = record {
                    self.on_capture_failed(message.clone(), now);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Advances the clock and the cancelled card's dwell. Auto-hide is the only
    /// timer in the machine: it ends, so an idle HUD never repaints.
    pub fn tick(&mut self, now: Instant) -> Vec<HudAction> {
        match self {
            // Recording's clock is the app's own (`sync_from_app` mirrors it):
            // the card and the floating pill must never disagree.
            HudState::Recording(_) => Vec::new(),
            HudState::Cancelled(cancelled) => {
                if now.saturating_duration_since(cancelled.at) >= CANCELLED_DWELL {
                    *self = HudState::Idle;
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Test/smoke seeding of the named states (the `SURAMAK_SMOKE_HUD` harness).
    /// Never reachable from the normal app path.
    pub fn seed_demo(&mut self, which: &str, chrome: &HudChrome, now: Instant) {
        match which {
            "recording" => {
                *self = HudState::Recording(RecordingHud {
                    started_at: now,
                    elapsed: Duration::from_secs(12),
                    level: 0.42,
                    stopping: false,
                });
            }
            "processing" => {
                *self = HudState::Processing(ProcessingHud {
                    started_at: now,
                    baseline: None,
                    progress: Some(63),
                    stage: Some("transcribing".to_string()),
                });
            }
            "result" => {
                let text = "The quick brown fox jumps over the lazy dog, and the dictation lands \
                            exactly where the cursor was."
                    .to_string();
                let _ = self.on_transcript(text, None, chrome, now);
            }
            "cancelled" => {
                self.refuse(
                    "A transcription is already running — try again when it finishes.",
                    now,
                );
            }
            "permission" => {
                *self = HudState::PermissionFailure(PermissionHud {
                    kind: PermissionKind::Accessibility,
                    message: INSERT_NEEDS_ACCESSIBILITY.to_string(),
                    retry_text: Some("Preserved text".to_string()),
                    at: now,
                });
            }
            _ => *self = HudState::Idle,
        }
    }
}

/// The transient user intents the view emits. The machine turns them into
/// [`HudAction`]s (and state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudIntent {
    Stop,
    Cancel,
    Dismiss,
    Copy,
    Insert,
    Keep,
    Retry,
    OpenSettings,
}

/// Side effects the UI thread executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HudAction {
    /// Start the existing M6 capture path (`AppState::start_recording`).
    StartCapture,
    /// Stop + finalize + transcribe it (`AppState::stop_recording`).
    StopCapture,
    /// Stop and discard the capture (`Runtime`'s abort path) — the only
    /// mid-recording close, and it never leaks a session.
    AbortCapture,
    /// Cancel the in-flight transcription job.
    CancelJob,
    /// Write `text` to the clipboard (the content we own).
    Copy(String),
    /// Write `text`, then post ⌘V (Accessibility-gated).
    Insert(String),
    OpenAccessibilitySettings,
    OpenMicrophoneSettings,
    /// Bring the card to the front for keyboard operation.
    FocusHud,
}

/// The chip/caption data the view needs from the app (not part of the machine's
/// state, and cheap to copy every frame).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudChrome {
    /// macOS Accessibility is granted (the Insert gate).
    pub accessibility: bool,
    pub insertion: InsertionPreference,
    /// The registered chord's display label (`⇧⌘Space`).
    pub hotkey_label: String,
    /// Whether the window's surface really supports transparency (macOS). On an
    /// opaque surface the card's drop shadow would render as a black frame, so
    /// the view skips it there.
    pub card_shadow: bool,
}

impl Default for HudChrome {
    fn default() -> Self {
        Self {
            accessibility: false,
            insertion: InsertionPreference::CopyPrimary,
            hotkey_label: suramak_taps::hotkey::DEFAULT_HOTKEY.label(),
            card_shadow: true,
        }
    }
}

impl HudChrome {
    pub fn new(accessibility: bool, insertion: InsertionPreference, hotkey_label: String) -> Self {
        Self {
            accessibility,
            insertion,
            hotkey_label,
            card_shadow: true,
        }
    }

    /// The window knows its surface's alpha mode; the view must not paint a
    /// shadow onto an opaque surface (it would be a hard black frame).
    pub fn with_card_shadow(mut self, card_shadow: bool) -> Self {
        self.card_shadow = card_shadow;
        self
    }
}

/// The short, honest disclosure shown wherever Insert is offered.
pub const INSERT_NEEDS_ACCESSIBILITY: &str =
    "Insert needs the Accessibility permission: Suramak presses ⌘V for you in the app you were \
     in. Until it is granted, Copy puts the transcript on your clipboard.";

/// The transcript's plain text: segments joined by one space, empties skipped.
pub fn transcript_text(transcript: &Transcript) -> String {
    transcript
        .segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Classifies a capture error. We only call something a permission failure when
/// the message actually names permission/authorization — a missing device is a
/// plain failure, not a lie about permissions.
pub fn permission_from_message(message: &str) -> Option<PermissionKind> {
    let lower = message.to_ascii_lowercase();
    let permission = lower.contains("permission")
        || lower.contains("not authorized")
        || lower.contains("denied")
        || lower.contains("privacy")
        || lower.contains("tcc");
    permission.then_some(PermissionKind::Microphone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Recording, RECORD_WAVEFORM_BARS};
    use std::path::PathBuf;

    fn t0() -> Instant {
        // A fixed origin is not possible for `Instant`; every test captures one
        // instant and derives the rest, which is deterministic for durations.
        Instant::now()
    }

    fn chrome() -> HudChrome {
        HudChrome::default()
    }

    fn insert_chrome(accessibility: bool) -> HudChrome {
        HudChrome::new(
            accessibility,
            InsertionPreference::InsertPrimary,
            "⇧⌘Space".to_string(),
        )
    }

    fn recording_state(started_at: Instant) -> RecordState {
        RecordState::Recording(Recording {
            title: "Recording test".to_string(),
            started_at,
            elapsed: Duration::from_secs(3),
            rms: 0.25,
            waveform: vec![0.25; RECORD_WAVEFORM_BARS],
            device: Some("Test mic".to_string()),
            sources: Vec::new(),
            provisional: Vec::new(),
            provisional_skipped: 0,
            provisional_arrivals: Vec::new(),
            stopping: false,
        })
    }

    fn transcript(id: Uuid) -> Transcript {
        let mut transcript = Transcript::new(
            "dictation".to_string(),
            suramak_core::transcript::Source::File {
                name: "dictation.wav".to_string(),
            },
            "turbo-q5-0".to_string(),
            None,
            Some(PathBuf::from("/tmp/dictation.wav")),
            vec![
                suramak_core::transcript::Segment {
                    idx: 0,
                    start_ms: 0,
                    end_ms: 900,
                    text: " Hello there. ".to_string(),
                },
                suramak_core::transcript::Segment {
                    idx: 1,
                    start_ms: 900,
                    end_ms: 1600,
                    text: String::new(),
                },
                suramak_core::transcript::Segment {
                    idx: 2,
                    start_ms: 1600,
                    end_ms: 2400,
                    text: "This is dictation.".to_string(),
                },
            ],
        );
        transcript.id = id;
        transcript
    }

    #[test]
    fn idle_is_hidden_and_never_repaints() {
        let state = HudState::idle();
        assert_eq!(state.phase(), HudPhase::Idle);
        assert!(!state.is_visible());
        assert!(!state.capture_live());
        assert_eq!(state.cadence(), None, "the M9 idle budget: no repaint");
    }

    #[test]
    fn the_hotkey_starts_recording_and_the_indicator_is_visible() {
        let now = t0();
        let mut state = HudState::idle();
        let actions = state.hotkey(now, None);
        assert_eq!(actions, [HudAction::StartCapture]);
        assert_eq!(state.phase(), HudPhase::Recording);
        assert!(state.is_visible(), "a live capture must always be visible");
        assert!(state.capture_live());
        assert_eq!(state.cadence(), Some(RECORDING_TICK));
    }

    #[test]
    fn the_hotkey_stops_a_recording_into_processing_once() {
        let now = t0();
        let mut state = HudState::idle();
        state.hotkey(now, None);
        let baseline = Uuid::new_v4();
        let actions = state.hotkey(now + Duration::from_secs(2), Some(baseline));
        assert_eq!(actions, [HudAction::StopCapture]);
        assert_eq!(state.phase(), HudPhase::Processing);
        match &state {
            HudState::Processing(processing) => {
                assert_eq!(processing.baseline, Some(baseline));
            }
            other => panic!("expected Processing, got {other:?}"),
        }
        // A re-press while processing must not queue a second stop or start.
        assert!(state.hotkey(now + Duration::from_secs(3), None).is_empty());
        assert_eq!(state.phase(), HudPhase::Processing);
    }

    #[test]
    fn the_hotkey_focuses_a_result_for_keyboard_operation() {
        let now = t0();
        let mut state = HudState::idle();
        state.on_transcript("hello".to_string(), None, &chrome(), now);
        assert_eq!(state.hotkey(now, None), [HudAction::FocusHud]);
        assert_eq!(state.phase(), HudPhase::Result);
    }

    #[test]
    fn stop_only_applies_to_a_live_recording() {
        let now = t0();
        let mut state = HudState::idle();
        assert!(state
            .on_intent(HudIntent::Stop, &chrome(), None, now)
            .is_empty());
        state.hotkey(now, None);
        assert_eq!(
            state.on_intent(HudIntent::Stop, &chrome(), None, now),
            [HudAction::StopCapture]
        );
        assert!(
            state
                .on_intent(HudIntent::Stop, &chrome(), None, now)
                .is_empty(),
            "a second Stop is a no-op"
        );
    }

    #[test]
    fn a_transcript_becomes_the_result_and_an_empty_one_is_cancelled() {
        let now = t0();
        let mut state = HudState::idle();
        state.hotkey(now, None);
        state.hotkey(now, None);
        let id = Uuid::new_v4();
        state.on_transcript("  hello world  ".to_string(), Some(id), &chrome(), now);
        match &state {
            HudState::Result(result) => {
                assert_eq!(result.text, "hello world");
                assert_eq!(result.transcript, Some(id));
                assert!(!result.copied && !result.inserted && !result.kept);
                assert_eq!(state.cadence(), None, "a result card is static");
            }
            other => panic!("expected Result, got {other:?}"),
        }

        let mut empty = HudState::idle();
        empty.on_transcript("   \n ".to_string(), None, &chrome(), now);
        match &empty {
            HudState::Cancelled(cancelled) => {
                assert!(cancelled.reason.contains("Nothing was heard"));
                assert!(!cancelled.by_user);
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn insert_primary_copies_then_inserts_only_with_accessibility() {
        let now = t0();
        let mut granted = HudState::idle();
        let actions =
            granted.on_transcript("dictated text".to_string(), None, &insert_chrome(true), now);
        assert_eq!(
            actions,
            [
                HudAction::Copy("dictated text".to_string()),
                HudAction::Insert("dictated text".to_string())
            ],
            "the clipboard is written before the synthetic ⌘V"
        );
        match &granted {
            HudState::Result(result) => {
                assert!(result.copied);
                assert!(result.inserted);
                assert_eq!(result.note, None);
            }
            other => panic!("expected Result, got {other:?}"),
        }

        let mut denied = HudState::idle();
        let actions = denied.on_transcript(
            "dictated text".to_string(),
            None,
            &insert_chrome(false),
            now,
        );
        assert!(actions.is_empty(), "no keystroke without the permission");
        match &denied {
            HudState::Result(result) => {
                assert!(!result.inserted);
                assert!(result
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Accessibility"));
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn copy_primary_never_posts_a_keystroke_on_its_own() {
        let now = t0();
        let mut state = HudState::idle();
        assert!(state
            .on_transcript("text".to_string(), None, &chrome(), now)
            .is_empty());
        assert_eq!(
            state.on_intent(HudIntent::Copy, &chrome(), None, now),
            [HudAction::Copy("text".to_string())]
        );
        match &state {
            HudState::Result(result) => assert!(result.copied && !result.inserted),
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn a_denied_insert_preserves_the_text_and_returns_to_the_result() {
        let now = t0();
        let mut state = HudState::idle();
        state.on_transcript("keep me".to_string(), None, &chrome(), now);
        let actions = state.on_intent(HudIntent::Insert, &chrome(), None, now);
        assert_eq!(actions, [HudAction::OpenAccessibilitySettings]);
        match &state {
            HudState::PermissionFailure(permission) => {
                assert_eq!(permission.kind, PermissionKind::Accessibility);
                assert_eq!(permission.retry_text.as_deref(), Some("keep me"));
            }
            other => panic!("expected PermissionFailure, got {other:?}"),
        }
        // Coming back from a granted permission restores the text.
        let actions = state.on_intent(HudIntent::Retry, &chrome(), None, now);
        assert_eq!(actions, [HudAction::FocusHud]);
        match &state {
            HudState::Result(result) => assert_eq!(result.text, "keep me"),
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn an_insert_failure_from_the_platform_is_reported_without_losing_the_result() {
        let now = t0();
        let mut state = HudState::idle();
        state.on_transcript("text".to_string(), None, &chrome(), now);
        state.on_insert_result(
            Err(InsertError::Failed(
                "the paste event was rejected".to_string(),
            )),
            now,
        );
        match &state {
            HudState::Result(result) => {
                assert!(!result.inserted);
                assert!(result
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .contains("rejected"));
            }
            other => panic!("expected Result, got {other:?}"),
        }
        state.on_insert_result(Err(InsertError::PermissionDenied), now);
        assert_eq!(state.phase(), HudPhase::PermissionFailure);
    }

    #[test]
    fn a_permission_capture_failure_has_recovery_and_other_failures_are_cancelled() {
        let now = t0();
        let mut state = HudState::idle();
        state.hotkey(now, None);
        state.on_capture_failed(
            "microphone permission denied (TCC)".to_string(),
            now + Duration::from_millis(20),
        );
        match &state {
            HudState::PermissionFailure(permission) => {
                assert_eq!(permission.kind, PermissionKind::Microphone);
            }
            other => panic!("expected PermissionFailure, got {other:?}"),
        }
        assert_eq!(
            state.on_intent(HudIntent::OpenSettings, &chrome(), None, now),
            [HudAction::OpenMicrophoneSettings]
        );
        assert_eq!(
            state.on_intent(HudIntent::Retry, &chrome(), None, now),
            [HudAction::StartCapture]
        );

        let mut missing = HudState::idle();
        missing.hotkey(now, None);
        missing.on_capture_failed("no default input device".to_string(), now);
        match &missing {
            HudState::Cancelled(cancelled) => {
                assert!(cancelled.reason.contains("no default input"));
                assert!(!cancelled.by_user);
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn closing_mid_recording_aborts_the_capture_and_never_leaks_a_session() {
        let now = t0();
        for intent in [HudIntent::Cancel, HudIntent::Dismiss] {
            let mut state = HudState::idle();
            state.hotkey(now, None);
            let actions = state.on_intent(intent, &chrome(), None, now);
            assert_eq!(actions, [HudAction::AbortCapture], "{intent:?}");
            assert!(!state.capture_live());
            assert_eq!(state.phase(), HudPhase::Cancelled);
            assert!(
                state.is_visible(),
                "the cancellation is acknowledged on screen"
            );
        }
    }

    #[test]
    fn closing_mid_processing_cancels_the_job() {
        let now = t0();
        let mut state = HudState::idle();
        state.hotkey(now, None);
        state.hotkey(now, None);
        assert_eq!(
            state.on_intent(HudIntent::Dismiss, &chrome(), None, now),
            [HudAction::CancelJob]
        );
        assert_eq!(state.phase(), HudPhase::Cancelled);
    }

    #[test]
    fn dismissing_a_result_returns_to_idle_but_keep_pins_the_card() {
        let now = t0();
        let mut state = HudState::idle();
        state.on_transcript("text".to_string(), None, &chrome(), now);
        state.on_intent(HudIntent::Keep, &chrome(), None, now);
        match &state {
            HudState::Result(result) => assert!(result.kept),
            other => panic!("expected Result, got {other:?}"),
        }
        state.on_intent(HudIntent::Dismiss, &chrome(), None, now);
        assert_eq!(state.phase(), HudPhase::Idle);
    }

    #[test]
    fn a_refusal_and_a_user_cancellation_read_differently() {
        let now = t0();
        let mut refused = HudState::idle();
        refused.refuse("A transcription is already running", now);
        assert_eq!(refused.title(), "Not started");
        assert_eq!(refused.phase(), HudPhase::Cancelled);

        let mut cancelled = HudState::idle();
        cancelled.hotkey(now, None);
        cancelled.on_intent(HudIntent::Cancel, &chrome(), None, now);
        assert_eq!(cancelled.title(), "Cancelled");
    }

    #[test]
    fn the_hotkey_attaches_to_a_recording_started_by_the_floating_pill() {
        let now = t0();
        let mut state = HudState::idle();
        state.attach_to_recording(Duration::from_secs(5), 0.4, now - Duration::from_secs(5));
        assert_eq!(state.phase(), HudPhase::Recording);
        assert!(state.capture_live() && state.is_visible());
        match &state {
            HudState::Recording(recording) => {
                assert_eq!(recording.elapsed, Duration::from_secs(5));
            }
            other => panic!("expected Recording, got {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_card_auto_hides_after_its_dwell() {
        let now = t0();
        let mut state = HudState::idle();
        state.refuse("a job is already running", now);
        assert_eq!(state.phase(), HudPhase::Cancelled);
        assert!(state.is_visible());
        state.tick(now + CANCELLED_DWELL - Duration::from_millis(1));
        assert_eq!(state.phase(), HudPhase::Cancelled);
        state.tick(now + CANCELLED_DWELL);
        assert_eq!(state.phase(), HudPhase::Idle);
        assert!(!state.is_visible());
    }

    #[test]
    fn sync_mirrors_the_live_recording_and_follows_a_runtime_failure() {
        let now = t0();
        let mut state = HudState::idle();
        state.hotkey(now, None);
        let live = recording_state(now);
        state.sync_from_app(
            &live,
            &Job::Idle,
            None,
            &chrome(),
            now + Duration::from_secs(3),
        );
        match &state {
            HudState::Recording(recording) => {
                assert_eq!(recording.elapsed, Duration::from_secs(3));
                assert!((recording.level - 0.25).abs() < f32::EPSILON);
            }
            other => panic!("expected Recording, got {other:?}"),
        }

        state.sync_from_app(
            &RecordState::Failed {
                message: "microphone permission denied".to_string(),
            },
            &Job::Idle,
            None,
            &chrome(),
            now,
        );
        assert_eq!(state.phase(), HudPhase::PermissionFailure);
    }

    #[test]
    fn a_processing_card_accepts_only_a_new_transcript_and_rejects_a_stale_one() {
        let now = t0();
        let mut state = HudState::idle();
        state.hotkey(now, None);
        let baseline = Uuid::new_v4();
        state.hotkey(now, Some(baseline));
        let stale = SaveOutcome {
            transcript: transcript(baseline),
            error: None,
        };
        assert!(state
            .sync_from_app(&RecordState::Idle, &Job::Idle, Some(&stale), &chrome(), now)
            .is_empty());
        assert_eq!(state.phase(), HudPhase::Processing);

        let fresh_id = Uuid::new_v4();
        let fresh = SaveOutcome {
            transcript: transcript(fresh_id),
            error: None,
        };
        state.sync_from_app(&RecordState::Idle, &Job::Idle, Some(&fresh), &chrome(), now);
        match &state {
            HudState::Result(result) => {
                assert_eq!(result.transcript, Some(fresh_id));
                assert_eq!(result.text, "Hello there. This is dictation.");
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn a_processing_card_reports_job_failure_progress_and_stage() {
        let now = t0();
        let mut state = HudState::idle();
        state.hotkey(now, None);
        state.hotkey(now, None);
        let running = Job::Running(crate::app::RunningJob {
            input: PathBuf::from("/tmp/a.wav"),
            pct: 42,
            model: None,
            requested_engine: None,
            planned_engine: None,
            warnings: Vec::new(),
            cancelling: false,
            language: None,
            started_at: Some(now),
            duration_ms: None,
            queue_id: None,
            auto: false,
            stage: Some(suramak_asr::worker::JobStage::Transcribing),
        });
        state.sync_from_app(&RecordState::Idle, &running, None, &chrome(), now);
        match &state {
            HudState::Processing(processing) => {
                assert_eq!(processing.progress, Some(42));
                assert_eq!(processing.stage.as_deref(), Some("transcribing"));
            }
            other => panic!("expected Processing, got {other:?}"),
        }
        state.sync_from_app(
            &RecordState::Idle,
            &Job::Failed {
                input: PathBuf::from("/tmp/a.wav"),
                message: "model not found".to_string(),
            },
            None,
            &chrome(),
            now,
        );
        match &state {
            HudState::Cancelled(cancelled) => assert_eq!(cancelled.reason, "model not found"),
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn a_refused_start_is_visible_and_explains_itself() {
        let now = t0();
        let mut state = HudState::idle();
        state.refuse("A transcription is already running", now);
        match &state {
            HudState::Cancelled(cancelled) => {
                assert!(cancelled.reason.contains("already running"));
                assert!(!cancelled.by_user);
                assert_eq!(
                    state.cadence(),
                    Some(RECORDING_TICK),
                    "it must paint, then hide"
                );
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn the_indicator_invariant_holds_across_every_transition() {
        let now = t0();
        let mut state = HudState::idle();
        let check = |state: &HudState| {
            if state.capture_live() {
                assert!(
                    state.is_visible(),
                    "a live capture must always have a visible indicator: {state:?}"
                );
            }
        };
        check(&state);
        for intent in [
            HudIntent::Stop,
            HudIntent::Cancel,
            HudIntent::Dismiss,
            HudIntent::Copy,
            HudIntent::Insert,
            HudIntent::Keep,
            HudIntent::Retry,
            HudIntent::OpenSettings,
        ] {
            state.on_intent(intent, &chrome(), None, now);
            check(&state);
            state.hotkey(now, None);
            check(&state);
            state.on_transcript("x".to_string(), None, &chrome(), now);
            check(&state);
            state.tick(now + CANCELLED_DWELL);
            check(&state);
            state.sync_from_app(&recording_state(now), &Job::Idle, None, &chrome(), now);
            check(&state);
            state.on_capture_failed("permission denied".to_string(), now);
            check(&state);
            state.on_intent(HudIntent::Dismiss, &chrome(), None, now);
            check(&state);
        }
    }

    #[test]
    fn permission_classification_is_literal() {
        assert_eq!(
            permission_from_message("microphone permission denied"),
            Some(PermissionKind::Microphone)
        );
        assert_eq!(
            permission_from_message("Recording permission is required"),
            Some(PermissionKind::Microphone)
        );
        assert_eq!(permission_from_message("no input device"), None);
        assert_eq!(permission_from_message("disk full"), None);
    }

    #[test]
    fn transcript_text_joins_segments_and_skips_empty_ones() {
        let text = transcript_text(&transcript(Uuid::new_v4()));
        assert_eq!(text, "Hello there. This is dictation.");
        let empty = Transcript::new(
            "empty".to_string(),
            suramak_core::transcript::Source::File {
                name: "empty.wav".to_string(),
            },
            "turbo-q5-0".to_string(),
            None,
            None,
            Vec::new(),
        );
        assert_eq!(transcript_text(&empty), "");
    }
}
