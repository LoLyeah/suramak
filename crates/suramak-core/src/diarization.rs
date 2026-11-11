//! M16I — diarization: the derived speaker annotation (schema v7).
//!
//! The M16H interface decision, implemented: a diarization result is a
//! **derived, replaceable** annotation of a transcript's audio, stored in three
//! additive tables (`diarization_runs`, `speakers`, `speaker_turns`). Nothing
//! here ever writes `segments.text`, `segments.source_text`, `segments.derived`
//! or `word_timings`; deleting a run restores exactly the pre-diarization view.
//!
//! ## Why turns are time-anchored
//!
//! A turn is `[start_ms, end_ms)` assigned to one app speaker. The segment
//! **range** an editor shows is derived on read ([`speaker_for_segments`],
//! [`speaker_turn_ranges`]) because `segments.idx` is rewritten by split/merge
//! — a stored `(first_idx, last_idx)` pair would silently point at different
//! text after the first edit, which is the one operation labels must survive.
//!
//! ## Manual correction is user data
//!
//! A speaker's label/colour is a row in `speakers` (`source = 'manual'` after a
//! rename or colour change), not a cache: every turn and every export sees the
//! change immediately because they join through `speaker_id`. A re-run matches
//! new clusters to existing speakers by temporal overlap, so a renamed voice
//! keeps its name.
//!
//! ## Staleness
//!
//! Text edits do not move time, but they change the row set the badge
//! projection was computed against; a split/merge changes it more. The run
//! therefore carries a sticky `stale` marker (the M15-B/E rule) that any edit
//! of the transcript's segment set sets, and only a fresh run clears. The UI
//! dims the badges/lane and offers a re-run — the turns themselves stay valid
//! in time and are never rewritten by an edit.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use uuid::Uuid;

use crate::transcript::Segment;

/// The colour slots the Deck palette authors (first N hues, fixed per index).
pub const COLOR_SLOTS: u8 = 6;

/// The largest speaker count the UI offers (Auto/2–6; the segmentation model's
/// powerset covers at most three simultaneous speakers, 4+ is never promised).
pub const MAX_SPEAKERS: u32 = 6;

/// A segment's share of a turn that marks a row as `split` in the editor badge.
pub const SPLIT_SHARE: f32 = 0.25;

/// One diarization attempt for one transcript. Only the latest run is kept;
/// a re-run replaces it in one transaction (derived data, not history).
#[derive(Debug, Clone, PartialEq)]
pub struct DiarizationRun {
    pub transcript_id: Uuid,
    /// Monotonic per transcript (`MAX(run_id) + 1`, read before the old row is
    /// deleted).
    pub run_id: u32,
    pub created_at: String,
    /// Segmentation model file name and sha256 (provenance, not a path).
    pub segmentation_model: String,
    pub segmentation_sha256: String,
    pub embedding_model: String,
    pub embedding_sha256: String,
    /// The runtime the models ran on ("sherpa-onnx 1.12.9"), part of the
    /// provenance so a later model/runtime bump is visible in the row.
    pub engine_version: String,
    /// `0` = threshold/auto. **Never** the binding's default of 4.
    pub num_clusters: i32,
    pub threshold: f32,
    pub min_duration_on: f32,
    pub min_duration_off: f32,
    /// The audio length the turns describe.
    pub audio_ms: i64,
    /// A segment edit happened after this run was written: the badge/lane view
    /// is provisional until a re-run. Turns stay time-valid.
    pub stale: bool,
}

impl DiarizationRun {
    /// A new run row for `transcript_id`, stamped now, not stale.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transcript_id: Uuid,
        run_id: u32,
        segmentation_model: impl Into<String>,
        segmentation_sha256: impl Into<String>,
        embedding_model: impl Into<String>,
        embedding_sha256: impl Into<String>,
        num_clusters: i32,
        threshold: f32,
        audio_ms: i64,
    ) -> Self {
        Self {
            transcript_id,
            run_id,
            created_at: Utc::now().to_rfc3339(),
            segmentation_model: segmentation_model.into(),
            segmentation_sha256: segmentation_sha256.into(),
            embedding_model: embedding_model.into(),
            embedding_sha256: embedding_sha256.into(),
            engine_version: String::new(),
            num_clusters,
            threshold,
            min_duration_on: 0.0,
            min_duration_off: 0.0,
            audio_ms,
            stale: false,
        }
    }

    /// The one-line provenance the speaker row shows ("pyannote-segmentation-3.0
    /// + CAM++ · 3 speakers · auto"), never in the transcript body.
    pub fn provenance_line(&self, speakers: usize) -> String {
        let count = if self.num_clusters > 0 {
            format!("c={}", self.num_clusters)
        } else {
            "auto".to_string()
        };
        let _ = speakers;
        format!(
            "{} + {} · {}",
            short_model(&self.segmentation_model),
            short_model(&self.embedding_model),
            count
        )
    }
}

/// Trims a model file name to its recognisable short name for the provenance
/// line (the pinned pair reads "pyannote-segmentation-3.0 + CAM++"). Anything
/// else keeps its stem.
fn short_model(name: &str) -> &str {
    if name.contains("pyannote-segmentation") {
        "pyannote-segmentation-3.0"
    } else if name.contains("campplus") {
        "CAM++"
    } else {
        name.strip_suffix(".onnx").unwrap_or(name)
    }
}

/// How a speaker row came to be. A rename or colour change makes it `Manual`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerSource {
    Auto,
    Manual,
}

impl SpeakerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SpeakerSource::Auto => "auto",
            SpeakerSource::Manual => "manual",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw == "manual" {
            SpeakerSource::Manual
        } else {
            SpeakerSource::Auto
        }
    }
}

/// One app-level speaker of one transcript. Stable across re-runs so a name is
/// never silently deleted; a speaker with no turns in the active run simply has
/// no rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Speaker {
    pub speaker_id: u32,
    /// `"Speaker 1"` initially, or the user's name after a rename.
    pub label: String,
    /// `0..COLOR_SLOTS`, fixed per speaker; the Deck palette's first N hues.
    pub color_slot: u8,
    pub source: SpeakerSource,
    pub created_at: String,
    pub updated_at: String,
}

impl Speaker {
    pub fn new(speaker_id: u32, label: impl Into<String>, color_slot: u8) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            speaker_id,
            label: label.into(),
            color_slot: color_slot % COLOR_SLOTS,
            source: SpeakerSource::Auto,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Whether the label is still the generated `"Speaker N"` form.
    pub fn has_default_label(&self) -> bool {
        self.label == default_label(self.speaker_id)
    }
}

/// The generated label for a speaker id (`"Speaker 1"`, 1-based).
pub fn default_label(speaker_id: u32) -> String {
    format!("Speaker {}", speaker_id + 1)
}

/// The badge's one or two characters: the numeral of a default label, else the
/// first grapheme of the user's name. Never empty (`?` as the last resort).
pub fn badge_text(label: &str) -> String {
    let trimmed = label.trim();
    if let Some(rest) = trimmed.strip_prefix("Speaker ") {
        let numeral: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
        if !numeral.is_empty() {
            return numeral;
        }
    }
    trimmed
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| "?".to_string())
}

/// How a turn's speaker was assigned (`auto` from clustering, `manual` from a
/// reassign/split in the editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnSource {
    Auto,
    Manual,
}

impl TurnSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnSource::Auto => "auto",
            TurnSource::Manual => "manual",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw == "manual" {
            TurnSource::Manual
        } else {
            TurnSource::Auto
        }
    }
}

/// One contiguous speech span assigned to one speaker. Overlaps are allowed
/// (two rows may overlap — the overlap fixture keeps both identities).
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerTurn {
    pub turn_idx: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_id: u32,
    pub source: TurnSource,
}

impl SpeakerTurn {
    pub fn new(turn_idx: u32, start_ms: i64, end_ms: i64, speaker_id: u32) -> Self {
        Self {
            turn_idx,
            start_ms,
            end_ms,
            speaker_id,
            source: TurnSource::Auto,
        }
    }

    /// Length in milliseconds (never negative; validated rows are positive).
    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }
}

/// One segment row's speaker projection. `secondary` is `Some` only when the
/// row straddles two turns with at least [`SPLIT_SHARE`] each — the badge then
/// shows a split marker (two colours), it never invents a third identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentSpeaker {
    pub primary: u32,
    pub secondary: Option<u32>,
}

/// The largest overlapping turn's speaker for every segment row, computed on
/// read (the stored data is time-anchored).
///
/// Overlap is summed per speaker (a speaker may own two adjacent turns that
/// both touch the row). Ties break to the earliest turn, then the lower speaker
/// id, so the projection is deterministic. A row that no turn touches is
/// `None` (a silent gap between turns — never a fabricated "Speaker ?").
pub fn speaker_for_segments(
    segments: &[Segment],
    turns: &[SpeakerTurn],
) -> Vec<Option<SegmentSpeaker>> {
    let mut sorted: Vec<&SpeakerTurn> = turns.iter().collect();
    sorted.sort_by_key(|turn| (turn.start_ms, turn.turn_idx));

    segments
        .iter()
        .map(|segment| {
            let span = (segment.end_ms - segment.start_ms).max(1) as f32;
            let mut per_speaker: BTreeMap<u32, (i64, i64)> = BTreeMap::new();
            for turn in &sorted {
                let overlap = segment.end_ms.min(turn.end_ms) - segment.start_ms.max(turn.start_ms);
                if overlap <= 0 {
                    continue;
                }
                let entry = per_speaker
                    .entry(turn.speaker_id)
                    .or_insert((0, turn.start_ms));
                entry.0 += overlap;
                entry.1 = entry.1.min(turn.start_ms);
            }
            let mut ranked: Vec<(u32, i64, i64)> = per_speaker
                .into_iter()
                .map(|(speaker, (overlap, first_start))| (speaker, overlap, first_start))
                .collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
            let &(primary, overlap, _) = ranked.first()?;
            let secondary = ranked.get(1).and_then(|&(speaker, second, _)| {
                if second as f32 >= span * SPLIT_SHARE {
                    Some(speaker)
                } else {
                    None
                }
            });
            debug_assert!(overlap > 0);
            Some(SegmentSpeaker { primary, secondary })
        })
        .collect()
}

/// The segment rows whose **midpoint** falls inside each turn — the "segment
/// range" for list operations such as "assign these rows".
///
/// Only non-empty ranges are returned; a turn that covers a pause owns no row.
/// The tuple is `(turn_idx, first_segment_idx, last_segment_idx)`.
pub fn speaker_turn_ranges(segments: &[Segment], turns: &[SpeakerTurn]) -> Vec<(u32, u32, u32)> {
    let mut ranges = Vec::new();
    for turn in turns {
        let mut first: Option<u32> = None;
        let mut last: Option<u32> = None;
        for segment in segments {
            let midpoint = segment.start_ms + (segment.end_ms - segment.start_ms) / 2;
            if midpoint >= turn.start_ms && midpoint < turn.end_ms {
                first.get_or_insert(segment.idx);
                last = Some(segment.idx);
            }
        }
        if let (Some(first), Some(last)) = (first, last) {
            ranges.push((turn.turn_idx, first, last));
        }
    }
    ranges
}

/// Validates a run and its rows before one transaction writes them.
///
/// The whole point is that a malformed result can never reach the tables:
/// speaker ids dense from 0 and colour slots in range, turn idx dense from 0 in
/// time order, every turn positive and inside the audio, labels unique and
/// non-empty. A refusal names the first problem.
pub fn validate(
    run: &DiarizationRun,
    speakers: &[Speaker],
    turns: &[SpeakerTurn],
) -> Result<(), String> {
    if run.audio_ms < 0 {
        return Err("diarization: audio length is negative".to_string());
    }
    if run.num_clusters < 0 || run.num_clusters as u32 > MAX_SPEAKERS {
        return Err(format!(
            "diarization: speaker count {} is outside 0..={MAX_SPEAKERS}",
            run.num_clusters
        ));
    }
    if !(0.0..=1.0).contains(&run.threshold) {
        return Err(format!(
            "diarization: threshold {} is outside 0..=1",
            run.threshold
        ));
    }
    if speakers.is_empty() && !turns.is_empty() {
        return Err("diarization: turns reference no speaker".to_string());
    }
    if speakers.len() > MAX_SPEAKERS as usize {
        return Err(format!(
            "diarization: {} speakers exceeds the {MAX_SPEAKERS}-slot palette",
            speakers.len()
        ));
    }
    let mut labels = BTreeSet::new();
    for (index, speaker) in speakers.iter().enumerate() {
        if speaker.speaker_id != index as u32 {
            return Err(format!(
                "diarization: speaker ids must be dense from 0 (found {} at {index})",
                speaker.speaker_id
            ));
        }
        if speaker.color_slot >= COLOR_SLOTS {
            return Err(format!(
                "diarization: colour slot {} is outside 0..{COLOR_SLOTS}",
                speaker.color_slot
            ));
        }
        let label = speaker.label.trim();
        if label.is_empty() {
            return Err(format!(
                "diarization: speaker {} has an empty label",
                speaker.speaker_id
            ));
        }
        if !labels.insert(label.to_lowercase()) {
            return Err(format!("diarization: duplicate speaker label {label:?}"));
        }
    }
    let known: BTreeSet<u32> = speakers.iter().map(|speaker| speaker.speaker_id).collect();
    let mut previous_start = i64::MIN;
    for (index, turn) in turns.iter().enumerate() {
        if turn.turn_idx != index as u32 {
            return Err(format!(
                "diarization: turn ids must be dense from 0 (found {} at {index})",
                turn.turn_idx
            ));
        }
        if turn.start_ms < 0 || turn.end_ms <= turn.start_ms {
            return Err(format!(
                "diarization: turn {} has an empty or negative span {}..{}",
                turn.turn_idx, turn.start_ms, turn.end_ms
            ));
        }
        if run.audio_ms > 0 && turn.end_ms > run.audio_ms {
            return Err(format!(
                "diarization: turn {} ends at {} ms, past the {} ms audio",
                turn.turn_idx, turn.end_ms, run.audio_ms
            ));
        }
        if turn.start_ms < previous_start {
            return Err(format!(
                "diarization: turn {} starts before the previous turn",
                turn.turn_idx
            ));
        }
        previous_start = turn.start_ms;
        if !known.contains(&turn.speaker_id) {
            return Err(format!(
                "diarization: turn {} references unknown speaker {}",
                turn.turn_idx, turn.speaker_id
            ));
        }
    }
    Ok(())
}

/// Renames one speaker's label in place (the store's rename rule, in memory).
///
/// A user-set name replaces the label everywhere in the run because turns only
/// carry `speaker_id`; the caller persists the one row.
pub fn rename(speakers: &mut [Speaker], speaker_id: u32, label: &str) -> Result<(), String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("a speaker name cannot be empty".to_string());
    }
    if label.chars().count() > 60 {
        return Err("a speaker name is at most 60 characters".to_string());
    }
    if speakers.iter().any(|speaker| {
        speaker.speaker_id != speaker_id && speaker.label.eq_ignore_ascii_case(label)
    }) {
        return Err(format!("another speaker is already called {label:?}"));
    }
    let speaker = speakers
        .iter_mut()
        .find(|speaker| speaker.speaker_id == speaker_id)
        .ok_or_else(|| format!("no speaker {speaker_id}"))?;
    speaker.label = label.to_string();
    speaker.source = SpeakerSource::Manual;
    speaker.updated_at = Utc::now().to_rfc3339();
    Ok(())
}

/// Reassigns one turn to another speaker, marking it manual (M16H §8.3).
pub fn reassign_turn(
    turns: &mut [SpeakerTurn],
    turn_idx: u32,
    speaker_id: u32,
) -> Result<(), String> {
    let turn = turns
        .iter_mut()
        .find(|turn| turn.turn_idx == turn_idx)
        .ok_or_else(|| format!("no turn {turn_idx}"))?;
    if turn.speaker_id == speaker_id {
        return Ok(());
    }
    turn.speaker_id = speaker_id;
    turn.source = TurnSource::Manual;
    Ok(())
}

/// Merges `from` into `into`: moves every turn, then drops `from` from the
/// speaker list and renumbers the remaining ids densely (turn references follow
/// the renumbering).
pub fn merge_speakers(
    speakers: &mut Vec<Speaker>,
    turns: &mut [SpeakerTurn],
    from: u32,
    into: u32,
) -> Result<(), String> {
    if from == into {
        return Ok(());
    }
    if !speakers.iter().any(|s| s.speaker_id == from)
        || !speakers.iter().any(|s| s.speaker_id == into)
    {
        return Err("merge names a speaker that is not in this run".to_string());
    }
    for turn in turns.iter_mut() {
        if turn.speaker_id == from {
            turn.speaker_id = into;
            turn.source = TurnSource::Manual;
        }
    }
    speakers.retain(|speaker| speaker.speaker_id != from);
    renumber(speakers, turns);
    Ok(())
}

/// Moves the given turns to a **new** speaker appended at the end.
pub fn split_speaker(
    speakers: &mut Vec<Speaker>,
    turns: &mut [SpeakerTurn],
    label: &str,
    color_slot: u8,
    turn_idxs: &[u32],
) -> Result<u32, String> {
    if speakers.len() >= MAX_SPEAKERS as usize {
        return Err(format!("at most {MAX_SPEAKERS} speakers"));
    }
    let speaker_id = speakers.len() as u32;
    speakers.push(Speaker::new(speaker_id, label, color_slot));
    let mut moved = 0usize;
    for turn in turns.iter_mut() {
        if turn_idxs.contains(&turn.turn_idx) {
            turn.speaker_id = speaker_id;
            turn.source = TurnSource::Manual;
            moved += 1;
        }
    }
    if moved == 0 {
        speakers.pop();
        return Err("the split selected no turns".to_string());
    }
    Ok(speaker_id)
}

/// Dense renumbering after a speaker is removed: speakers take `0..n` in their
/// existing order, and every turn follows its speaker.
fn renumber(speakers: &mut [Speaker], turns: &mut [SpeakerTurn]) {
    let mut mapping: BTreeMap<u32, u32> = BTreeMap::new();
    for (next, speaker) in speakers.iter_mut().enumerate() {
        mapping.insert(speaker.speaker_id, next as u32);
        speaker.speaker_id = next as u32;
    }
    for turn in turns.iter_mut() {
        if let Some(next) = mapping.get(&turn.speaker_id) {
            turn.speaker_id = *next;
        }
    }
}

/// A run plus its rows, as one read (the UI's whole view of a transcript).
#[derive(Debug, Clone, PartialEq)]
pub struct Diarization {
    pub run: DiarizationRun,
    pub speakers: Vec<Speaker>,
    pub turns: Vec<SpeakerTurn>,
}

impl Diarization {
    /// The label a badge/popover/export shows for a speaker id.
    pub fn label(&self, speaker_id: u32) -> Option<&str> {
        self.speakers
            .iter()
            .find(|speaker| speaker.speaker_id == speaker_id)
            .map(|speaker| speaker.label.as_str())
    }

    /// The colour slot for a speaker id (0 when the id is unknown — the caller
    /// only ever passes ids that came from the same read).
    pub fn color_slot(&self, speaker_id: u32) -> u8 {
        self.speakers
            .iter()
            .find(|speaker| speaker.speaker_id == speaker_id)
            .map(|speaker| speaker.color_slot)
            .unwrap_or(0)
    }

    /// The per-row badge projection for `segments`.
    pub fn segment_speakers(&self, segments: &[Segment]) -> Vec<Option<SegmentSpeaker>> {
        speaker_for_segments(segments, &self.turns)
    }

    /// Whether any manual correction exists (a re-run would discard those turns
    /// and must ask first).
    pub fn has_manual_turns(&self) -> bool {
        self.turns
            .iter()
            .any(|turn| turn.source == TurnSource::Manual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(idx: u32, start_ms: i64, end_ms: i64) -> Segment {
        Segment {
            idx,
            start_ms,
            end_ms,
            text: format!("row {idx}"),
        }
    }

    fn run() -> DiarizationRun {
        DiarizationRun::new(
            Uuid::nil(),
            1,
            "pyannote-segmentation-3.0.onnx",
            "seg-sha",
            "campplus.onnx",
            "emb-sha",
            2,
            0.5,
            10_000,
        )
    }

    #[test]
    fn the_badge_shows_the_numeral_of_a_default_label_or_the_initial_of_a_name() {
        assert_eq!(badge_text("Speaker 1"), "1");
        assert_eq!(badge_text("Speaker 12"), "12");
        assert_eq!(badge_text("Budi"), "B");
        assert_eq!(badge_text("  siti "), "S");
        assert_eq!(badge_text(""), "?");
        assert_eq!(badge_text("Speaker "), "S");
    }

    #[test]
    fn a_silent_run_has_no_speakers_and_is_still_valid() {
        let run = DiarizationRun::new(
            Uuid::new_v4(),
            1,
            "pyannote-segmentation-3.0.onnx",
            "seg",
            "campplus.onnx",
            "emb",
            2,
            0.5,
            5_000,
        );
        assert!(validate(&run, &[], &[]).is_ok());
        let orphan = vec![SpeakerTurn::new(0, 0, 1_000, 0)];
        assert!(validate(&run, &[], &orphan).is_err());
    }

    #[test]
    fn the_projection_uses_the_largest_overlap_and_marks_a_split_at_a_quarter() {
        let turns = vec![
            SpeakerTurn::new(0, 0, 4_000, 0),
            SpeakerTurn::new(1, 4_000, 10_000, 1),
        ];
        let segments = vec![
            segment(0, 0, 3_000),     // fully A
            segment(1, 3_000, 5_000), // 50/50 → split
            segment(2, 4_500, 9_000), // fully B
        ];
        let projected = speaker_for_segments(&segments, &turns);
        assert_eq!(
            projected[0],
            Some(SegmentSpeaker {
                primary: 0,
                secondary: None
            })
        );
        assert_eq!(
            projected[1],
            Some(SegmentSpeaker {
                primary: 0,
                secondary: Some(1)
            }),
            "a 50/50 row is split"
        );
        assert_eq!(
            projected[2],
            Some(SegmentSpeaker {
                primary: 1,
                secondary: None
            })
        );
    }

    #[test]
    fn a_short_overlap_does_not_mark_a_split() {
        // 20 % of the row is A, 80 % B: the badge shows one colour.
        let turns = vec![
            SpeakerTurn::new(0, 0, 1_000, 0),
            SpeakerTurn::new(1, 1_000, 6_000, 1),
        ];
        let projected = speaker_for_segments(&[segment(0, 0, 5_000)], &turns)[0].unwrap();
        assert_eq!(projected.primary, 1);
        assert_eq!(projected.secondary, None);
    }

    #[test]
    fn a_row_between_turns_has_no_speaker_and_a_row_in_overlap_picks_the_dominant() {
        let turns = vec![
            SpeakerTurn::new(0, 0, 1_000, 0),
            SpeakerTurn::new(1, 2_000, 3_000, 1),
        ];
        let projected = speaker_for_segments(&[segment(0, 1_200, 1_800)], &turns);
        assert_eq!(projected[0], None, "silence between turns is nobody");

        // Overlapping turns: the longer intersection wins; the shorter one is
        // still a split share (≥ 25 %), so the badge shows both colours.
        let turns = vec![
            SpeakerTurn::new(0, 0, 3_000, 0),
            SpeakerTurn::new(1, 2_000, 5_000, 1),
        ];
        let projected = speaker_for_segments(&[segment(0, 2_200, 3_800)], &turns)[0].unwrap();
        assert_eq!(projected.primary, 1);
        assert_eq!(projected.secondary, Some(0));
    }

    #[test]
    fn the_turn_range_is_the_midpoint_projection() {
        let turns = vec![
            SpeakerTurn::new(0, 0, 4_000, 0),
            SpeakerTurn::new(1, 4_000, 8_000, 1),
        ];
        let segments = vec![
            segment(0, 0, 3_900),
            segment(1, 3_900, 4_100),
            segment(2, 4_100, 7_900),
            segment(3, 8_100, 9_000),
        ];
        let ranges = speaker_turn_ranges(&segments, &turns);
        // Row 1's midpoint is 4 000 → turn 1. Row 3 is past the last turn.
        assert_eq!(ranges, vec![(0, 0, 0), (1, 1, 2)]);
    }

    #[test]
    fn validation_refuses_dense_id_gaps_colour_slots_and_duplicate_labels() {
        let mut speakers = vec![
            Speaker::new(0, "Speaker 1", 0),
            Speaker::new(1, "Speaker 2", 1),
        ];
        let turns = vec![
            SpeakerTurn::new(0, 0, 1_000, 0),
            SpeakerTurn::new(1, 1_000, 2_000, 1),
        ];
        assert!(validate(&run(), &speakers, &turns).is_ok());

        let mut bad = speakers.clone();
        bad[1].speaker_id = 5;
        assert!(validate(&run(), &bad, &turns).is_err());

        let mut bad = speakers.clone();
        bad[1].color_slot = COLOR_SLOTS;
        assert!(validate(&run(), &bad, &turns).is_err());

        let mut bad = speakers.clone();
        bad[1].label = "speaker 1".to_string();
        assert!(validate(&run(), &bad, &turns).is_err());

        let bad_turns = vec![
            SpeakerTurn::new(0, 1_000, 2_000, 0),
            SpeakerTurn::new(1, 500, 3_000, 1), // starts before the previous
        ];
        assert!(validate(&run(), &speakers, &bad_turns).is_err());

        let past = vec![SpeakerTurn::new(0, 0, 11_000, 0)];
        assert!(validate(&run(), &speakers, &past).is_err());

        speakers[1].label = "Speaker 2".to_string();
        let unknown = vec![SpeakerTurn::new(0, 0, 1_000, 9)];
        assert!(validate(&run(), &speakers, &unknown).is_err());
    }

    #[test]
    fn rename_touches_one_row_and_refuses_an_empty_or_duplicate_name() {
        let mut speakers = vec![
            Speaker::new(0, "Speaker 1", 0),
            Speaker::new(1, "Speaker 2", 1),
        ];
        rename(&mut speakers, 1, "  Budi ").unwrap();
        assert_eq!(speakers[1].label, "Budi");
        assert_eq!(speakers[1].source, SpeakerSource::Manual);
        assert_eq!(speakers[0].label, "Speaker 1");
        assert!(rename(&mut speakers, 0, "  ").is_err());
        assert!(rename(&mut speakers, 0, "budi").is_err());
        assert!(rename(&mut speakers, 9, "x").is_err());
    }

    #[test]
    fn merge_moves_turns_and_renumbers_the_remaining_speakers() {
        let mut speakers = vec![
            Speaker::new(0, "Speaker 1", 0),
            Speaker::new(1, "Speaker 2", 1),
            Speaker::new(2, "Speaker 3", 2),
        ];
        let mut turns = vec![
            SpeakerTurn::new(0, 0, 1_000, 0),
            SpeakerTurn::new(1, 1_000, 2_000, 1),
            SpeakerTurn::new(2, 2_000, 3_000, 2),
        ];
        merge_speakers(&mut speakers, &mut turns, 1, 0).unwrap();
        assert_eq!(speakers.len(), 2);
        assert_eq!(turns[1].speaker_id, 0);
        assert_eq!(turns[1].source, TurnSource::Manual);
        // Speaker 2 (old id 2) becomes id 1; its turn follows.
        assert_eq!(speakers[1].speaker_id, 1);
        assert_eq!(turns[2].speaker_id, 1);
    }

    #[test]
    fn split_appends_a_speaker_and_moves_only_the_selected_turns() {
        let mut speakers = vec![Speaker::new(0, "Speaker 1", 0)];
        let mut turns = vec![
            SpeakerTurn::new(0, 0, 1_000, 0),
            SpeakerTurn::new(1, 1_000, 2_000, 0),
        ];
        let id = split_speaker(&mut speakers, &mut turns, "Speaker 2", 1, &[1]).unwrap();
        assert_eq!(id, 1);
        assert_eq!(turns[0].speaker_id, 0);
        assert_eq!(turns[1].speaker_id, 1);
        assert_eq!(turns[1].source, TurnSource::Manual);
        // An empty selection is a refusal, not an empty speaker.
        assert!(split_speaker(&mut speakers, &mut turns, "Speaker 3", 2, &[9]).is_err());
        assert_eq!(speakers.len(), 2);
    }

    #[test]
    fn provenance_names_the_models_and_never_a_path() {
        let line = run().provenance_line(2);
        assert_eq!(line, "pyannote-segmentation-3.0 + CAM++ · c=2");
        let mut auto = run();
        auto.num_clusters = 0;
        assert!(auto.provenance_line(3).ends_with("· auto"));
    }
}
