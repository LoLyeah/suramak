//! M25B — long sessions, honestly capped and split at pauses.
//!
//! The M25A gate measured the in-memory single-pass path at **859 MB at 60
//! minutes** and **722 MB interpolated at 45 minutes**. Chunked decoding of one
//! file was measured and rejected on quality, so a long session is handled the
//! only honest way: **cap the part, and split at a real pause**.
//!
//! This module is the one place that owns:
//!
//! * the platform cap ([`SESSION_CAP_MS`]) and its named refusal
//!   ([`SessionCapRefusal`], reason `session_too_long`) carrying the measured
//!   budget;
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

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::decode;
use crate::resample;
use crate::transcript::Segment;
use crate::SuramakError;

/// Session cap on a 4 GB Android phone: 45 minutes (722 MB measured budget).
pub const SESSION_CAP_ANDROID_MS: i64 = 45 * 60 * 1000;
/// Session cap on desktop: 60 minutes (859 MB measured budget).
pub const SESSION_CAP_DESKTOP_MS: i64 = 60 * 60 * 1000;
/// The measured tiny-model peak at the Android cap (M25A, interpolated).
pub const SESSION_PEAK_ANDROID_MB: u64 = 722;
/// The measured tiny-model peak at the desktop cap (M25A).
pub const SESSION_PEAK_DESKTOP_MB: u64 = 859;

/// The cap this build enforces. Android is the phone target; every other host
/// (desktop, CLI, tests) uses the 60-minute budget.
#[cfg(target_os = "android")]
pub const SESSION_CAP_MS: i64 = SESSION_CAP_ANDROID_MS;
#[cfg(not(target_os = "android"))]
pub const SESSION_CAP_MS: i64 = SESSION_CAP_DESKTOP_MS;

/// A real pause of at least this long is a safe cut (M25B's rule).
pub const SPLIT_SILENCE_MS: i64 = 600;
/// How long the recorder may run past the cap waiting for a pause.
pub const SPLIT_GRACE_MS: i64 = 120_000;
/// Silence analysis frame (RMS window).
pub const ANALYSIS_FRAME_MS: i64 = 10;
/// A frame is silent when its RMS is at or below this (≈ −40 dBFS).
pub const SILENCE_RMS: f32 = 0.01;

/// The platform a cap is being resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPlatform {
    Android,
    Desktop,
}

impl SessionPlatform {
    /// The host this build is running on.
    pub const fn host() -> Self {
        if cfg!(target_os = "android") {
            SessionPlatform::Android
        } else {
            SessionPlatform::Desktop
        }
    }
}

/// The cap for `platform`, in milliseconds.
pub const fn session_cap_ms_for(platform: SessionPlatform) -> i64 {
    match platform {
        SessionPlatform::Android => SESSION_CAP_ANDROID_MS,
        SessionPlatform::Desktop => SESSION_CAP_DESKTOP_MS,
    }
}

/// The measured peak budget for `platform`, in MB (the number the refusal
/// carries; measured in `docs/reports/M25a-long-recording-gate.md`).
pub const fn session_peak_mb_for(platform: SessionPlatform) -> u64 {
    match platform {
        SessionPlatform::Android => SESSION_PEAK_ANDROID_MB,
        SessionPlatform::Desktop => SESSION_PEAK_DESKTOP_MB,
    }
}

/// The cap this host enforces.
pub const fn session_cap_ms() -> i64 {
    SESSION_CAP_MS
}

/// The measured peak this host's cap corresponds to.
pub const fn session_peak_mb() -> u64 {
    session_peak_mb_for(SessionPlatform::host())
}

/// `45:00` / `60:00` — the cap's own clock. Minutes never fold into hours
/// here: a 60-minute cap reads `60:00`, not `01:00:00`.
pub fn clock(ms: i64) -> String {
    let total = (ms.max(0) + 500) / 1000;
    format!("{}:{:02}", total / 60, total % 60)
}

/// A refused session: `session_too_long` plus the measured budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCapRefusal {
    pub audio_ms: i64,
    pub cap_ms: i64,
    pub peak_mb: u64,
    pub platform: SessionPlatform,
}

impl SessionCapRefusal {
    /// The stable machine-readable code the queue and the UI key on.
    pub const CODE: &'static str = "session_too_long";

    /// The named reason the job card shows. It carries the duration, the cap
    /// and the measured budget — never a generic error.
    pub fn reason(&self) -> String {
        let where_ = match self.platform {
            SessionPlatform::Android => "on a 4 GB phone",
            SessionPlatform::Desktop => "on this machine",
        };
        format!(
            "{CODE}: this session is {}, over the {} cap; {} minutes measures {peak} MB peak \
             {where_} — split it into shorter parts",
            clock(self.audio_ms),
            clock(self.cap_ms),
            self.cap_ms / 60_000,
            CODE = Self::CODE,
            peak = self.peak_mb,
            where_ = where_,
        )
    }
}

impl std::fmt::Display for SessionCapRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason())
    }
}

/// `Some` when `audio_ms` is over the platform cap. Exactly at the cap is
/// accepted (a 45:00.000 file is a legal 45-minute session); one millisecond
/// over is refused.
pub fn cap_refusal_for(audio_ms: i64, platform: SessionPlatform) -> Option<SessionCapRefusal> {
    let cap_ms = session_cap_ms_for(platform);
    if audio_ms > cap_ms {
        Some(SessionCapRefusal {
            audio_ms,
            cap_ms,
            peak_mb: session_peak_mb_for(platform),
            platform,
        })
    } else {
        None
    }
}

/// [`cap_refusal_for`] against this host's cap.
pub fn cap_refusal(audio_ms: i64) -> Option<SessionCapRefusal> {
    cap_refusal_for(audio_ms, SessionPlatform::host())
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

    #[test]
    fn the_cap_is_45_minutes_on_android_and_60_on_desktop() {
        assert_eq!(session_cap_ms_for(SessionPlatform::Android), 2_700_000);
        assert_eq!(session_cap_ms_for(SessionPlatform::Desktop), 3_600_000);
        assert_eq!(session_cap_ms_for(SessionPlatform::Android), 45 * 60 * 1000);
        assert_eq!(session_cap_ms_for(SessionPlatform::Desktop), 60 * 60 * 1000);
        assert_eq!(session_peak_mb_for(SessionPlatform::Android), 722);
        assert_eq!(session_peak_mb_for(SessionPlatform::Desktop), 859);
        // The host build reads the one constant the app uses.
        assert_eq!(session_cap_ms(), SESSION_CAP_MS);
        assert_eq!(
            SessionPlatform::host(),
            if cfg!(target_os = "android") {
                SessionPlatform::Android
            } else {
                SessionPlatform::Desktop
            }
        );
    }

    #[test]
    fn exactly_at_the_cap_is_accepted_and_one_millisecond_over_is_refused() {
        for (platform, cap) in [
            (SessionPlatform::Android, SESSION_CAP_ANDROID_MS),
            (SessionPlatform::Desktop, SESSION_CAP_DESKTOP_MS),
        ] {
            assert!(cap_refusal_for(cap, platform).is_none());
            let refused = cap_refusal_for(cap + 1, platform).expect("1 ms over is refused");
            assert_eq!(refused.audio_ms, cap + 1);
            assert_eq!(refused.cap_ms, cap);
            assert!(refused.reason().starts_with("session_too_long:"));
            assert!(refused.reason().contains("split it into shorter parts"));
            assert!(refused
                .reason()
                .contains(if platform == SessionPlatform::Android {
                    "722 MB"
                } else {
                    "859 MB"
                }));
        }
    }

    #[test]
    fn the_refusal_carries_the_measured_budget_not_a_generic_error() {
        let refused = cap_refusal_for(SESSION_CAP_ANDROID_MS + 62_000, SessionPlatform::Android)
            .expect("over");
        let reason = refused.reason();
        assert!(reason.contains("45:00"), "{reason}");
        assert!(reason.contains("722 MB"), "{reason}");
        assert!(reason.contains("4 GB phone"), "{reason}");
        assert!(reason.contains("46:02"), "{reason}");
    }

    #[test]
    fn a_short_session_is_one_part() {
        let samples = speech(1_000, 16_000);
        let plan = plan_split(&samples, 16_000, SESSION_CAP_DESKTOP_MS);
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
}
