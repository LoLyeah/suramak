//! Opt-in bilingual export surface (M15.8) — the consent gate, the run
//! lifecycle, and the editable derived column's write path. **Egui-free**: the
//! rules here are unit-tested headless, including a full round trip against a
//! hand-rolled OpenAI-compatible mock (the pattern `app::polish` established).
//!
//! ## Rules this module encodes
//!
//! - **Nothing happens until the action is pressed.** [`AppState::start_translation`]
//!   is the only path to [`Effect::TranslateTranscript`]. No panel open, no
//!   transcript load, no export and no frame ever emits it.
//! - **No endpoint, no traffic.** [`bilingual_gate`] resolves the same gate the
//!   polish flow uses (base URL + API key) and returns the human reason; the
//!   action is disabled with that reason and `start_translation` is a no-op.
//!   The zero-request rule is asserted against a live mock listener.
//! - **Partial results are real results.** A run returns every validated
//!   segment plus one named failure per segment that did not land. Successful
//!   rows are written with their provenance; failures stay visible and the
//!   retry affordance re-sends exactly those ids.
//! - **Cancellation keeps what validated.** The cancel effect sets the same
//!   cooperative token the ASR job uses; the worker observes it between
//!   batches. A cancelled run persists its completed segments and reports
//!   `cancelled after N/M batches`.
//! - **The derived column is editable, but the source is authoritative.**
//!   Editing the translation rewrites only the derived text; editing the
//!   transcript text marks the translation stale immediately (and the store
//!   records the same rule, so it survives a reload).

use std::collections::BTreeMap;

use suramak_core::settings::Settings;
use suramak_core::translate::{
    TargetLanguage, TranslationProgress, TranslationRecord, TranslationRun,
};

use super::{AppState, Effect};

/// Shown next to the disabled action when the endpoint is not configured.
pub const BILINGUAL_SETUP_NOTE: &str = "bilingual export needs the endpoint configured in \
Settings (base URL + API key) — the transcript still exports normally without it";

/// The action label on the export sheet.
pub const BILINGUAL_ACTION_LABEL: &str = "Translate for export";

/// The caption under an untouched bilingual section.
pub const BILINGUAL_IDLE_NOTE: &str = "the endpoint is only called when you press this button; \
results are editable in the transcript's translation column";

/// The resolved gate: `Ok(())` when a translation run may be offered, `Err` with
/// the user-facing reason otherwise.
///
/// Deliberately the **same** configuration as polish (base URL + API key): the
/// workorder's opt-in endpoint is the one M10 wired, not a second setting.
/// `key_present` is injected so the rule is testable without an environment.
pub fn bilingual_gate(settings: &Settings, key_present: bool) -> Result<(), String> {
    let Some(polish) = &settings.polish else {
        return Err(BILINGUAL_SETUP_NOTE.to_string());
    };
    if polish.base_url.trim().is_empty() {
        return Err(BILINGUAL_SETUP_NOTE.to_string());
    }
    if !key_present {
        return Err(
            "bilingual export has no API key — save it in Settings (OS keychain) on \
macOS/Windows, or set SURAMAK_OPENAI_API_KEY on Linux"
                .to_string(),
        );
    }
    Ok(())
}

/// Lifecycle of the bilingual export run.
#[derive(Debug, Clone, PartialEq)]
pub enum BilingualState {
    /// Nothing in flight, nothing to retry.
    Idle,
    /// The batches are running on the runtime's thread.
    Running {
        target: TargetLanguage,
        progress: TranslationProgress,
    },
    /// A run returned (fully or partially): successful rows are already applied
    /// and persisted, `run.failed` is what a retry would send.
    Finished {
        target: TargetLanguage,
        run: TranslationRun,
    },
    /// The run never started or died before a batch (no key, client build
    /// failure): the transcript is unchanged and the reason is shown.
    Failed {
        target: TargetLanguage,
        message: String,
    },
}

impl BilingualState {
    pub fn is_running(&self) -> bool {
        matches!(self, BilingualState::Running { .. })
    }

    pub fn target(&self) -> Option<TargetLanguage> {
        match self {
            BilingualState::Idle => None,
            BilingualState::Running { target, .. }
            | BilingualState::Finished { target, .. }
            | BilingualState::Failed { target, .. } => Some(*target),
        }
    }
}

/// What the runtime reports to the UI on each poll.
#[derive(Debug, Clone, PartialEq)]
pub enum TranslationEvent {
    /// One batch was sent (or is about to be): update the progress caption.
    Progress(TranslationProgress),
    /// The run ended — success, partial, cancelled, or a run-level failure.
    Done(Result<TranslationRun, String>),
}

impl AppState {
    /// Whether the Translate action may run: the endpoint gate resolved, an
    /// editor with segments open, and no run already in flight.
    pub fn can_translate(&self) -> bool {
        self.translation_gate_reason.is_none()
            && !self.bilingual.is_running()
            && self
                .editor
                .as_ref()
                .is_some_and(|editor| !editor.transcript.segments.is_empty())
    }

    /// The target the surface offers: the transcript's language implies the
    /// useful direction (ID → EN, anything else → ID), overridable per run.
    pub fn bilingual_target(&self) -> TargetLanguage {
        self.bilingual_target
    }

    /// The export sheet's target control.
    pub fn set_bilingual_target(&mut self, target: TargetLanguage) {
        self.bilingual_target = target;
    }

    /// The Translate action: snapshot the editor's segments and ask the runtime
    /// to run the batches. `only` restricts the run to a failed-id retry.
    ///
    /// If the editor has unsaved text, the save is emitted first and the run is
    /// parked ([`AppState::pending_translation`]) until the save lands: a
    /// translation must never describe text the store has not accepted, or the
    /// save would immediately mark the fresh translation stale.
    pub fn start_translation(&mut self, only: Option<Vec<u32>>) -> Vec<Effect> {
        if !self.can_translate() {
            return Vec::new();
        }
        let target = self.bilingual_target;
        let Some(editor) = self.editor.as_ref() else {
            return Vec::new();
        };
        if editor.dirty {
            let updates = editor.pending_updates();
            if !updates.is_empty() {
                self.pending_translation = Some((target, only));
                return vec![Effect::SaveEdits { updates }];
            }
        }
        self.start_translation_now(target, only)
    }

    /// Emits the run effect for the current editor snapshot.
    pub(super) fn start_translation_now(
        &mut self,
        target: TargetLanguage,
        only: Option<Vec<u32>>,
    ) -> Vec<Effect> {
        let Some(editor) = self.editor.as_ref() else {
            return Vec::new();
        };
        let mut transcript = editor.snapshot();
        if let Some(ids) = &only {
            transcript
                .segments
                .retain(|segment| ids.contains(&segment.idx));
        }
        let segments = transcript.segments.len();
        if segments == 0 {
            self.set_status("nothing to translate in the selected segments", true);
            return Vec::new();
        }
        self.bilingual = BilingualState::Running {
            target,
            progress: TranslationProgress {
                batch: 0,
                total: 0,
                translated: 0,
                failed: 0,
            },
        };
        self.set_status(
            format!(
                "translating {} to {} — the endpoint answers batch by batch",
                super::count_noun(segments, "segment"),
                target.label()
            ),
            false,
        );
        vec![Effect::TranslateTranscript {
            transcript: Box::new(transcript),
            target,
            only,
        }]
    }

    /// A progress/done event from the runtime.
    pub fn on_translation_event(&mut self, event: TranslationEvent) -> Vec<Effect> {
        match event {
            TranslationEvent::Progress(progress) => {
                if let BilingualState::Running {
                    progress: current, ..
                } = &mut self.bilingual
                {
                    *current = progress;
                }
                Vec::new()
            }
            TranslationEvent::Done(result) => {
                if !self.bilingual.is_running() {
                    // A stray result (the editor was closed / a new transcript
                    // opened while the run was in flight) — nothing to attach
                    // it to. The store is untouched by this path.
                    return Vec::new();
                }
                let target = self.bilingual.target().unwrap_or(self.bilingual_target);
                match result {
                    Ok(run) => self.apply_translation_run(target, run),
                    Err(message) => {
                        self.bilingual = BilingualState::Failed {
                            target,
                            message: message.clone(),
                        };
                        self.set_status(format!("translation failed: {message}"), true);
                        Vec::new()
                    }
                }
            }
        }
    }

    /// Applies a returned run to the editor's derived column and persists the
    /// validated rows with their provenance.
    fn apply_translation_run(
        &mut self,
        target: TargetLanguage,
        run: TranslationRun,
    ) -> Vec<Effect> {
        let mut records: Vec<(u32, TranslationRecord)> = Vec::with_capacity(run.translated.len());
        if let Some(editor) = self.editor.as_mut() {
            for translation in &run.translated {
                let record = run.record(translation, vec![translation.idx]);
                let index = translation.idx as usize;
                if index < editor.translations.len() {
                    editor.translations[index] = record.text.clone();
                    editor.translation_meta[index] = Some(record.clone());
                }
                records.push((translation.idx, record));
            }
            // A derived edit pending on an *untranslated* row must survive the
            // run: only clear the flag when nothing is left to save.
            editor.translation_dirty = !editor.translation_records().is_empty();
            editor.translation_save_error = None;
        }
        let summary = run.summary();
        let failed = run.failed.len();
        let cancelled = run.cancelled;
        self.bilingual = BilingualState::Finished { target, run };
        let mut effects = Vec::new();
        if !records.is_empty() {
            let id = self.editor.as_ref().map(|editor| editor.transcript.id);
            if let Some(id) = id {
                effects.push(Effect::SaveTranslations {
                    transcript: id,
                    records,
                });
            }
        }
        if cancelled {
            self.set_status(format!("translation cancelled — {summary}"), false);
        } else if failed > 0 {
            self.set_status(
                format!(
                    "{summary} — failed segments are listed in the export sheet, Retry resends only those"
                ),
                true,
            );
        } else {
            self.set_status(format!("translation done — {summary}"), false);
        }
        effects
    }

    /// The retry affordance: re-sends only the failed ids of the last run.
    pub fn retry_failed_translations(&mut self) -> Vec<Effect> {
        let ids = match &self.bilingual {
            BilingualState::Finished { run, .. } if !run.failed.is_empty() => run.failed_ids(),
            _ => return Vec::new(),
        };
        self.start_translation(Some(ids))
    }

    /// Cancels a run in flight through the runtime's cooperative token.
    pub fn cancel_translation(&mut self) -> Vec<Effect> {
        if !self.bilingual.is_running() {
            return Vec::new();
        }
        self.set_status(
            "cancelling translation — the current batch finishes, completed batches are kept",
            false,
        );
        vec![Effect::CancelTranslation]
    }

    /// A translation-row save landed (or failed): the status line reports the
    /// failure; the derived column keeps the text for a retry either way.
    pub fn on_translations_saved(&mut self, result: Result<(), String>) -> Vec<Effect> {
        match result {
            Ok(()) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.apply_translation_buffers();
                    editor.translation_dirty = false;
                    editor.translation_save_error = None;
                }
            }
            Err(message) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.translation_save_error = Some(message.clone());
                }
                self.set_status(format!("translation text not saved: {message}"), true);
            }
        }
        Vec::new()
    }

    /// The store's translation rows for the open transcript (after a load, a
    /// structural edit or an undo): rebuilds the derived column by `idx`.
    pub fn on_translations_loaded(&mut self, id: uuid::Uuid, records: &[(u32, TranslationRecord)]) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.transcript.id != id {
            return;
        }
        let by_idx: BTreeMap<u32, &TranslationRecord> =
            records.iter().map(|(idx, record)| (*idx, record)).collect();
        editor.translations = editor
            .transcript
            .segments
            .iter()
            .map(|segment| {
                by_idx
                    .get(&segment.idx)
                    .map(|record| record.text.clone())
                    .unwrap_or_default()
            })
            .collect();
        editor.translation_meta = editor
            .transcript
            .segments
            .iter()
            .map(|segment| by_idx.get(&segment.idx).map(|record| (*record).clone()))
            .collect();
        editor.translation_dirty = false;
        editor.translation_save_error = None;
    }

    /// The bilingual section's headline: how many cues have a translation (and
    /// how many of those are stale), or the gate reason.
    pub fn bilingual_status_line(&self) -> String {
        if let Some(reason) = &self.translation_gate_reason {
            return reason.clone();
        }
        let Some(editor) = &self.editor else {
            return "open a transcript to translate it".to_string();
        };
        let translated = editor
            .translation_meta
            .iter()
            .filter(|meta| meta.is_some())
            .count();
        let stale = editor
            .translation_meta
            .iter()
            .flatten()
            .filter(|record| record.stale)
            .count();
        if translated == 0 {
            return BILINGUAL_IDLE_NOTE.to_string();
        }
        let mut line = format!(
            "{translated} of {} segments translated",
            editor.transcript.segments.len()
        );
        if stale > 0 {
            line.push_str(&format!(
                " · {stale} stale — the source changed, re-translate to refresh"
            ));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use suramak_core::paths;
    use suramak_core::settings::{PolishSettings, Settings};
    use suramak_core::store::Store;
    use suramak_core::transcript::{Segment, Source, Transcript};

    use super::*;
    use crate::app::Route;
    use crate::runtime::Runtime;

    fn segment(idx: u32, text: &str) -> Segment {
        Segment {
            idx,
            start_ms: i64::from(idx) * 1_000,
            end_ms: i64::from(idx) * 1_000 + 900,
            text: text.to_string(),
        }
    }

    fn transcript(language: &str, texts: &[&str]) -> Transcript {
        Transcript::new(
            "bilingual.wav".to_string(),
            Source::File {
                name: "bilingual.wav".to_string(),
            },
            "tiny".to_string(),
            Some(language.to_string()),
            None,
            texts
                .iter()
                .enumerate()
                .map(|(idx, text)| segment(idx as u32, text))
                .collect(),
        )
    }

    fn state_with(language: &str, texts: &[&str]) -> AppState {
        let mut state = AppState::new();
        state.translation_gate_reason = None;
        state.on_transcript_loaded(transcript(language, texts));
        state
    }

    fn settings(base_url: &str) -> Settings {
        Settings {
            polish: Some(PolishSettings {
                base_url: base_url.to_string(),
                model: "mock-model".to_string(),
                send_audio: false,
            }),
            ..Settings::default()
        }
    }

    fn translated_record(idx: u32, text: &str, stale: bool) -> TranslationRecord {
        let mut record =
            TranslationRecord::new(text, "mock-model", "2026-11-10T10:00:00+00:00", vec![idx]);
        record.stale = stale;
        record
    }

    // ---- the gate ------------------------------------------------------------

    #[test]
    fn the_gate_resolves_to_a_named_reason() {
        let err = bilingual_gate(&Settings::default(), true).unwrap_err();
        assert!(err.contains("endpoint configured"), "{err}");

        let blank = Settings {
            polish: Some(PolishSettings {
                base_url: "  ".to_string(),
                model: "m".to_string(),
                send_audio: false,
            }),
            ..Settings::default()
        };
        assert!(bilingual_gate(&blank, true).is_err());

        let configured = settings("http://localhost:9/v1");
        let err = bilingual_gate(&configured, false).unwrap_err();
        assert!(err.contains("API key"), "{err}");
        assert!(bilingual_gate(&configured, true).is_ok());
    }

    #[test]
    fn without_an_endpoint_the_action_is_visible_but_disabled_and_sends_nothing() {
        let mut state = state_with("en", &["Hello world."]);
        state.translation_gate_reason = Some(BILINGUAL_SETUP_NOTE.to_string());
        assert!(!state.can_translate());
        assert!(!state.bilingual_status_line().is_empty());
        assert!(state.start_translation(None).is_empty());
        assert_eq!(state.bilingual, BilingualState::Idle);
        // The ordinary export path is untouched by the disabled feature.
        assert!(!state.export.bilingual);
    }

    // ---- the run lifecycle ---------------------------------------------------

    #[test]
    fn starting_a_run_snapshots_the_segments_and_the_target() {
        let mut state = state_with("en", &["Hello world.", "This is a test."]);
        assert_eq!(state.bilingual_target(), TargetLanguage::Indonesian);
        let effects = state.start_translation(None);
        match effects.as_slice() {
            [Effect::TranslateTranscript {
                transcript,
                target,
                only,
            }] => {
                assert_eq!(*target, TargetLanguage::Indonesian);
                assert_eq!(transcript.segments.len(), 2);
                assert!(only.is_none());
            }
            other => panic!("expected a translation effect, got {other:?}"),
        }
        assert!(state.bilingual.is_running());
        assert!(state.busy(), "a run keeps the tick alive");
        assert!(state.start_translation(None).is_empty(), "no double start");
    }

    #[test]
    fn an_indonesian_transcript_defaults_to_english() {
        let state = state_with("id", &["Halo dunia."]);
        assert_eq!(state.bilingual_target(), TargetLanguage::English);
    }

    #[test]
    fn unsaved_text_is_saved_before_the_run_starts() {
        let mut state = state_with("en", &["helo wrold"]);
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "Hello world.".to_string(), Instant::now());
        let effects = state.start_translation(None);
        assert!(
            matches!(effects.as_slice(), [Effect::SaveEdits { .. }]),
            "the run waits for the save: {effects:?}"
        );
        assert_eq!(state.bilingual, BilingualState::Idle);
        assert!(state.pending_translation.is_some());

        let effects = state.on_edits_saved(Instant::now(), Ok(()));
        assert!(matches!(
            effects.as_slice(),
            [Effect::TranslateTranscript { .. }]
        ));
        assert!(state.bilingual.is_running());
    }

    fn finished_run(
        state: &mut AppState,
        target: TargetLanguage,
        translated: &[(u32, &str)],
        failed: &[u32],
    ) -> Vec<Effect> {
        let run = TranslationRun {
            translated: translated
                .iter()
                .map(|(idx, text)| suramak_core::translate::SegmentTranslation {
                    idx: *idx,
                    text: (*text).to_string(),
                })
                .collect(),
            failed: failed
                .iter()
                .map(|idx| suramak_core::translate::SegmentFailure {
                    idx: *idx,
                    message: "HTTP 500: mock".to_string(),
                })
                .collect(),
            batches_total: 1,
            batches_done: 1,
            cancelled: false,
            model: "mock-model".to_string(),
            target,
            translated_at: "2026-11-10T10:00:00+00:00".to_string(),
        };
        let b = &mut state.bilingual;
        *b = BilingualState::Running {
            target,
            progress: TranslationProgress {
                batch: 1,
                total: 1,
                translated: 0,
                failed: 0,
            },
        };
        state.on_translation_event(TranslationEvent::Done(Ok(run)))
    }

    #[test]
    fn a_finished_run_populates_the_derived_column_and_persists_provenance() {
        let mut state = state_with("en", &["Hello world.", "This is a test."]);
        state.start_translation(None);
        let effects = finished_run(
            &mut state,
            TargetLanguage::Indonesian,
            &[(0, "Halo dunia."), (1, "Ini adalah ujian.")],
            &[],
        );
        match effects.as_slice() {
            [Effect::SaveTranslations {
                transcript,
                records,
            }] => {
                assert_eq!(*transcript, state.editor.as_ref().unwrap().transcript.id);
                assert_eq!(records.len(), 2);
                assert_eq!(records[0].1.model, "mock-model");
                assert_eq!(records[0].1.source_ids, vec![0]);
                assert!(!records[0].1.stale);
            }
            other => panic!("expected one persistence effect, got {other:?}"),
        }
        let editor = state.editor.as_ref().unwrap();
        assert_eq!(editor.translations[0], "Halo dunia.");
        assert_eq!(editor.translations[1], "Ini adalah ujian.");
        assert!(editor.translation_meta[0].is_some());
        assert!(matches!(state.bilingual, BilingualState::Finished { .. }));
        assert!(state
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("translation done"));
    }

    #[test]
    fn a_partial_run_names_the_failures_and_retry_sends_only_those_ids() {
        let mut state = state_with("en", &["Hello world.", "This is a test.", "Goodbye."]);
        state.start_translation(None);
        let effects = finished_run(
            &mut state,
            TargetLanguage::Indonesian,
            &[(0, "Halo dunia."), (2, "Selamat tinggal.")],
            &[1],
        );
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveTranslations { .. }]
        ));
        match &state.bilingual {
            BilingualState::Finished { run, .. } => {
                assert_eq!(run.failed.len(), 1);
                assert_eq!(run.failed[0].idx, 1);
                assert!(run.failed[0].label().contains("HTTP 500"));
            }
            other => panic!("expected a finished run, got {other:?}"),
        }
        assert!(state
            .bilingual_status_line()
            .contains("2 of 3 segments translated"));
        assert!(state
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("Retry resends only those"));

        let effects = state.retry_failed_translations();
        match effects.as_slice() {
            [Effect::TranslateTranscript {
                transcript, only, ..
            }] => {
                assert_eq!(transcript.segments.len(), 1);
                assert_eq!(transcript.segments[0].idx, 1);
                assert_eq!(only.as_deref(), Some(&[1u32][..]));
            }
            other => panic!("expected one retry effect, got {other:?}"),
        }
    }

    #[test]
    fn cancellation_keeps_completed_segments_and_reports_where_it_stopped() {
        let mut state = state_with("en", &["Hello world.", "This is a test."]);
        state.start_translation(None);
        assert!(matches!(
            state.cancel_translation().as_slice(),
            [Effect::CancelTranslation]
        ));
        let run = TranslationRun {
            translated: vec![suramak_core::translate::SegmentTranslation {
                idx: 0,
                text: "Halo dunia.".to_string(),
            }],
            failed: Vec::new(),
            batches_total: 3,
            batches_done: 1,
            cancelled: true,
            model: "mock-model".to_string(),
            target: TargetLanguage::Indonesian,
            translated_at: "2026-11-10T10:00:00+00:00".to_string(),
        };
        let effects = state.on_translation_event(TranslationEvent::Done(Ok(run)));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveTranslations { .. }]
        ));
        assert_eq!(
            state.editor.as_ref().unwrap().translations[0],
            "Halo dunia."
        );
        assert!(state
            .status
            .as_ref()
            .unwrap()
            .text
            .contains("cancelled after 1/3"));
        assert!(!state.bilingual.is_running());
    }

    #[test]
    fn a_run_that_never_started_fails_with_its_reason_and_retries() {
        let mut state = state_with("en", &["Hello world."]);
        state.start_translation(None);
        state.on_translation_event(TranslationEvent::Done(Err(
            "translation has no API key".to_string()
        )));
        match &state.bilingual {
            BilingualState::Failed { message, .. } => assert!(message.contains("API key")),
            other => panic!("expected a failure, got {other:?}"),
        }
        // Retry is available once the failure is dismissible by a new action.
        assert!(state.can_translate());
    }

    #[test]
    fn a_stray_result_is_ignored() {
        let mut state = state_with("en", &["Hello world."]);
        let effects = state.on_translation_event(TranslationEvent::Done(Ok(TranslationRun {
            translated: Vec::new(),
            failed: Vec::new(),
            batches_total: 0,
            batches_done: 0,
            cancelled: false,
            model: "m".to_string(),
            target: TargetLanguage::Indonesian,
            translated_at: "2026-11-10T10:00:00+00:00".to_string(),
        })));
        assert!(effects.is_empty());
        assert_eq!(state.bilingual, BilingualState::Idle);
    }

    // ---- stale semantics -----------------------------------------------------

    #[test]
    fn source_edits_mark_the_translation_stale_and_derived_edits_do_not() {
        let mut state = state_with("en", &["Hello world."]);
        state.on_translations_loaded(
            state.editor.as_ref().unwrap().transcript.id,
            &[(0, translated_record(0, "Halo dunia.", false))],
        );
        let now = Instant::now();
        state
            .editor
            .as_mut()
            .unwrap()
            .edit(0, "Hello, world!".to_string(), now);
        assert!(
            state.editor.as_ref().unwrap().translation_meta[0]
                .as_ref()
                .unwrap()
                .stale
        );

        // Editing the derived text keeps the stale marker and provenance.
        state
            .editor
            .as_mut()
            .unwrap()
            .edit_translation(0, "Halo, dunia!".to_string(), now);
        let record = state.editor.as_ref().unwrap().translation_meta[0]
            .as_ref()
            .unwrap();
        assert!(record.stale);
        assert_eq!(record.model, "mock-model");
        assert_eq!(
            state.editor.as_ref().unwrap().translations[0],
            "Halo, dunia!"
        );
    }

    #[test]
    fn loading_translations_rebuilds_the_column_by_segment_id() {
        let mut state = state_with("en", &["one", "two", "three"]);
        let id = state.editor.as_ref().unwrap().transcript.id;
        state.on_translations_loaded(
            id,
            &[
                (2, translated_record(2, "tiga", true)),
                (0, translated_record(0, "satu", false)),
            ],
        );
        let editor = state.editor.as_ref().unwrap();
        assert_eq!(editor.translations, vec!["satu", "", "tiga"]);
        assert!(editor.translation_meta[2].as_ref().unwrap().stale);
        assert!(editor.translation_meta[1].is_none());

        // A record for another transcript is ignored.
        state.on_translations_loaded(uuid::Uuid::new_v4(), &[]);
        assert_eq!(state.editor.as_ref().unwrap().translations[0], "satu");
    }

    #[test]
    fn editing_a_derived_row_emits_its_own_save_effect() {
        let mut state = state_with("en", &["Hello world."]);
        let id = state.editor.as_ref().unwrap().transcript.id;
        state.on_translations_loaded(id, &[(0, translated_record(0, "Halo dunia.", false))]);
        state.editor.as_mut().unwrap().edit_translation(
            0,
            "Halo dunia!".to_string(),
            Instant::now() - Duration::from_secs(5),
        );
        let effects = state.tick(Instant::now());
        match effects.as_slice() {
            [Effect::SaveTranslations { records, .. }] => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].1.text, "Halo dunia!");
                assert_eq!(records[0].1.model, "mock-model");
            }
            other => panic!("expected a translation save, got {other:?}"),
        }
        state.on_translations_saved(Ok(()));
        assert_eq!(
            state.editor.as_ref().unwrap().translation_meta[0]
                .as_ref()
                .unwrap()
                .text,
            "Halo dunia!"
        );
    }

    // ---- runtime: the mock endpoint (EN→ID and ID→EN) ------------------------

    /// Serialized on the app-wide [`crate::app::TEST_ENV_LOCK`], shared with the
    /// polish suite (both mutate the same process-global variables).
    use crate::app::TEST_ENV_LOCK as ENV_LOCK;

    fn read_request(stream: &mut TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 8192];
        let mut expected: Option<(usize, usize)> = None;
        loop {
            if expected.is_none() {
                if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..pos]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    expected = Some((pos + 4, length));
                }
            }
            if let Some((body_at, length)) = expected {
                if data.len() >= body_at + length {
                    break;
                }
            }
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&data).to_string()
    }

    fn respond(stream: &mut TcpStream, status: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).ok();
    }

    /// The translation payload's (target, ids, texts).
    fn payload(request: &str) -> (String, Vec<u32>, Vec<String>) {
        let at = request
            .as_bytes()
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap()
            + 4;
        let body: serde_json::Value = serde_json::from_str(request[at..].trim_end()).unwrap();
        let content = body["messages"][1]["content"].as_str().unwrap();
        let payload: serde_json::Value = serde_json::from_str(content).unwrap();
        let segments = payload["segments"].as_array().unwrap();
        (
            payload["target_language"].as_str().unwrap().to_string(),
            segments
                .iter()
                .map(|s| s["id"].as_u64().unwrap() as u32)
                .collect(),
            segments
                .iter()
                .map(|s| s["text"].as_str().unwrap().to_string())
                .collect(),
        )
    }

    fn chat_reply(pairs: &[(u32, &str)]) -> String {
        let segments: Vec<serde_json::Value> = pairs
            .iter()
            .map(|(id, text)| serde_json::json!({ "id": id, "text": text }))
            .collect();
        let content = serde_json::json!({ "segments": segments }).to_string();
        serde_json::json!({ "choices": [{ "message": { "content": content } }] }).to_string()
    }

    /// A mock endpoint serving `requests` connections; returns (url, seen count,
    /// recorded request bodies).
    fn spawn_mock<F>(
        requests: usize,
        handler: F,
    ) -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>)
    where
        F: Fn(usize, &str) -> (String, String) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let c = Arc::clone(&count);
        let s = Arc::clone(&seen);
        thread::spawn(move || {
            for index in 0..requests {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let request = read_request(&mut stream);
                c.fetch_add(1, Ordering::SeqCst);
                s.lock().unwrap().push(request.clone());
                let (status, body) = handler(index, &request);
                respond(&mut stream, &status, &body);
            }
        });
        (format!("http://{addr}/v1"), count, seen)
    }

    fn with_env<T>(tag: &str, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!("suramak-ui-bi-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let previous_xdg = std::env::var_os("XDG_DATA_HOME");
        let previous_key = std::env::var_os("SURAMAK_OPENAI_API_KEY");
        std::env::set_var("XDG_DATA_HOME", &root);
        std::env::set_var("SURAMAK_OPENAI_API_KEY", "test-key");
        let out = f();
        match previous_xdg {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match previous_key {
            Some(value) => std::env::set_var("SURAMAK_OPENAI_API_KEY", value),
            None => std::env::remove_var("SURAMAK_OPENAI_API_KEY"),
        }
        std::fs::remove_dir_all(&root).ok();
        out
    }

    fn write_settings(base_url: &str) {
        let path = paths::settings_path().expect("settings path");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        settings(base_url).save(&path).unwrap();
    }

    fn wait_for_translation(runtime: &mut Runtime, state: &mut AppState) -> Vec<Effect> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(event) = runtime.translation_status() {
                let effects = state.on_translation_event(event);
                if !state.bilingual.is_running() {
                    return effects;
                }
            }
            assert!(
                Instant::now() < deadline,
                "the translation run did not finish in time"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn en_to_id_round_trip_through_the_mock_endpoint() {
        let (url, seen, requests) = spawn_mock(1, |_index, request| {
            let (target, ids, _) = payload(request);
            assert_eq!(target, "id");
            assert_eq!(ids, vec![0, 1]);
            (
                "200 OK".to_string(),
                chat_reply(&[(0, "Halo dunia."), (1, "Ini adalah ujian.")]),
            )
        });
        with_env("en-id", || {
            write_settings(&url);
            let mut runtime = Runtime::new(egui::Context::default());
            assert!(runtime.translation_gate_reason().is_none());

            let mut state = AppState::new();
            state.translation_gate_reason = runtime.translation_gate_reason();
            let transcript = transcript("en", &["Hello world.", "This is a test."]);
            let id = transcript.id;
            Store::open(&paths::db_path().unwrap())
                .unwrap()
                .insert_transcript(&transcript)
                .unwrap();
            state.on_transcript_loaded(transcript);

            let effects = state.start_translation(None);
            runtime.apply(effects, &mut state);
            let effects = wait_for_translation(&mut runtime, &mut state);
            runtime.apply(effects, &mut state);

            let editor = state.editor.as_ref().unwrap();
            assert_eq!(editor.translations[0], "Halo dunia.");
            assert_eq!(editor.translations[1], "Ini adalah ujian.");

            // The store has the rows and their provenance.
            let stored = Store::open(&paths::db_path().unwrap())
                .unwrap()
                .segment_translations(id)
                .unwrap();
            assert_eq!(stored.len(), 2);
            assert_eq!(stored[0].1.text, "Halo dunia.");
            assert_eq!(stored[0].1.model, "mock-model");
            assert_eq!(stored[0].1.source_ids, vec![0]);
            assert!(!stored[0].1.stale);
            assert_eq!(seen.load(Ordering::SeqCst), 1);
            assert_eq!(payload(&requests.lock().unwrap()[0]).1, vec![0, 1]);
        });
    }

    #[test]
    fn id_to_en_round_trip_through_the_mock_endpoint() {
        let (url, _seen, _requests) = spawn_mock(1, |_index, request| {
            let (target, ids, _) = payload(request);
            assert_eq!(target, "en");
            assert_eq!(ids, vec![0, 1]);
            (
                "200 OK".to_string(),
                chat_reply(&[(0, "Hello world."), (1, "Thank you.")]),
            )
        });
        with_env("id-en", || {
            write_settings(&url);
            let mut runtime = Runtime::new(egui::Context::default());
            let mut state = AppState::new();
            state.translation_gate_reason = runtime.translation_gate_reason();
            state.on_transcript_loaded(transcript("id", &["Halo dunia.", "Terima kasih."]));
            assert_eq!(state.bilingual_target(), TargetLanguage::English);

            runtime.apply(state.start_translation(None), &mut state);
            let effects = wait_for_translation(&mut runtime, &mut state);
            runtime.apply(effects, &mut state);
            let editor = state.editor.as_ref().unwrap();
            assert_eq!(editor.translations[0], "Hello world.");
            assert_eq!(editor.translations[1], "Thank you.");
        });
    }

    #[test]
    fn no_endpoint_means_zero_requests_even_when_the_action_is_tried() {
        // A listener that would count a connection if any were made.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        with_env("zero", || {
            // No polish section at all in settings.
            let path = paths::settings_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            Settings::default().save(&path).unwrap();
            let mut runtime = Runtime::new(egui::Context::default());
            assert!(runtime.translation_gate_reason().is_some());
            let mut state = AppState::new();
            state.translation_gate_reason = runtime.translation_gate_reason();
            state.on_transcript_loaded(transcript("en", &["Hello world."]));

            runtime.apply(state.start_translation(None), &mut state);
            assert_eq!(state.bilingual, BilingualState::Idle);
            assert!(
                state
                    .bilingual_status_line()
                    .contains("endpoint configured"),
                "the resolved reason is what the disabled surface shows"
            );
            thread::sleep(Duration::from_millis(150));
            assert!(
                listener.accept().is_err(),
                "no connection may reach {addr} while the endpoint is disabled"
            );
        });
    }

    #[test]
    fn a_mid_flight_cancel_keeps_the_first_batches_and_stops_the_next_request() {
        let (url, seen, _requests) = spawn_mock(3, |index, request| {
            if index == 1 {
                // The second batch is slow enough for the Cancel to land while
                // it is on the wire; the run observes it after the reply.
                thread::sleep(Duration::from_millis(250));
            }
            let (_, ids, _) = payload(request);
            let pairs: Vec<(u32, &str)> = ids.iter().map(|id| (*id, "Halo.")).collect();
            ("200 OK".to_string(), chat_reply(&pairs))
        });
        with_env("cancel", || {
            write_settings(&url);
            let mut runtime = Runtime::new(egui::Context::default());
            let mut state = AppState::new();
            state.translation_gate_reason = runtime.translation_gate_reason();
            let texts: Vec<String> = (0..45).map(|idx| format!("line {idx}")).collect();
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            state.on_transcript_loaded(transcript("en", &refs));

            runtime.apply(state.start_translation(None), &mut state);
            let deadline = Instant::now() + Duration::from_secs(5);
            while seen.load(Ordering::SeqCst) < 1 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            thread::sleep(Duration::from_millis(80));
            runtime.apply(state.cancel_translation(), &mut state);

            let effects = wait_for_translation(&mut runtime, &mut state);
            runtime.apply(effects, &mut state);

            match &state.bilingual {
                BilingualState::Finished { run, .. } => {
                    assert!(run.cancelled, "the run reports the cancellation");
                    assert!(
                        run.batches_done < run.batches_total,
                        "a batch was left unsent: {} of {}",
                        run.batches_done,
                        run.batches_total
                    );
                    assert!(!run.translated.is_empty(), "completed work is kept");
                }
                other => panic!("expected a finished run, got {other:?}"),
            }
            assert!(
                seen.load(Ordering::SeqCst) < 3,
                "no request may follow the observed cancellation"
            );
            assert!(state.status.as_ref().unwrap().text.contains("cancelled"));
        });
    }

    #[test]
    fn opening_another_transcript_resets_the_run_and_the_target() {
        let mut state = state_with("id", &["Halo dunia."]);
        assert_eq!(state.bilingual_target(), TargetLanguage::English);
        state.on_transcript_loaded(transcript("en", &["Hello world."]));
        assert_eq!(state.bilingual_target(), TargetLanguage::Indonesian);
        assert_eq!(state.bilingual, BilingualState::Idle);
        assert_eq!(state.route, Route::Editor);
    }
}
