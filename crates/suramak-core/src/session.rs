//! M25B — long sessions, split at pauses. M26A — the cap is the device's.
//!
//! The M25B gate measured the in-memory single-pass path and capped it at 45
//! minutes on Android / 60 on desktop. Those constants were artifacts of the
//! 3.7 GiB box the measurements came from. M26A replaces them with
//! [`crate::limits::compute_device_limits`]: the cap is derived from this
//! device's memory, the configured model's measured footprint and the free temp
//! disk, and the model-fit gate refuses a model that cannot fit **before** a
//! job starts.
//!
//! This module is the one place that owns:
//!
//! * the **computed** platform cap ([`session_cap_ms`], resolved once from
//!   [`session_limits`]) and its named refusal ([`SessionCapRefusal`], reason
//!   `session_too_long`) carrying the budget and the model's cost;
//! * the silence rule ([`SPLIT_SILENCE_MS`] = 600 ms, grace
//!   [`SPLIT_GRACE_MS`] = 120 s, then the nearest low-energy point with an
//!   honest `no pause found` reason);
//! * the sample-exact part spans and the `Part N of M` label both the file
//!   names and the library carry.
//!
//! Everything here is pure except [`write_split_wavs`] / [`split_audio_file`],
//! which write 16 kHz mono PCM16 parts. A part is a *slice of the same input
//! buffer*; concatenating the written parts is sample-for-sample the
//! uninterrupted capture (the PCM16 conversion is per-sample and deterministic).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::decode;
use crate::limits::{self, Limits, MemoryProbe, ModelFootprint, Platform};
use crate::resample;
use crate::transcript::Segment;
use crate::SuramakError;

/// A real pause of at least this long is a safe cut (M25B's rule).
pub const SPLIT_SILENCE_MS: i64 = 600;
/// How long the recorder may run past the cap waiting for a pause.
pub const SPLIT_GRACE_MS: i64 = 120_000;
/// Silence analysis frame (RMS window).
pub const ANALYSIS_FRAME_MS: i64 = 10;
/// A frame is silent when its RMS is at or below this (≈ −40 dBFS).
pub const SILENCE_RMS: f32 = 0.01;

/// The Android host's memory report, stored before the first cap is resolved.
///
/// Android's `isLowRamDevice` and its `availMem` are only visible from Java, so
/// the host passes `DeviceMemory.Report` here (the M13d plumbing already does).
/// The first report wins: the cap is resolved once per process, so the rotator
/// and the final part write can never disagree about it.
static DEVICE_PROBE: OnceLock<MemoryProbe> = OnceLock::new();
/// The cap this process resolves, once, with the model it was computed for.
static SESSION: OnceLock<(Limits, &'static ModelFootprint)> = OnceLock::new();

/// Records the Android device report for the session cap.
///
/// Called from the host's `device`/`memory` op before any recording or import.
/// A report that arrives after the cap is resolved is ignored (and logged at
/// debug): a mid-recording cap change would make the rotator and the file
/// split disagree.
pub fn set_device_probe(probe: MemoryProbe) {
    if SESSION.get().is_some() {
        tracing::debug!(
            total_bytes = probe.total_bytes,
            "the session cap is already resolved; the late device report is ignored"
        );
        return;
    }
    let _ = DEVICE_PROBE.set(probe);
}

/// The model the app is configured to run — the cap's model half.
///
/// A settings file that names a model outside the measured table is budgeted
/// with `tiny`'s row (the audio slope), exactly like an unknown model path; the
/// app only runs manifest models, so this is a safe last resort rather than a
/// claim about an unmeasured file.
pub fn session_model() -> &'static ModelFootprint {
    let configured = crate::paths::settings_path()
        .ok()
        .and_then(|path| crate::settings::Settings::load(&path).ok())
        .map(|settings| settings.asr.model_id);
    configured
        .as_deref()
        .and_then(limits::footprint_for)
        .or_else(|| limits::footprint_for("tiny"))
        .expect("the model table always carries tiny")
}

/// The computed limits for this device and the configured model.
///
/// Resolved once per process. Precedence: a probe the Android host supplied
/// ([`set_device_probe`]), else this platform's provider, else the documented
/// 60-minute fallback (logged by [`limits::limits_from_probe`]).
///
/// When the configured model does not fit this device at all, the *job* is
/// refused by the model-fit gate; the recording/import cap still has to be a
/// usable number, so it is computed with the **largest model that does fit**
/// (the conservative part size) and the substitution is logged, never silent.
pub fn session_limits() -> Limits {
    session_resolved().0
}

/// The model the cap was computed for.
///
/// Usually [`session_model`]; when the configured model cannot fit at all this
/// is the largest model that does (see [`session_limits`]), so the refusal's
/// arithmetic names the model it actually used.
pub fn session_cap_model() -> &'static ModelFootprint {
    session_resolved().1
}

fn session_resolved() -> (Limits, &'static ModelFootprint) {
    *SESSION.get_or_init(|| {
        let model = session_model();
        let disk = limits::host_free_disk_bytes();
        let compute = |model: &'static ModelFootprint| match DEVICE_PROBE.get() {
            Some(probe) => limits::compute_for(Platform::host(), *probe, model, disk),
            None => limits::limits_for_model(model, disk),
        };
        let primary = compute(model);
        if primary.model_fits {
            return (primary, model);
        }
        let Some(cap_model) = limits::largest_fitting(primary.budget_mb) else {
            return (primary, model);
        };
        tracing::warn!(
            model = model.id,
            cap_model = cap_model.id,
            budget_mb = primary.budget_mb,
            "the configured model does not fit this device; the recording cap uses the largest \
             model that does (the job itself is refused by the model-fit gate)"
        );
        (compute(cap_model), cap_model)
    })
}

/// The cap this build enforces, in milliseconds — `Limits.max_minutes` × 60 s.
pub fn session_cap_ms() -> i64 {
    session_limits().max_ms()
}

/// The byte budget the cap came from (the refusal's `4,284 MB`).
pub fn session_budget_mb() -> u32 {
    session_limits().budget_mb
}

/// `45:00` / `60:00` — the cap's own clock. Minutes never fold into hours
/// here: a 60-minute cap reads `60:00`, not `01:00:00`.
pub fn clock(ms: i64) -> String {
    let total = (ms.max(0) + 500) / 1000;
    format!("{}:{:02}", total / 60, total % 60)
}

/// A refused session: `session_too_long` plus the computed budget and the
/// model's own cost.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionCapRefusal {
    pub audio_ms: i64,
    pub limits: Limits,
    pub model: &'static ModelFootprint,
}

impl SessionCapRefusal {
    /// The stable machine-readable code the queue and the UI key on.
    pub const CODE: &'static str = "session_too_long";

    /// The computed cap in milliseconds.
    pub fn cap_ms(&self) -> i64 {
        self.limits.max_ms()
    }

    /// The named reason the job card shows. It carries the duration, the cap,
    /// the rule that set it, and the device's own budget — never a generic
    /// error and never a number from another machine.
    pub fn reason(&self) -> String {
        format!(
            "{CODE}: this session is {}, over the {} cap ({why}); {budget} — \
             split it into shorter parts",
            clock(self.audio_ms),
            clock(self.cap_ms()),
            why = self.limits.reason.as_str(),
            budget = self.limits.budget_line(self.model),
            CODE = Self::CODE,
        )
    }
}

impl std::fmt::Display for SessionCapRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason())
    }
}

/// `Some` when `audio_ms` is over the computed cap. Exactly at the cap is
/// accepted (a 45:00.000 file is a legal session at that cap); one millisecond
/// over is refused.
///
/// `None` when the model does not fit at all: that refusal belongs to the
/// model-fit gate ([`limits::model_fit_refusal`]), which names the model and
/// the fix instead of pretending the session is too long.
pub fn cap_refusal_for(
    audio_ms: i64,
    limits: Limits,
    model: &'static ModelFootprint,
) -> Option<SessionCapRefusal> {
    if !limits.model_fits {
        return None;
    }
    if audio_ms > limits.max_ms() {
        Some(SessionCapRefusal {
            audio_ms,
            limits,
            model,
        })
    } else {
        None
    }
}

/// [`cap_refusal_for`] against this process's computed cap.
pub fn cap_refusal(audio_ms: i64) -> Option<SessionCapRefusal> {
    cap_refusal_for(audio_ms, session_limits(), session_cap_model())
}

/// Why a cut exists — the row's honest explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitReason {
    /// A real pause of at least [`SPLIT_SILENCE_MS`].
    Pause,
    /// No pause arrived inside the grace window; the cut is at the nearest
    /// low-energy point (or the cap itself) and the row says so.
    NoPause,
    /// The session ended — this is the tail, not a cut.
    End,
}

impl SplitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitReason::Pause => "pause",
            SplitReason::NoPause => "no_pause",
            SplitReason::End => "end",
        }
    }
}

/// One part's half-open sample range `[start, end)` and its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartSpan {
    pub start: usize,
    pub end: usize,
    pub info: PartInfo,
}

impl PartSpan {
    pub fn samples(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn duration_ms(&self, rate: u32) -> i64 {
        if rate == 0 {
            0
        } else {
            (self.samples() as i64) * 1000 / i64::from(rate)
        }
    }
}

/// One cut and why it happened (the library row's `split at …` line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitCut {
    /// Sample index where the next part starts.
    pub at: usize,
    pub at_ms: i64,
    pub reason: SplitReason,
}

impl SplitCut {
    /// `split at 45:00 (pause)` / `split at 45:00 (no pause found)`.
    pub fn line(&self) -> String {
        match self.reason {
            SplitReason::NoPause => format!("split at {} (no pause found)", clock(self.at_ms)),
            SplitReason::Pause => format!("split at {} (pause)", clock(self.at_ms)),
            SplitReason::End => format!("ends at {}", clock(self.at_ms)),
        }
    }
}

/// `Part 2 of 3` — the label the library and the part file names carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartInfo {
    /// 1-based part index.
    pub index: u32,
    /// Total parts in the session.
    pub count: u32,
}

impl PartInfo {
    pub fn new(index: u32, count: u32) -> Self {
        Self { index, count }
    }

    pub fn label(&self) -> String {
        format!("Part {} of {}", self.index, self.count)
    }

    /// Parses the canonical `… — Part N of M` suffix from a title or a file
    /// name (the `.wav` extension is optional). `None` for an ordinary name —
    /// a transcript is a part only when the name says so.
    pub fn from_name(name: &str) -> Option<Self> {
        let stem = Path::new(name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(name);
        let (_, tail) = stem.rsplit_once(" — Part ")?;
        let (index, count) = tail.split_once(" of ")?;
        let index = index.parse::<u32>().ok()?;
        let count = count.parse::<u32>().ok()?;
        if index == 0 || count == 0 || index > count {
            return None;
        }
        Some(Self { index, count })
    }
}

impl std::fmt::Display for PartInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label())
    }
}

/// The canonical part file stem: `{stem} — Part 2 of 3`.
pub fn part_stem(stem: &str, info: &PartInfo) -> String {
    format!("{stem} — {}", info.label())
}

/// The base name of a part (`Meeting — Part 2 of 3.wav` → `Meeting`), so the
/// library title reads `Meeting` and the part label sits beside it instead of
/// being repeated in the title. A non-part name is returned unchanged.
pub fn base_name(name: &str) -> String {
    if PartInfo::from_name(name).is_none() {
        return name.to_string();
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name);
    stem.rsplit_once(" — Part ")
        .map(|(base, _)| base.to_string())
        .unwrap_or_else(|| name.to_string())
}

// ---- M25C: one meeting, N parts ------------------------------------------

/// One transcribed part as the session timeline reads it.
///
/// `duration_ms` is the part's **audio** length, not its last segment's end:
/// a part may end in silence, and the next part starts after all of it. The
/// recording/import path supplies the duration it planned; a caller that only
/// has segments may pass the last segment's end as an honest lower bound.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPartInput {
    pub transcript_id: Uuid,
    pub title: String,
    /// 1-based, as stored on the transcript.
    pub part_index: u32,
    pub part_count: u32,
    pub duration_ms: i64,
    pub segments: Vec<Segment>,
}

/// One part's place on the session-absolute timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPartSpan {
    pub transcript_id: Uuid,
    pub title: String,
    pub part_index: u32,
    pub part_count: u32,
    /// Session-absolute start of this part (sum of every earlier part's
    /// duration). Part 1 is always 0.
    pub offset_ms: i64,
    pub duration_ms: i64,
}

/// The whole meeting: every part's segments on one absolute clock.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTimeline {
    /// Every segment, in part order, with absolute `start_ms`/`end_ms` and a
    /// fresh contiguous `idx` (so the citation validator sees one recording).
    pub segments: Vec<Segment>,
    pub parts: Vec<SessionPartSpan>,
    /// Sum of every part's duration.
    pub total_ms: i64,
}

impl SessionTimeline {
    /// The part a session-absolute time lands in.
    pub fn part_at(&self, ms: i64) -> Option<&SessionPartSpan> {
        self.parts
            .iter()
            .find(|part| ms >= part.offset_ms && ms < part.offset_ms + part.duration_ms)
            .or_else(|| self.parts.last())
    }
}

/// Turns stored transcripts into timeline inputs.
///
/// The part's audio header is the authoritative duration (a part can end in
/// silence after its last word); when the file is gone or unreadable, the last
/// segment's end is the honest lower bound. Titles are reduced to the base
/// name so a session's parts all read as one meeting.
pub fn session_inputs(parts: &[crate::transcript::Transcript]) -> Vec<SessionPartInput> {
    parts
        .iter()
        .map(|part| {
            let segments_end = part
                .segments
                .iter()
                .map(|segment| segment.end_ms)
                .max()
                .unwrap_or(0);
            let probed = part
                .audio_path
                .as_deref()
                .and_then(decode::probe_duration_ms)
                .map(|probed| i64::try_from(probed).unwrap_or(i64::MAX))
                .unwrap_or(segments_end);
            SessionPartInput {
                transcript_id: part.id,
                title: base_name(&part.title),
                part_index: 0,
                part_count: 0,
                duration_ms: probed.max(segments_end).max(0),
                segments: part.segments.clone(),
            }
        })
        .collect()
}

/// Builds the session-absolute timeline.
///
/// Parts are ordered by `part_index` (never by row order or title). The offset
/// is the sum of the preceding parts' durations, so part 2 starts exactly at
/// part 1's length — to the sample, when the caller passes the sample-exact
/// duration. Segment `idx` is reassigned `0..n` across the whole session so a
/// citation can never be ambiguous between two parts that both start at 0.
pub fn session_timeline(parts: &[SessionPartInput]) -> SessionTimeline {
    let mut ordered: Vec<&SessionPartInput> = parts.iter().collect();
    ordered.sort_by_key(|part| part.part_index);

    let mut segments = Vec::new();
    let mut spans = Vec::with_capacity(ordered.len());
    let mut offset_ms = 0i64;
    let mut idx = 0u32;
    for part in ordered {
        spans.push(SessionPartSpan {
            transcript_id: part.transcript_id,
            title: part.title.clone(),
            part_index: part.part_index,
            part_count: part.part_count,
            offset_ms,
            duration_ms: part.duration_ms.max(0),
        });
        for segment in &part.segments {
            segments.push(Segment {
                idx,
                start_ms: offset_ms + segment.start_ms,
                end_ms: offset_ms + segment.end_ms,
                text: segment.text.clone(),
            });
            idx = idx.saturating_add(1);
        }
        offset_ms += part.duration_ms.max(0);
    }
    SessionTimeline {
        segments,
        parts: spans,
        total_ms: offset_ms,
    }
}

/// `2 of 3 parts transcribed` — the one honest line for an incomplete session.
pub fn session_incomplete_line(transcribed: usize, total: usize) -> String {
    format!("{transcribed} of {total} parts transcribed")
}

/// The named refusal for whole-session notes over a session with a hole.
///
/// `None` means every part is present and the run may proceed. The reason
/// starts with `session_incomplete` so the sheet, the log and the tests share
/// one name; the sentence carries the actual `2 of 3` count.
pub fn session_incomplete_refusal(transcribed: usize, total: usize) -> Option<String> {
    if transcribed >= total {
        return None;
    }
    Some(format!(
        "session_incomplete: {} — whole-session notes need every part; \
         transcribe the missing part or use This part",
        session_incomplete_line(transcribed, total)
    ))
}

/// `2 h 10 m`, `42 m`, `45 s` — the length the collapsed library row shows.
pub fn session_length_label(total_ms: i64) -> String {
    let total_s = total_ms.max(0) / 1000;
    let hours = total_s / 3600;
    let minutes = (total_s % 3600) / 60;
    let seconds = total_s % 60;
    if hours > 0 {
        if minutes > 0 {
            format!("{hours} h {minutes} m")
        } else {
            format!("{hours} h")
        }
    } else if minutes > 0 {
        format!("{minutes} m")
    } else {
        format!("{seconds} s")
    }
}

/// `3 parts · 2 h 10 m` — the collapsed library row's meta line.
pub fn session_meta_label(part_count: usize, total_ms: i64) -> String {
    format!(
        "{} part{} · {}",
        part_count,
        if part_count == 1 { "" } else { "s" },
        session_length_label(total_ms)
    )
}

/// The plan for one captured buffer. `parts` always covers `0..samples.len()`
/// exactly once, in order, with no gap and no overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitPlan {
    pub rate: u32,
    pub cap_ms: i64,
    pub parts: Vec<PartSpan>,
    /// One entry per part boundary (never for the tail).
    pub cuts: Vec<SplitCut>,
}

impl SplitPlan {
    /// A single part (the session fits the cap).
    pub fn is_single(&self) -> bool {
        self.parts.len() <= 1
    }

    /// Total samples covered by the parts — the source buffer's length when
    /// the plan is valid.
    pub fn total_samples(&self) -> usize {
        self.parts.iter().map(PartSpan::samples).sum()
    }

    /// The row's honest summary: `2 parts · split at 45:00 (pause)`.
    pub fn summary(&self) -> String {
        if self.parts.len() <= 1 {
            return String::new();
        }
        let cuts = self
            .cuts
            .iter()
            .map(SplitCut::line)
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "{} part{} · {cuts}",
            self.parts.len(),
            if self.parts.len() == 1 { "" } else { "s" }
        )
    }

    /// Every part is at or under the cap (the 50-minute test's assertion).
    pub fn all_within_cap(&self) -> bool {
        let cap_samples =
            (self.cap_ms.max(0) as f64 * f64::from(self.rate) / 1000.0).ceil() as usize;
        self.parts.iter().all(|part| part.samples() <= cap_samples)
    }
}

fn max_samples(rate: u32, ms: i64) -> usize {
    if rate == 0 || ms <= 0 {
        return 0;
    }
    ((ms as f64) * f64::from(rate) / 1000.0).round() as usize
}

fn frame_samples(rate: u32) -> usize {
    ((i64::from(rate) * ANALYSIS_FRAME_MS / 1000).max(1)) as usize
}

fn frame_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Plans the cuts with the shipping rule. See [`plan_split_with`].
pub fn plan_split(samples: &[f32], rate: u32, cap_ms: i64) -> SplitPlan {
    plan_split_with(samples, rate, cap_ms, SPLIT_GRACE_MS, SILENCE_RMS)
}

/// The split rule, with the grace/threshold exposed for tests.
///
/// At the cap the next qualifying pause (≥ [`SPLIT_SILENCE_MS`]) is used:
///
/// * a pause that straddles the cap cuts **exactly at the cap** (the part stays
///   at the cap, never over it);
/// * a pause that starts after the cap cuts at the pause's first sample (the
///   recorder kept writing until the pause arrived);
/// * no pause inside `grace_ms` cuts at the lowest-energy 100 ms window and
///   records [`SplitReason::NoPause`] — the row says `no pause found`.
pub fn plan_split_with(
    samples: &[f32],
    rate: u32,
    cap_ms: i64,
    grace_ms: i64,
    silence_rms: f32,
) -> SplitPlan {
    if rate == 0 || cap_ms <= 0 {
        return SplitPlan {
            rate,
            cap_ms,
            parts: vec![PartSpan {
                start: 0,
                end: samples.len(),
                info: PartInfo::new(1, 1),
            }],
            cuts: Vec::new(),
        };
    }
    let cap = max_samples(rate, cap_ms).max(1);
    if samples.len() <= cap {
        return SplitPlan {
            rate,
            cap_ms,
            parts: vec![PartSpan {
                start: 0,
                end: samples.len(),
                info: PartInfo::new(1, 1),
            }],
            cuts: Vec::new(),
        };
    }

    let frame = frame_samples(rate);
    let frames = samples.len().div_ceil(frame);
    let silent: Vec<bool> = (0..frames)
        .map(|index| {
            let start = index * frame;
            let end = (start + frame).min(samples.len());
            frame_rms(&samples[start..end]) <= silence_rms
        })
        .collect();
    let run_frames = (max_samples(rate, SPLIT_SILENCE_MS) / frame).max(1);

    let mut parts = Vec::new();
    let mut cuts = Vec::new();
    let mut start = 0usize;
    while samples.len() - start > cap {
        let cap_at = start + cap;
        let grace_end = (cap_at + max_samples(rate, grace_ms.max(0))).min(samples.len());
        let (at, reason) = find_cut(samples, rate, frame, &silent, cap_at, grace_end, run_frames);
        if at <= start || at >= samples.len() {
            break;
        }
        let info = PartInfo::new((parts.len() + 1) as u32, 0);
        parts.push(PartSpan {
            start,
            end: at,
            info,
        });
        cuts.push(SplitCut {
            at,
            at_ms: (at as i64) * 1000 / i64::from(rate),
            reason,
        });
        start = at;
    }
    let count = (parts.len() + 1) as u32;
    for (index, part) in parts.iter_mut().enumerate() {
        part.info = PartInfo::new((index + 1) as u32, count);
    }
    parts.push(PartSpan {
        start,
        end: samples.len(),
        info: PartInfo::new(count, count),
    });
    SplitPlan {
        rate,
        cap_ms,
        parts,
        cuts,
    }
}

/// Finds the cut for one part: a qualifying pause in `[cap_at, grace_end]`,
/// else the nearest low-energy point.
fn find_cut(
    samples: &[f32],
    rate: u32,
    frame: usize,
    silent: &[bool],
    cap_at: usize,
    grace_end: usize,
    run_frames: usize,
) -> (usize, SplitReason) {
    let cap_frame = cap_at / frame;
    let end_frame = grace_end.div_ceil(frame);

    // The first run of silent frames that reaches `run_frames`, scanning from
    // the cap. A run already in progress at the cap counts if it is long enough
    // (`cap_frame` is inside it).
    let mut run_start: Option<usize> = None;
    let mut index = cap_frame;
    let mut first_run: Option<(usize, usize)> = None;
    // Start the scan at the beginning of the run containing the cap, so a pause
    // that straddles the cap is seen whole.
    while index > 0 && silent.get(index).copied().unwrap_or(false) {
        index -= 1;
    }
    let scan_start = index;
    let mut cursor = scan_start;
    while cursor < end_frame {
        if silent.get(cursor).copied().unwrap_or(false) {
            run_start.get_or_insert(cursor);
        } else if let Some(start) = run_start.take() {
            if cursor - start >= run_frames && (cursor * frame) > cap_at {
                first_run = Some((start, cursor));
                break;
            }
        }
        cursor += 1;
    }
    if first_run.is_none() {
        if let Some(start) = run_start {
            if cursor - start >= run_frames && (cursor * frame) > cap_at {
                first_run = Some((start, cursor));
            }
        }
    }
    if let Some((start_frame, _end)) = first_run {
        let run_start_sample = start_frame * frame;
        let at = if run_start_sample <= cap_at {
            // The cap lands inside the pause: cut exactly at the cap.
            cap_at
        } else if run_start_sample < grace_end {
            // The pause begins after the cap: cut at its first sample.
            run_start_sample
        } else {
            cap_at
        };
        if at > 0 && at < samples.len() {
            return (at, SplitReason::Pause);
        }
    }

    // No pause: cut at the nearest low-energy point **at or before the cap**,
    // so a part never exceeds the cap just because no pause arrived. The 120 s
    // grace was spent looking; the split still lands on the best quiet spot
    // available in the last two seconds before the cap. The row says the truth
    // (`no pause found`).
    let window = max_samples(rate, 100).max(frame);
    let search_start = cap_at.saturating_sub(max_samples(rate, 2_000));
    let mut best: Option<(f32, usize)> = None;
    let mut at = search_start;
    while at + window <= cap_at {
        let energy = frame_rms(&samples[at..at + window]);
        if best.is_none_or(|(best_energy, _)| energy < best_energy) {
            best = Some((energy, at));
        }
        at += frame;
    }
    match best {
        Some((_, start)) => ((start + window).min(cap_at), SplitReason::NoPause),
        None => (cap_at.min(samples.len()), SplitReason::NoPause),
    }
}

/// The live half of the rotation rule: fed the same 16 kHz mixed chunks the
/// recorder's live meter sees, it answers **when** a part crossed a pause. The
/// UI uses it for the counter reset; the files themselves are cut from the
/// final mixed buffer with [`plan_split`], so the two never disagree about the
/// samples — only about when the news arrives.
#[derive(Debug, Clone)]
pub struct LiveSplitTracker {
    rate: u32,
    cap_samples: usize,
    pos: usize,
    part_start: usize,
    frame_len: usize,
    frame_count: usize,
    frame_sq: f64,
    silent_run: usize,
    seeking: bool,
    silence_rms: f32,
    grace_samples: usize,
}

impl LiveSplitTracker {
    pub fn new(rate: u32, cap_ms: i64) -> Self {
        let frame_len = frame_samples(rate);
        Self {
            rate,
            cap_samples: max_samples(rate, cap_ms).max(1),
            pos: 0,
            part_start: 0,
            frame_len,
            frame_count: 0,
            frame_sq: 0.0,
            silent_run: 0,
            seeking: false,
            silence_rms: SILENCE_RMS,
            grace_samples: max_samples(rate, SPLIT_GRACE_MS),
        }
    }

    /// Feeds one chunk; `Some(global_sample)` when the part just rotated.
    pub fn push(&mut self, samples: &[f32]) -> Option<usize> {
        let silence_run_needed = max_samples(self.rate, SPLIT_SILENCE_MS);
        let mut rotated = None;
        for &sample in samples {
            self.pos += 1;
            self.frame_sq += f64::from(sample) * f64::from(sample);
            self.frame_count += 1;
            if self.frame_count < self.frame_len {
                continue;
            }
            let rms = (self.frame_sq / self.frame_count as f64).sqrt() as f32;
            self.frame_sq = 0.0;
            self.frame_count = 0;
            if self.pos - self.part_start >= self.cap_samples {
                self.seeking = true;
            }
            if self.seeking {
                if rms <= self.silence_rms {
                    self.silent_run += self.frame_len;
                } else {
                    self.silent_run = 0;
                }
                let grace_over =
                    self.pos - self.part_start >= self.cap_samples + self.grace_samples;
                if self.silent_run >= silence_run_needed || grace_over {
                    self.part_start = self.pos;
                    self.seeking = false;
                    self.silent_run = 0;
                    rotated = Some(self.pos);
                }
            }
        }
        rotated
    }
}

/// One written part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPart {
    pub path: PathBuf,
    pub span: PartSpan,
}

/// The written plan for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitOutput {
    pub rate: u32,
    pub plan: SplitPlan,
    pub parts: Vec<WrittenPart>,
}

/// Writes every part as 16 kHz mono PCM16 WAV (`{stem} — Part N of M.wav`).
///
/// Each file is a slice of `samples`; the PCM16 conversion is per-sample and
/// deterministic, so reading the parts back in order reproduces the same
/// samples as writing `samples` as one WAV.
pub fn write_split_wavs(
    samples: &[f32],
    rate: u32,
    cap_ms: i64,
    dir: &Path,
    stem: &str,
) -> Result<SplitOutput, SuramakError> {
    std::fs::create_dir_all(dir)?;
    let plan = plan_split(samples, rate, cap_ms);
    let mut written = Vec::with_capacity(plan.parts.len());
    let single = plan.is_single();
    for span in &plan.parts {
        let name = if single {
            format!("{stem}.wav")
        } else {
            format!("{}.wav", part_stem(stem, &span.info))
        };
        let path = dir.join(name);
        decode::write_wav_i16(&path, &samples[span.start..span.end], rate, 1)?;
        written.push(WrittenPart {
            path,
            span: span.clone(),
        });
    }
    Ok(SplitOutput {
        rate,
        plan,
        parts: written,
    })
}

/// Decodes `input` once, resamples to 16 kHz mono, and writes its parts under
/// `dir`. `None` when the file already fits the cap (nothing is written and the
/// caller keeps the original path — no needless re-encode).
///
/// This is the import path's front half. It is **not** chunked decoding for
/// transcription: each part becomes its own queued job and is decoded again by
/// that job, exactly like a file the user imported.
pub fn split_audio_file(
    input: &Path,
    dir: &Path,
    cap_ms: i64,
) -> Result<Option<SplitOutput>, SuramakError> {
    if let Some(duration) = decode::probe_duration_ms(input) {
        if duration as i64 <= cap_ms {
            return Ok(None);
        }
    }
    let buffer = decode::decode_file(input)?;
    let samples = resample::to_16khz_mono(&buffer)?;
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("import");
    let output = write_split_wavs(&samples, resample::TARGET_RATE, cap_ms, dir, stem)?;
    if output.plan.is_single() {
        // The header lied (or the decode came in under the cap): do not leave a
        // pointless copy; the caller keeps the original file.
        for part in &output.parts {
            let _ = std::fs::remove_file(&part.path);
        }
        return Ok(None);
    }
    Ok(Some(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(ms: i64, rate: u32) -> Vec<f32> {
        let samples = max_samples(rate, ms);
        (0..samples)
            .map(|index| {
                let phase = 2.0 * std::f32::consts::PI * 220.0 * index as f32 / rate as f32;
                0.4 * phase.sin()
            })
            .collect()
    }

    fn silence(ms: i64, rate: u32) -> Vec<f32> {
        vec![0.0; max_samples(rate, ms)]
    }

    fn cat(parts: &[Vec<f32>]) -> Vec<f32> {
        let mut out = Vec::new();
        for part in parts {
            out.extend_from_slice(part);
        }
        out
    }

    /// A phone's limits with the design's total-only budget (`availMem`
    /// unwired), which is how the design's worked table is computed.
    fn phone_limits(model: &'static ModelFootprint, total: u64, low_ram: bool) -> Limits {
        limits::compute_device_limits(
            Platform::Android,
            total,
            None,
            low_ram,
            model.baseline_mb,
            model.slope_mb_per_min,
            Some(64 * 1024 * 1024 * 1024),
        )
    }

    #[test]
    fn the_cap_is_computed_per_device_not_a_static_constant() {
        let tiny = limits::footprint_for("tiny").expect("the table model");
        let four = phone_limits(tiny, 4_000_000_000, false);
        let twelve = phone_limits(tiny, 12_000_000_000, false);
        assert_eq!(four.max_minutes, 77);
        assert_eq!(twelve.max_minutes, 431);
        assert_ne!(
            four.max_minutes, twelve.max_minutes,
            "two devices, two caps — the point of the slice"
        );

        // The shipping function is the host's own computation, in minutes.
        println!(
            "M26A_SESSION model={} cap_model={} cap_ms={} limits={:?}",
            session_model().id,
            session_cap_model().id,
            session_cap_ms(),
            session_limits()
        );
        assert_eq!(session_cap_ms(), session_limits().max_ms());
        assert_eq!(
            session_cap_ms(),
            i64::from(session_limits().max_minutes) * 60_000
        );
        assert_eq!(session_budget_mb(), session_limits().budget_mb);
        assert!(session_limits().max_minutes <= limits::MAX_MINUTES_CEILING);
        assert!(
            session_limits().max_minutes >= 1,
            "a recording cap is never zero, even when the configured model must be \
             refused: {:?}",
            session_limits()
        );
        // The configured model resolves through the one model table.
        assert!(limits::MODEL_FOOTPRINTS
            .iter()
            .any(|model| model.id == session_model().id));
    }

    #[test]
    fn exactly_at_the_computed_cap_is_accepted_and_one_millisecond_over_is_refused() {
        let tiny = limits::footprint_for("tiny").expect("the table model");
        let limits = phone_limits(tiny, 4_000_000_000, false);
        let cap = limits.max_ms();
        assert_eq!(cap, 77 * 60_000);
        assert!(cap_refusal_for(cap, limits, tiny).is_none());
        let refused = cap_refusal_for(cap + 1, limits, tiny).expect("1 ms over is refused");
        assert_eq!(refused.audio_ms, cap + 1);
        assert_eq!(refused.cap_ms(), cap);
        let reason = refused.reason();
        assert!(reason.starts_with("session_too_long:"), "{reason}");
        assert!(reason.contains("split it into shorter parts"), "{reason}");
        // The cap, its rule and the device's budget, not another machine's.
        assert!(reason.contains("77:00"), "{reason}");
        assert!(reason.contains("(memory)"), "{reason}");
        assert!(
            reason.contains("this device budgets 1,020 MB; tiny needs 310 MB + 9.2 MB per minute"),
            "{reason}"
        );
    }

    #[test]
    fn the_refusal_carries_the_computed_budget_and_the_model_cost() {
        let tiny = limits::footprint_for("tiny").expect("the table model");
        let limits = phone_limits(tiny, 12_000_000_000, false);
        assert_eq!(limits.budget_mb, 4_284);
        let refused = cap_refusal_for(limits.max_ms() + 62_000, limits, tiny).expect("over");
        let reason = refused.reason();
        assert!(reason.contains("431:00"), "{reason}");
        assert!(reason.contains("432:02"), "{reason}");
        assert!(
            reason.contains("this device budgets 4,284 MB; tiny needs 310 MB + 9.2 MB per minute"),
            "{reason}"
        );
        assert!(!reason.contains("4 GB phone"), "{reason}");
    }

    #[test]
    fn the_session_refusal_steps_aside_when_the_model_does_not_fit() {
        let turbo = limits::footprint_for("turbo-q5-0").expect("the table model");
        let limits = phone_limits(turbo, 4_000_000_000, false);
        assert!(!limits.model_fits);
        assert!(
            cap_refusal_for(1_000, limits, turbo).is_none(),
            "the model-fit gate owns a model that cannot load"
        );
    }

    /// The rotator reads the **computed** cap: a recording that reaches it
    /// rotates there (the live tracker at the cap + grace, the file plan at the
    /// cap). A 10 Hz fixture keeps the test cheap while the cap stays the real
    /// 4 GB phone's 77 minutes on tiny.
    #[test]
    fn a_recording_at_the_computed_cap_rotates_there() {
        let tiny = limits::footprint_for("tiny").expect("the table model");
        let limits = phone_limits(tiny, 4_000_000_000, false);
        let cap_ms = limits.max_ms();
        let rate = 10u32;
        let cap_samples = max_samples(rate, cap_ms);
        let samples = vec![0.05f32; cap_samples + max_samples(rate, 180_000)];
        let plan = plan_split(&samples, rate, cap_ms);
        assert!(plan.parts.len() >= 2, "{}", plan.summary());
        assert!(plan.all_within_cap());
        assert_eq!(plan.cuts[0].reason, SplitReason::NoPause);
        let cut = plan.parts[0].samples();
        assert!(
            cut <= cap_samples && cut + max_samples(rate, 2_000) > cap_samples,
            "the no-pause cut lands at or before the computed cap ({cut} vs {cap_samples})"
        );

        // The live rotator: constant speech, so it rotates at cap + grace.
        let mut tracker = LiveSplitTracker::new(rate, cap_ms);
        let mut rotated = None;
        for chunk in samples.chunks(rate as usize) {
            if let Some(at) = tracker.push(chunk) {
                rotated = Some(at);
                break;
            }
        }
        let at = rotated.expect("the tracker rotates at the computed cap");
        assert_eq!(at, cap_samples + max_samples(rate, SPLIT_GRACE_MS));
    }

    #[test]
    fn a_short_session_is_one_part() {
        let samples = speech(1_000, 16_000);
        let plan = plan_split(&samples, 16_000, session_cap_ms());
        assert!(plan.is_single());
        assert_eq!(plan.parts.len(), 1);
        assert_eq!(plan.parts[0].start, 0);
        assert_eq!(plan.parts[0].end, samples.len());
        assert_eq!(plan.parts[0].info.label(), "Part 1 of 1");
        assert!(plan.cuts.is_empty());
    }

    #[test]
    fn a_pause_straddling_the_cap_cuts_exactly_at_the_cap() {
        let rate = 16_000;
        // 10 s cap; speech 0..9.7 s, silence 9.7..10.3 s, speech 10.3..12 s.
        let mut samples = speech(9_700, rate);
        samples.extend(silence(600, rate));
        samples.extend(speech(1_700, rate));
        let plan = plan_split(&samples, rate, 10_000);
        assert_eq!(plan.parts.len(), 2);
        assert_eq!(plan.cuts.len(), 1);
        assert_eq!(plan.cuts[0].at, 160_000, "cut exactly at the 10 s cap");
        assert_eq!(plan.cuts[0].reason, SplitReason::Pause);
        assert_eq!(plan.parts[0].samples(), 160_000);
        assert_eq!(plan.parts[0].info.label(), "Part 1 of 2");
        assert_eq!(plan.parts[1].info.label(), "Part 2 of 2");
        assert_eq!(plan.total_samples(), samples.len());
        assert!(plan.all_within_cap());
    }

    #[test]
    fn a_pause_after_the_cap_is_where_the_recorder_stops() {
        let rate = 16_000;
        // 10 s cap; the next real pause starts at 11 s.
        let mut samples = speech(11_000, rate);
        samples.extend(silence(600, rate));
        samples.extend(speech(2_000, rate));
        let plan = plan_split(&samples, rate, 10_000);
        assert_eq!(plan.parts.len(), 2);
        assert_eq!(plan.cuts[0].at, 176_000, "cut at the pause's first sample");
        assert_eq!(plan.cuts[0].reason, SplitReason::Pause);
        assert_eq!(plan.cuts[0].at_ms, 11_000);
    }

    #[test]
    fn no_pause_inside_the_grace_splits_at_low_energy_and_says_so() {
        let rate = 16_000;
        // 10 s cap; no pause anywhere, but a quiet notch at 9.5 s.
        let mut samples = speech(9_500, rate);
        samples.extend(vec![0.002; max_samples(rate, 100)]);
        samples.extend(speech(12_000, rate));
        let plan = plan_split(&samples, rate, 10_000);
        assert!(plan.parts.len() >= 2);
        assert_eq!(plan.cuts[0].reason, SplitReason::NoPause);
        assert!(
            plan.summary().contains("(no pause found)"),
            "{}",
            plan.summary()
        );
        // The cut is the quiet notch's end, at or before the cap.
        assert_eq!(plan.cuts[0].at_ms, 9_600);
        assert!(plan.all_within_cap());
    }

    #[test]
    fn a_silence_shorter_than_600_ms_is_not_a_cut() {
        let rate = 16_000;
        // 10 s cap; a 300 ms gap at 10.2 s; then a real 600 ms pause at 12 s.
        let mut samples = speech(10_200, rate);
        samples.extend(silence(300, rate));
        samples.extend(speech(1_500, rate));
        samples.extend(silence(600, rate));
        samples.extend(speech(1_000, rate));
        let plan = plan_split(&samples, rate, 10_000);
        assert_eq!(plan.cuts.len(), 1);
        assert_eq!(plan.cuts[0].at_ms, 12_000);
        assert_eq!(plan.cuts[0].reason, SplitReason::Pause);
    }

    #[test]
    fn many_parts_are_numbered_densely_and_cover_every_sample() {
        let rate = 16_000;
        // 3 × 8 s of speech separated by 1 s pauses; 8 s cap → 3 parts, each cut
        // inside a pause that straddles the cap.
        let mut samples = Vec::new();
        for _ in 0..3 {
            samples.extend(speech(7_900, rate));
            samples.extend(silence(1_000, rate));
        }
        samples.extend(speech(3_000, rate));
        let plan = plan_split(&samples, rate, 8_000);
        assert_eq!(plan.parts.len(), 4);
        assert_eq!(plan.total_samples(), samples.len());
        // Concatenating the plan's spans is the input, sample for sample.
        let rebuilt = cat(&plan
            .parts
            .iter()
            .map(|part| samples[part.start..part.end].to_vec())
            .collect::<Vec<_>>());
        assert_eq!(rebuilt, samples);
        for (index, part) in plan.parts.iter().enumerate() {
            assert_eq!(part.info.index as usize, index + 1);
            assert_eq!(part.info.count as usize, plan.parts.len());
            if index > 0 {
                assert_eq!(part.start, plan.parts[index - 1].end);
            }
        }
    }

    #[test]
    fn part_labels_round_trip_and_refuse_ordinary_names() {
        let info = PartInfo::new(2, 3);
        assert_eq!(info.label(), "Part 2 of 3");
        let stem = part_stem("Meeting", &info);
        assert_eq!(stem, "Meeting — Part 2 of 3");
        assert_eq!(PartInfo::from_name(&stem), Some(info.clone()));
        assert_eq!(PartInfo::from_name("Meeting — Part 2 of 3.wav"), Some(info));
        assert_eq!(PartInfo::from_name("Meeting"), None);
        assert_eq!(PartInfo::from_name("Part 0 of 3"), None);
        assert_eq!(PartInfo::from_name("Part 4 of 3"), None);
        assert_eq!(base_name("Meeting — Part 2 of 3.wav"), "Meeting");
        assert_eq!(base_name("Meeting.wav"), "Meeting.wav");
    }

    #[test]
    fn the_summary_names_the_cut_and_why() {
        let rate = 16_000;
        let mut samples = speech(9_800, rate);
        samples.extend(silence(700, rate));
        samples.extend(speech(1_000, rate));
        let plan = plan_split(&samples, rate, 10_000);
        let summary = plan.summary();
        assert!(
            summary.starts_with("2 parts · split at 0:10 (pause)"),
            "{summary}"
        );
    }

    #[test]
    fn the_live_tracker_reports_a_rotation_at_a_pause_and_resets_its_part() {
        let rate = 16_000;
        let mut tracker = LiveSplitTracker::new(rate, 10_000);
        assert!(tracker.push(&speech(9_900, rate)).is_none());
        // 1 s of silence: the 600 ms rule fires inside it.
        let cut = tracker.push(&silence(1_000, rate)).expect("rotation");
        assert!((cut as i64) >= 169_000, "cut {cut}");
        assert!((cut as i64) <= 160_000 + 9_600 + 160);
        // The next part starts now; a short tail does not rotate again.
        assert!(tracker.push(&speech(2_000, rate)).is_none());
    }

    #[test]
    fn the_live_tracker_rotates_at_the_grace_boundary_with_no_pause() {
        let rate = 16_000;
        // A tiny cap and grace keeps the test fast: 1 s cap, 120 s grace is the
        // shipping rule; feed constant speech and the tracker answers at the
        // grace edge.
        let mut tracker = LiveSplitTracker::new(rate, 1_000);
        let mut cut = None;
        for _ in 0..125 {
            if let Some(at) = tracker.push(&speech(1_000, rate)) {
                cut = Some(at);
                break;
            }
        }
        let cut = cut.expect("grace rotation");
        assert_eq!(
            cut,
            16_000 + 16_000 * 120,
            "the 1 s cap plus the 120 s grace"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "suramak-session-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn decode_concat(paths: &[PathBuf]) -> Vec<f32> {
        let mut out = Vec::new();
        for path in paths {
            let buffer = decode::decode_file(path).expect("decode a part");
            out.extend_from_slice(&buffer.samples);
        }
        out
    }

    #[test]
    fn written_parts_concatenate_to_the_uninterrupted_capture_sample_for_sample() {
        let rate = 16_000;
        let dir = scratch("concat");
        // 9.8 s speech + 0.7 s pause + 2 s speech; 10 s cap straddles the pause.
        let mut samples = speech(9_800, rate);
        samples.extend(silence(700, rate));
        samples.extend(speech(2_000, rate));
        let whole_path = dir.join("whole.wav");
        decode::write_wav_i16(&whole_path, &samples, rate, 1).expect("write whole");
        let output =
            write_split_wavs(&samples, rate, 10_000, &dir.join("parts"), "Meeting").expect("split");
        assert_eq!(output.parts.len(), 2);
        assert_eq!(
            output.parts[0].path.file_name().unwrap(),
            "Meeting — Part 1 of 2.wav"
        );
        assert_eq!(
            output.parts[1].path.file_name().unwrap(),
            "Meeting — Part 2 of 2.wav"
        );
        let whole = decode::decode_file(&whole_path).expect("whole decode");
        let parts: Vec<PathBuf> = output.parts.iter().map(|part| part.path.clone()).collect();
        let joined = decode_concat(&parts);
        assert_eq!(joined.len(), whole.samples.len(), "same total sample count");
        assert_eq!(joined, whole.samples, "sample-exact concatenation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_import_over_the_cap_splits_into_two_parts_with_metadata() {
        let rate = 16_000;
        let dir = scratch("import2");
        let input = dir.join("Interview.wav");
        let mut samples = speech(9_700, rate);
        samples.extend(silence(700, rate));
        samples.extend(speech(5_000, rate));
        decode::write_wav_i16(&input, &samples, rate, 1).expect("write input");
        let output = split_audio_file(&input, &dir.join("parts"), 10_000)
            .expect("split")
            .expect("over the cap");
        assert_eq!(output.parts.len(), 2);
        assert_eq!(output.plan.parts[0].info.label(), "Part 1 of 2");
        assert_eq!(output.plan.parts[1].info.label(), "Part 2 of 2");
        assert!(output.parts[0]
            .path
            .ends_with("Interview — Part 1 of 2.wav"));
        assert!(output.parts[1]
            .path
            .ends_with("Interview — Part 2 of 2.wav"));
        let parts: Vec<PathBuf> = output.parts.iter().map(|part| part.path.clone()).collect();
        let joined = decode_concat(&parts);
        let original = decode::decode_file(&input).expect("input decode");
        assert_eq!(joined, original.samples);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_import_far_over_the_cap_splits_into_three_parts() {
        let rate = 16_000;
        let dir = scratch("import3");
        let input = dir.join("All hands.wav");
        let mut samples = Vec::new();
        // Pauses at 9.8–10.4 s and 19.6–20.6 s: the 10 s and 20 s caps land
        // inside them, so the import is cut at 10:00 and 20:00 exactly.
        samples.extend(speech(9_800, rate));
        samples.extend(silence(600, rate));
        samples.extend(speech(9_200, rate));
        samples.extend(silence(1_000, rate));
        samples.extend(speech(7_400, rate));
        decode::write_wav_i16(&input, &samples, rate, 1).expect("write input");
        let output = split_audio_file(&input, &dir.join("parts"), 10_000)
            .expect("split")
            .expect("over the cap");
        assert_eq!(output.parts.len(), 3, "{}", output.plan.summary());
        assert_eq!(output.plan.parts[2].info.label(), "Part 3 of 3");
        let parts: Vec<PathBuf> = output.parts.iter().map(|part| part.path.clone()).collect();
        let joined = decode_concat(&parts);
        let original = decode::decode_file(&input).expect("input decode");
        assert_eq!(joined.len(), original.samples.len());
        assert_eq!(joined, original.samples);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_import_at_the_cap_is_not_re_encoded() {
        let rate = 16_000;
        let dir = scratch("import-at-cap");
        let input = dir.join("exact.wav");
        let samples = speech(10_000, rate);
        decode::write_wav_i16(&input, &samples, rate, 1).expect("write input");
        let output = split_audio_file(&input, &dir.join("parts"), 10_000).expect("probe");
        assert!(output.is_none(), "exactly at the cap stays one file");
        assert!(
            !dir.join("parts").exists()
                || std::fs::read_dir(dir.join("parts"))
                    .unwrap()
                    .next()
                    .is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn part_input(
        index: u32,
        count: u32,
        duration_ms: i64,
        starts: &[(i64, i64)],
    ) -> SessionPartInput {
        SessionPartInput {
            transcript_id: Uuid::new_v4(),
            title: format!("Meeting — Part {index} of {count}"),
            part_index: index,
            part_count: count,
            duration_ms,
            segments: starts
                .iter()
                .enumerate()
                .map(|(idx, (start, end))| Segment {
                    idx: idx as u32,
                    start_ms: *start,
                    end_ms: *end,
                    text: format!("part {index} row {idx}"),
                })
                .collect(),
        }
    }

    #[test]
    fn absolute_times_accumulate_the_exact_part_durations() {
        // Part 1 is 45:00, part 2 is 5:00; part 3 starts at 50:00 exactly.
        let parts = vec![
            part_input(1, 3, 2_700_000, &[(0, 12_000), (2_688_000, 2_700_000)]),
            part_input(2, 3, 300_000, &[(0, 6_000)]),
            part_input(3, 3, 60_000, &[(500, 1_500)]),
        ];
        let timeline = session_timeline(&parts);
        assert_eq!(timeline.parts[0].offset_ms, 0);
        assert_eq!(timeline.parts[1].offset_ms, 2_700_000);
        assert_eq!(timeline.parts[2].offset_ms, 3_000_000);
        assert_eq!(timeline.total_ms, 3_060_000);
        // Segment times are part start + absolute offset, exactly.
        assert_eq!(timeline.segments[0].start_ms, 0);
        assert_eq!(timeline.segments[1].start_ms, 2_688_000);
        assert_eq!(timeline.segments[2].start_ms, 2_700_000);
        assert_eq!(timeline.segments[3].start_ms, 3_000_500);
        // The reindexed rows are contiguous across the session.
        assert_eq!(
            timeline.segments.iter().map(|s| s.idx).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(timeline.part_at(2_700_000).unwrap().part_index, 2);
        assert_eq!(timeline.part_at(3_000_500).unwrap().part_index, 3);
    }

    #[test]
    fn an_incomplete_session_refuses_whole_session_notes_by_name() {
        assert_eq!(session_incomplete_refusal(3, 3), None);
        let reason = session_incomplete_refusal(2, 3).expect("2 of 3 refuses");
        assert!(reason.starts_with("session_incomplete:"));
        assert!(reason.contains("2 of 3 parts transcribed"), "{reason}");
        assert_eq!(session_incomplete_line(2, 3), "2 of 3 parts transcribed");
    }

    #[test]
    fn the_collapsed_row_meta_is_parts_then_length() {
        assert_eq!(session_meta_label(3, 7_800_000), "3 parts · 2 h 10 m");
        assert_eq!(session_meta_label(2, 2_520_000), "2 parts · 42 m");
        assert_eq!(session_meta_label(1, 45_000), "1 part · 45 s");
        assert_eq!(session_meta_label(2, 3_600_000), "2 parts · 1 h");
    }
}
