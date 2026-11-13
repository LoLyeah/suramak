//! M16I — the editor's diarization state: the loaded run, the badge
//! projection, the rename draft, and the job's visible status.
//!
//! The surface is deliberately restrained (M16H §8.6): badges on segment rows,
//! one 8 px lane above the waveform, a single status row at the top of the
//! transcript. There is no timeline editor and no speaker column anywhere else.
//!
//! The store is the source of truth. Every mutation (run, rename, colour,
//! delete) is one store transaction followed by a re-read; this state only
//! mirrors what the store answered, so a refused write can never leave a
//! half-drawn badge.

use suramak_asr::diarization::{Availability, LaneStatus};
use suramak_core::diarization::{Diarization, SegmentSpeaker, Speaker};
use suramak_core::transcript::Segment;
use suramak_core::Uuid;

/// M25B: diarization's own 30-minute limit, surfaced with the honest route.
/// `None` when the audio fits. The wording is the existing named refusal plus
/// the one action that actually works — split the session into shorter parts.
pub fn length_refusal(audio_ms: i64) -> Option<String> {
    if audio_ms <= suramak_asr::diarization::MAX_AUDIO_MS {
        return None;
    }
    let refusal = suramak_asr::diarization::DiarizationError::TooLong {
        audio_ms,
        cap_ms: suramak_asr::diarization::MAX_AUDIO_MS,
    };
    Some(format!(
        "{refusal} — split into shorter parts to label speakers"
    ))
}

/// The rename-in-place draft: the badge becomes a text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameDraft {
    pub speaker_id: u32,
    pub buffer: String,
    /// Set when the last commit was refused (empty/duplicate name).
    pub error: Option<String>,
}

impl RenameDraft {
    pub fn new(speaker: &Speaker) -> Self {
        Self {
            speaker_id: speaker.speaker_id,
            buffer: speaker.label.clone(),
            error: None,
        }
    }
}

/// The job's visible state (mirrors the runner's lane; no second source of
/// truth for the slot — the runtime owns the lane).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JobState {
    #[default]
    Idle,
    Queued,
    Running {
        pct: u8,
        cancelling: bool,
    },
    Done,
    Failed {
        reason: String,
    },
    /// `discarded` = the run finished after the user asked to stop; the result
    /// was thrown away (the binding's no-abort contract).
    Cancelled {
        discarded: bool,
    },
}

/// Everything the editor needs to draw the speaker surface.
#[derive(Debug, Clone)]
pub struct DiarizationState {
    /// Whether the runner is usable on this build/platform/model install. The
    /// reason is shown verbatim (never a silently absent control).
    pub availability: Availability,
    /// The active run and its rows, for the open transcript only.
    pub view: Option<Diarization>,
    /// The read-time badge projection, parallel to the open transcript's rows.
    pub per_segment: Vec<Option<SegmentSpeaker>>,
    pub job: JobState,
    /// The speaker-count choice: `0` = Auto, `2..=6` explicit.
    pub count: u8,
    pub rename: Option<RenameDraft>,
    /// The delete affordance's in-place confirm.
    pub confirm_delete: bool,
    /// The last operational message (a store refusal, a missing audio path).
    pub note: Option<(String, bool)>,
}

impl Default for DiarizationState {
    fn default() -> Self {
        Self {
            availability: Availability::Unavailable {
                reason: "diarization has not been probed yet".to_string(),
            },
            view: None,
            per_segment: Vec::new(),
            job: JobState::Idle,
            count: 0,
            rename: None,
            confirm_delete: false,
            note: None,
        }
    }
}

impl DiarizationState {
    /// Adopts the store's answer for `id`. A late read for a transcript that is
    /// no longer open is ignored by the caller (it checks the id first).
    pub fn adopt(&mut self, result: Result<Option<Diarization>, String>, segments: &[Segment]) {
        match result {
            Ok(Some(diarization)) => {
                self.per_segment = diarization.segment_speakers(segments);
                self.view = Some(diarization);
                self.note = None;
            }
            Ok(None) => {
                self.view = None;
                self.per_segment = vec![None; segments.len()];
            }
            Err(err) => {
                self.note = Some((err, true));
            }
        }
        // A rename draft for a speaker that no longer exists must not linger.
        if let (Some(draft), Some(view)) = (&self.rename, &self.view) {
            if !view
                .speakers
                .iter()
                .any(|speaker| speaker.speaker_id == draft.speaker_id)
            {
                self.rename = None;
            }
        }
    }

    pub fn has_run(&self) -> bool {
        self.view.is_some()
    }

    pub fn is_stale(&self) -> bool {
        self.view.as_ref().is_some_and(|view| view.run.stale)
    }

    /// The badge for one row (`None` when the transcript has no run). Used by
    /// the surface tests; the editor reads the parallel snapshot instead.
    #[cfg(test)]
    pub fn row(&self, idx: u32) -> Option<SegmentSpeaker> {
        self.per_segment.get(idx as usize).copied().flatten()
    }

    pub fn speaker(&self, speaker_id: u32) -> Option<&Speaker> {
        self.view
            .as_ref()
            .and_then(|view| view.speakers.iter().find(|s| s.speaker_id == speaker_id))
    }

    pub fn label(&self, speaker_id: u32) -> String {
        self.speaker(speaker_id)
            .map(|speaker| speaker.label.clone())
            .unwrap_or_else(|| suramak_core::diarization::default_label(speaker_id))
    }

    pub fn color_slot(&self, speaker_id: u32) -> u8 {
        self.speaker(speaker_id)
            .map(|speaker| speaker.color_slot)
            .unwrap_or(0)
    }

    /// The status row's one line, and whether it is an error.
    pub fn status_line(&self) -> Option<(String, bool)> {
        if let Some((note, error)) = &self.note {
            return Some((note.clone(), *error));
        }
        if let Some(reason) = self.availability.reason() {
            return Some((format!("Speakers unavailable — {reason}"), false));
        }
        match &self.job {
            JobState::Idle => None,
            JobState::Queued => Some(("Speakers · queued…".to_string(), false)),
            JobState::Running { pct, cancelling } => Some((
                if *cancelling {
                    format!("Speakers · stopping after this file ({pct}%)…")
                } else {
                    format!("Speakers · listening for voices ({pct}%)…")
                },
                false,
            )),
            JobState::Done => self
                .view
                .as_ref()
                .map(|view| {
                    if view.speakers.is_empty() {
                        // M16H §6.5: silence is a real answer, not a failure.
                        ("Speakers · no speech found".to_string(), false)
                    } else {
                        (
                            format!(
                                "Speakers · {} · {}",
                                view.speakers.len(),
                                view.run.provenance_line(view.speakers.len())
                            ),
                            false,
                        )
                    }
                })
                .or(Some(("Speakers · no speech found".to_string(), false))),
            JobState::Failed { reason } => Some((format!("Speakers failed — {reason}"), true)),
            JobState::Cancelled { discarded } => Some((
                if *discarded {
                    "Speakers stopped — the result was discarded".to_string()
                } else {
                    "Speakers cancelled".to_string()
                },
                false,
            )),
        }
    }

    /// Whether the job controls are enabled: available, no job running, and the
    /// transcript has audio.
    pub fn can_run(&self, has_audio: bool) -> bool {
        self.availability.is_ready()
            && has_audio
            && !matches!(self.job, JobState::Queued | JobState::Running { .. })
    }

    pub fn is_running(&self) -> bool {
        matches!(self.job, JobState::Queued | JobState::Running { .. })
    }

    /// Starts a rename draft on one speaker (the badge's in-place field).
    pub fn begin_rename(&mut self, speaker_id: u32) -> bool {
        let Some(speaker) = self.speaker(speaker_id).cloned() else {
            return false;
        };
        self.rename = Some(RenameDraft::new(&speaker));
        true
    }

    /// Applies the runner's lane status to the visible state.
    pub fn mirror_lane(&mut self, status: &LaneStatus) {
        self.job = match status {
            LaneStatus::Idle => self.job.clone(),
            LaneStatus::Queued { .. } => JobState::Queued,
            LaneStatus::Running {
                pct, cancelling, ..
            } => JobState::Running {
                pct: *pct,
                cancelling: *cancelling,
            },
            LaneStatus::Done { .. } => JobState::Done,
            LaneStatus::Failed { reason, .. } => JobState::Failed {
                reason: reason.clone(),
            },
            LaneStatus::Cancelled { discarded, .. } => JobState::Cancelled {
                discarded: *discarded,
            },
        };
    }

    /// The label the run's provenance calls the count: "auto" or the number.
    pub fn count_label(&self) -> String {
        match self.count {
            0 => "Auto".to_string(),
            other => format!("{other} speakers"),
        }
    }
}

/// The next count in the selector's cycle (Auto → 2 → 3 → 4 → 5 → 6 → Auto).
pub fn next_count(current: u8) -> u8 {
    match current {
        0 => 2,
        2 => 3,
        3 => 4,
        4 => 5,
        5 => 6,
        _ => 0,
    }
}

impl super::AppState {
    /// Adopts the store's diarization read for the open transcript. A late read
    /// for another id is discarded (the editor may have switched while the
    /// query ran).
    pub fn on_diarization_loaded(&mut self, id: Uuid, result: Result<Option<Diarization>, String>) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.transcript.id != id {
            return;
        }
        let segments = editor.transcript.segments.clone();
        self.diarization.adopt(result, &segments);
    }

    /// Mirrors one runner event into the visible status (the store write is the
    /// runtime's; this never fabricates a run).
    pub fn on_diarization_event(&mut self, event: &suramak_asr::diarization::DiarizationEvent) {
        use suramak_asr::diarization::DiarizationEvent;
        match event {
            DiarizationEvent::Progress { pct, .. } => {
                let cancelling = matches!(
                    self.diarization.job,
                    JobState::Running {
                        cancelling: true,
                        ..
                    }
                );
                self.diarization.job = JobState::Running {
                    pct: *pct,
                    cancelling,
                };
            }
            DiarizationEvent::Done { .. } => {
                self.diarization.job = JobState::Done;
            }
            DiarizationEvent::Failed { reason, .. } => {
                self.diarization.job = JobState::Failed {
                    reason: reason.clone(),
                };
            }
            DiarizationEvent::Cancelled { discarded, .. } => {
                self.diarization.job = JobState::Cancelled {
                    discarded: *discarded,
                };
            }
        }
    }

    /// Starts (or re-runs) the job for the open transcript. The count is the
    /// selector's (`0` = Auto).
    pub fn run_diarization(&mut self) -> Vec<super::Effect> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        if !self.diarization.availability.is_ready() {
            return Vec::new();
        }
        let id = editor.transcript.id;
        let count = self.diarization.count;
        self.diarization.note = None;
        self.diarization.confirm_delete = false;
        vec![super::Effect::RunDiarization { id, count }]
    }

    /// Requests the stop. The visible state turns to `cancelling` only when
    /// the runtime's lane confirms it holds a running job.
    pub fn cancel_diarization(&mut self) -> Vec<super::Effect> {
        if !self.diarization.is_running() {
            return Vec::new();
        }
        vec![super::Effect::CancelDiarization]
    }

    /// Commits the badge's rename draft (Enter or the field's apply).
    pub fn commit_rename(&mut self) -> Vec<super::Effect> {
        let Some(draft) = self.diarization.rename.clone() else {
            return Vec::new();
        };
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let label = draft.buffer.trim().to_string();
        if label.is_empty() {
            if let Some(draft) = self.diarization.rename.as_mut() {
                draft.error = Some("a name cannot be empty".to_string());
            }
            return Vec::new();
        }
        self.diarization.rename = None;
        vec![super::Effect::RenameSpeaker {
            id: editor.transcript.id,
            speaker_id: draft.speaker_id,
            label,
        }]
    }

    /// Abandons the rename draft (Escape).
    pub fn cancel_rename(&mut self) {
        self.diarization.rename = None;
    }

    /// The first click on the delete affordance arms it; the second asks for
    /// the delete. (The editor's existing in-place confirm pattern.)
    pub fn delete_diarization(&mut self) -> Vec<super::Effect> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        if !self.diarization.has_run() {
            return Vec::new();
        }
        if !self.diarization.confirm_delete {
            self.diarization.confirm_delete = true;
            return Vec::new();
        }
        self.diarization.confirm_delete = false;
        self.diarization.rename = None;
        vec![super::Effect::DeleteDiarization {
            id: editor.transcript.id,
        }]
    }

    /// Cycles the speaker-count selector (Auto/2–6).
    pub fn cycle_speaker_count(&mut self) {
        self.diarization.count = next_count(self.diarization.count);
    }

    /// Re-reads the run (after an edit/split/merge marked it stale, or after a
    /// failed mutation whose message the surface should show).
    pub fn refresh_diarization(&mut self) -> Vec<super::Effect> {
        match &self.editor {
            Some(editor) => vec![super::Effect::RefreshDiarization {
                id: editor.transcript.id,
            }],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_asr::diarization::Availability;
    use suramak_core::diarization::{DiarizationRun, Speaker, SpeakerTurn};
    use suramak_core::transcript::Transcript;

    fn segment(idx: u32, start_ms: i64, end_ms: i64) -> Segment {
        Segment {
            idx,
            start_ms,
            end_ms,
            text: format!("row {idx}"),
        }
    }

    fn view(stale: bool) -> Diarization {
        let mut run = DiarizationRun::new(
            Uuid::nil(),
            1,
            "pyannote-segmentation-3.0.onnx",
            "seg",
            "campplus.onnx",
            "emb",
            2,
            0.5,
            4_000,
        );
        run.stale = stale;
        Diarization {
            run,
            speakers: vec![Speaker::new(0, "Speaker 1", 0), Speaker::new(1, "Budi", 1)],
            turns: vec![
                SpeakerTurn::new(0, 0, 2_000, 0),
                SpeakerTurn::new(1, 2_000, 4_000, 1),
            ],
        }
    }

    #[test]
    fn opening_another_transcript_clears_the_previous_speaker_view() {
        let mut state = super::super::AppState::new();
        state.diarization.availability = Availability::Ready;
        let mut first = Transcript::new(
            "satu.wav".to_string(),
            suramak_core::transcript::Source::File {
                name: "satu.wav".to_string(),
            },
            "tiny".to_string(),
            None,
            None,
            vec![segment(0, 0, 1_000)],
        );
        first.id = suramak_core::Uuid::new_v4();
        state.on_transcript_loaded(first.clone());
        let run = DiarizationRun::new(
            first.id,
            1,
            "pyannote-segmentation-3.0.onnx",
            "seg",
            "campplus.onnx",
            "emb",
            2,
            0.6,
            1_000,
        );
        let diarization = Diarization {
            run,
            speakers: vec![Speaker::new(0, "Budi", 0)],
            turns: vec![SpeakerTurn::new(0, 0, 1_000, 0)],
        };
        state
            .diarization
            .adopt(Ok(Some(diarization)), &first.segments);
        state.diarization.begin_rename(0);
        assert!(state.diarization.has_run());

        let mut second = Transcript::new(
            "dua.wav".to_string(),
            suramak_core::transcript::Source::File {
                name: "dua.wav".to_string(),
            },
            "tiny".to_string(),
            None,
            None,
            vec![segment(0, 0, 500)],
        );
        second.id = suramak_core::Uuid::new_v4();
        let _ = state.open_editor(second);
        assert!(!state.diarization.has_run());
        assert!(state.diarization.per_segment.is_empty());
        assert!(state.diarization.rename.is_none());
        assert_eq!(state.diarization.availability, Availability::Ready);
    }

    #[test]
    fn a_stop_that_has_not_returned_reads_as_cancelling() {
        let mut state = DiarizationState {
            availability: Availability::Ready,
            ..Default::default()
        };
        state.mirror_lane(&LaneStatus::Running {
            transcript_id: Uuid::new_v4(),
            pct: 42,
            cancelling: true,
        });
        assert!(state.is_running());
        let (line, error) = state.status_line().unwrap();
        assert!(line.contains("stopping after this file"), "{line}");
        assert!(!error);
        // The slot is still held: the run/stop controls stay in the running
        // branch, and a second submit is impossible.
        assert!(!state.can_run(true));
    }

    #[test]
    fn a_silent_run_says_no_speech_found_rather_than_zero_speakers() {
        let mut run = DiarizationRun::new(
            Uuid::new_v4(),
            1,
            "pyannote-segmentation-3.0.onnx",
            "seg",
            "campplus.onnx",
            "emb",
            0,
            0.5,
            5_000,
        );
        run.stale = false;
        let mut state = DiarizationState {
            availability: Availability::Ready,
            ..Default::default()
        };
        state.adopt(
            Ok(Some(Diarization {
                run,
                speakers: Vec::new(),
                turns: Vec::new(),
            })),
            &[segment(0, 0, 5_000)],
        );
        state.job = JobState::Done;
        assert!(state.has_run());
        assert!(!state.is_stale());
        let (line, error) = state.status_line().unwrap();
        assert_eq!(line, "Speakers · no speech found");
        assert!(!error);
        assert!(state.row(0).is_none());
    }

    #[test]
    fn adopting_a_run_sets_the_badges_and_the_status_line() {
        let mut state = DiarizationState {
            availability: Availability::Ready,
            ..Default::default()
        };
        let segments = vec![segment(0, 0, 1_900), segment(1, 2_100, 3_900)];
        state.adopt(Ok(Some(view(false))), &segments);
        assert!(state.has_run());
        assert_eq!(state.row(0).unwrap().primary, 0);
        assert_eq!(state.row(1).unwrap().primary, 1);
        assert_eq!(state.label(1), "Budi");
        assert_eq!(state.color_slot(1), 1);
        assert!(!state.is_stale());
        state.job = JobState::Done;
        let (line, error) = state.status_line().unwrap();
        assert!(line.contains("Speakers · 2"), "{line}");
        assert!(line.contains("pyannote-segmentation-3.0"), "{line}");
        assert!(!error);
    }

    #[test]
    fn no_run_means_no_badges_and_a_missing_model_names_itself() {
        let mut state = DiarizationState {
            availability: Availability::Unavailable {
                reason: "the diarization model /m/model.onnx is not installed".to_string(),
            },
            ..Default::default()
        };
        state.adopt(Ok(None), &[segment(0, 0, 1_000)]);
        assert!(!state.has_run());
        assert_eq!(state.row(0), None);
        assert!(state.status_line().unwrap().0.contains("/m/model.onnx"));
        assert!(!state.can_run(true));
    }

    #[test]
    fn a_rename_draft_targets_one_speaker_and_never_lingers_on_a_deleted_one() {
        let mut state = DiarizationState::default();
        state.adopt(Ok(Some(view(false))), &[segment(0, 0, 1_000)]);
        assert!(state.begin_rename(1));
        assert_eq!(state.rename.as_ref().unwrap().buffer, "Budi");
        assert!(!state.begin_rename(9));
        // A re-run that drops the speaker clears the draft.
        let mut other = view(false);
        other.speakers.truncate(1);
        state.adopt(Ok(Some(other)), &[segment(0, 0, 1_000)]);
        assert!(state.rename.is_none());
    }

    #[test]
    fn the_job_state_mirrors_the_lane_and_the_count_cycles() {
        let mut state = DiarizationState {
            availability: Availability::Ready,
            ..Default::default()
        };
        state.mirror_lane(&LaneStatus::Running {
            transcript_id: Uuid::nil(),
            pct: 40,
            cancelling: false,
        });
        assert_eq!(
            state.job,
            JobState::Running {
                pct: 40,
                cancelling: false
            }
        );
        assert!(state.is_running());
        state.mirror_lane(&LaneStatus::Running {
            transcript_id: Uuid::nil(),
            pct: 60,
            cancelling: true,
        });
        assert!(state
            .status_line()
            .unwrap()
            .0
            .contains("stopping after this file"));
        state.mirror_lane(&LaneStatus::Cancelled {
            transcript_id: Uuid::nil(),
            discarded: true,
        });
        assert!(state.status_line().unwrap().0.contains("discarded"));

        assert_eq!(next_count(0), 2);
        assert_eq!(next_count(5), 6);
        assert_eq!(next_count(6), 0);
    }

    /// M25B: a part over diarization's 30-minute limit is refused with the
    /// existing named reason *and* the one route that works.
    #[test]
    fn a_long_part_names_the_diarization_limit_and_the_route() {
        let cap = suramak_asr::diarization::MAX_AUDIO_MS;
        assert!(length_refusal(cap).is_none());
        let refusal = length_refusal(cap + 1).expect("1 ms over the limit");
        assert!(refusal.contains("30 min"), "{refusal}");
        assert!(refusal.contains("capped at 30 min"), "{refusal}");
        assert!(
            refusal.ends_with("split into shorter parts to label speakers"),
            "{refusal}"
        );
        // The status line shows it as a note, not a hard error.
        let state = DiarizationState {
            note: Some((refusal, false)),
            ..Default::default()
        };
        let (line, error) = state.status_line().expect("status");
        assert!(line.contains("split into shorter parts"), "{line}");
        assert!(!error);
    }
}
