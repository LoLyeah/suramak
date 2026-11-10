//! Transactional split/merge of timed segments (M15.3) — the pure half.
//!
//! Everything here is deterministic and store-free: given the segment rows of
//! one transcript, [`split_rows`] and [`merge_rows`] return the exact row set a
//! successful transaction must persist, or a named [`EditError`]. The store
//! (`store::Store::split_segment` / `merge_segments`) runs those in one
//! transaction and hands back a [`SegmentSnapshot`]; the snapshot's `before`
//! rows are also what makes undo/redo exact instead of reconstructed.
//!
//! ## The rules this module encodes
//!
//! **Split text** — a split is a *word-boundary* partition:
//! - the caret must fall on a UTF-8 character boundary (`NotCharBoundary`);
//! - a caret at either end of the text has no two sides (`EmptySide`);
//! - the boundary is the whitespace run the caret sits in or immediately
//!   follows (so a caret on the space of "halo dunia" splits there, and a caret
//!   on a dash keeps the dash with the following half); a caret at the start of
//!   a word whose preceding run would leave an empty left half uses the run
//!   after it instead (so a caret inside the first word still splits after that
//!   word — the closest legal boundary, never a forced mid-token cut);
//! - no whitespace anywhere means the segment is one unbreakable token:
//!   `NoWordBoundary`, and nothing is written;
//! - both halves are trimmed; a half that trims to nothing is `EmptySide`.
//!
//! **Split time** — `[start, end)` is cut at `at_ms` with `start < at < end`.
//! The left keeps `[start, at)`, the right `[at, end)`: no overlap, no gap, no
//! negative or zero crossing, and `end > start` on both sides. A segment whose
//! own `end <= start` follows the subtitle exporter's zero-duration policy
//! ([`ZeroDuration`], M15.4): `Skip` refuses the split, `OneFrame` first widens
//! the span to one 25 fps frame (40 ms) so the cut has somewhere to land.
//!
//! **Merge** — two or more *contiguous* rows become one: text is the parts
//! trimmed and joined with a caller-chosen separator, the span is the union
//! (`min start`, `max end`), and ordering survives because the result takes the
//! first row's position and every later row shifts down by `count - 1`.
//!
//! **Provenance** — `source_text` (the inference output, M15-A's invariant) is
//! never dropped: a split gives both halves the parent's text (the parent's
//! inference covers both halves' audio), a merge concatenates the parts' texts
//! with a newline. The multiset of provenance is preserved in either direction —
//! no inference text is ever lost, only duplicated across a split or joined
//! across a merge. `derived` records how the row's `text` came to be.

use std::collections::VecDeque;
use std::fmt;

use crate::export::{ZeroDuration, ASS_ONE_FRAME_MS};

/// The largest number of rows one merge may join. A bound, not a rule of the
/// format: it keeps a single transaction's in-memory snapshot small enough to
/// sit in the undo log without measuring anything.
pub const MAX_MERGE_ROWS: u32 = 256;

/// How deep the in-memory undo log goes, per open transcript. Documented in
/// the report: older entries fall off the back, and closing the editor drops
/// the whole log (a fresh app session never offers undo for edits it did not
/// witness).
pub const UNDO_DEPTH: usize = 20;

/// One segment row exactly as the store holds it (`idx`, span, current text,
/// the first stored/inference text, and the derived marker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRow {
    pub idx: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// The first text ever stored for the row (the inference output). `None`
    /// only for a row that never had one (not produced by this build).
    pub source_text: Option<String>,
    /// `""` for an untouched row, else how `text` was derived (`split`,
    /// `merge`, `manual`, …). Free-form so a later slice can add kinds without
    /// a migration.
    pub derived: String,
}

impl SegmentRow {
    pub fn new(idx: u32, start_ms: i64, end_ms: i64, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            idx,
            start_ms,
            end_ms,
            source_text: Some(text.clone()),
            derived: String::new(),
            text,
        }
    }
}

/// Why a split/merge was refused. Every variant is a distinct, documented
/// boundary — the UI shows the message, and no partial write ever happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// No segment with that index in this transcript.
    NotFound(u32),
    /// The caret is not on a UTF-8 character boundary.
    NotCharBoundary,
    /// The caret sits at the start or end of the text: one side would be empty.
    EmptySide,
    /// The text has no whitespace at all, so there is no word boundary to cut.
    NoWordBoundary,
    /// The requested cut time is not strictly inside the segment's span.
    CrossingBoundary { start_ms: i64, end_ms: i64 },
    /// A zero/negative-duration segment under the `Skip` policy.
    ZeroDuration { start_ms: i64, end_ms: i64 },
    /// A merge needs at least two rows.
    NothingToMerge,
    /// The merge range exceeds [`MAX_MERGE_ROWS`].
    MergeTooLarge { count: u32 },
    /// The merge range is not a contiguous run starting at `first`.
    NotContiguous,
    /// The merge range runs past the end of the transcript.
    OutOfRange,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::NotFound(idx) => write!(f, "segment {idx} does not exist"),
            EditError::NotCharBoundary => write!(f, "the caret is not on a character boundary"),
            EditError::EmptySide => write!(f, "the split would leave an empty half"),
            EditError::NoWordBoundary => write!(f, "this segment has no word boundary to split at"),
            EditError::CrossingBoundary { start_ms, end_ms } => write!(
                f,
                "the cut must fall strictly inside {start_ms}..{end_ms} ms"
            ),
            EditError::ZeroDuration { start_ms, end_ms } => write!(
                f,
                "segment {start_ms}..{end_ms} ms has no duration to split"
            ),
            EditError::NothingToMerge => write!(f, "a merge needs at least two segments"),
            EditError::MergeTooLarge { count } => write!(
                f,
                "{count} segments is more than one merge may join (max {MAX_MERGE_ROWS})"
            ),
            EditError::NotContiguous => write!(f, "only adjacent segments can be merged"),
            EditError::OutOfRange => write!(f, "the merge range runs past the transcript"),
        }
    }
}

impl std::error::Error for EditError {}

/// The half-open span `[start, end)` a split produces on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitSpan {
    pub left_start: i64,
    pub left_end: i64,
    pub right_start: i64,
    pub right_end: i64,
}

/// Splits `[start, end)` at `at_ms` (default: the midpoint), honouring the
/// zero-duration policy. Pure and total: either a coherent two-cue span or an
/// error, never a half-applied one.
pub fn split_span(
    start: i64,
    end: i64,
    at_ms: Option<i64>,
    policy: ZeroDuration,
) -> Result<SplitSpan, EditError> {
    let end = if end <= start {
        match policy {
            ZeroDuration::Skip => {
                return Err(EditError::ZeroDuration {
                    start_ms: start,
                    end_ms: end,
                })
            }
            ZeroDuration::OneFrame => start + ASS_ONE_FRAME_MS,
        }
    } else {
        end
    };
    if start < 0 {
        return Err(EditError::CrossingBoundary {
            start_ms: start,
            end_ms: end,
        });
    }
    let at = at_ms.unwrap_or_else(|| start + (end - start) / 2);
    if at <= start || at >= end {
        return Err(EditError::CrossingBoundary {
            start_ms: start,
            end_ms: end,
        });
    }
    Ok(SplitSpan {
        left_start: start,
        left_end: at,
        right_start: at,
        right_end: end,
    })
}

/// The byte index the split cuts at for `caret`, plus the two trimmed halves.
///
/// See the module docs for the boundary rule. The returned index is the start
/// of the right half *before* trimming; callers that only need the text use the
/// halves.
pub fn split_text(text: &str, caret: usize) -> Result<(String, String), EditError> {
    if caret == 0 || caret >= text.len() {
        return Err(EditError::EmptySide);
    }
    if !text.is_char_boundary(caret) {
        return Err(EditError::NotCharBoundary);
    }
    // The whitespace run the caret sits in or just after is the boundary; a
    // caret in the middle of a word falls back to the run *after* it when the
    // run before would leave an empty left half.
    let leading = text[caret..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace);
    let preferred = if leading {
        Some(caret)
    } else {
        text[..caret]
            .char_indices()
            .rfind(|(_, ch)| ch.is_whitespace())
            .map(|(offset, _)| offset)
    };
    let boundary = match preferred {
        Some(boundary) if !text[..boundary].trim_end().is_empty() => boundary,
        _ => text[caret..]
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(offset, _)| caret + offset)
            .ok_or(EditError::NoWordBoundary)?,
    };
    let left = text[..boundary].trim_end();
    let right = text[boundary..].trim_start();
    if left.is_empty() || right.is_empty() {
        return Err(EditError::EmptySide);
    }
    Ok((left.to_string(), right.to_string()))
}

/// Splits one row in two. `caret` is a byte index into the row's **current**
/// text (what the editor shows); `at_ms` defaults to the span midpoint.
///
/// Both resulting rows carry the parent's `source_text` (see the module docs)
/// and `derived = "split"`.
pub fn split_rows(
    rows: &[SegmentRow],
    idx: u32,
    caret: usize,
    at_ms: Option<i64>,
    policy: ZeroDuration,
) -> Result<Vec<SegmentRow>, EditError> {
    let position = rows
        .iter()
        .position(|row| row.idx == idx)
        .ok_or(EditError::NotFound(idx))?;
    let parent = &rows[position];
    let (left_text, right_text) = split_text(&parent.text, caret)?;
    let span = split_span(parent.start_ms, parent.end_ms, at_ms, policy)?;

    let mut out = Vec::with_capacity(rows.len() + 1);
    out.extend_from_slice(&rows[..position]);
    out.push(SegmentRow {
        idx,
        start_ms: span.left_start,
        end_ms: span.left_end,
        text: left_text,
        source_text: parent.source_text.clone(),
        derived: "split".to_string(),
    });
    out.push(SegmentRow {
        idx: idx + 1,
        start_ms: span.right_start,
        end_ms: span.right_end,
        text: right_text,
        source_text: parent.source_text.clone(),
        derived: "split".to_string(),
    });
    out.extend_from_slice(&rows[position + 1..]);
    renumber(out).ok_or(EditError::OutOfRange)
}

/// Merges `count` contiguous rows starting at `first`.
///
/// The merged row's span is the union of the parts', its text is the parts
/// trimmed and joined with `separator`, its `source_text` is the parts'
/// provenance joined with a newline (`None` only when no part had any), and its
/// `derived` is `"merge"`.
pub fn merge_rows(
    rows: &[SegmentRow],
    first: u32,
    count: u32,
    separator: &str,
) -> Result<Vec<SegmentRow>, EditError> {
    if count < 2 {
        return Err(EditError::NothingToMerge);
    }
    if count > MAX_MERGE_ROWS {
        return Err(EditError::MergeTooLarge { count });
    }
    let start = rows
        .iter()
        .position(|row| row.idx == first)
        .ok_or(EditError::NotFound(first))?;
    let end = start + count as usize;
    if end > rows.len() {
        return Err(EditError::OutOfRange);
    }
    // Contiguity is by position after `first`, which is how the store keeps
    // idx dense: any hole means a caller sent the wrong range.
    for (offset, row) in rows[start..end].iter().enumerate() {
        if row.idx != first + offset as u32 {
            return Err(EditError::NotContiguous);
        }
    }

    let parts = &rows[start..end];
    let text = parts
        .iter()
        .map(|row| row.text.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(separator);
    if text.is_empty() {
        return Err(EditError::EmptySide);
    }
    let start_ms = parts.iter().map(|row| row.start_ms).min().unwrap_or(0);
    let end_ms = parts.iter().map(|row| row.end_ms).max().unwrap_or(0);
    let provenance: Vec<&str> = parts
        .iter()
        .filter_map(|row| row.source_text.as_deref())
        .collect();
    let source_text = if provenance.is_empty() {
        None
    } else {
        Some(provenance.join("\n"))
    };

    let mut out = Vec::with_capacity(rows.len() - count as usize + 1);
    out.extend_from_slice(&rows[..start]);
    out.push(SegmentRow {
        idx: first,
        start_ms,
        end_ms,
        text,
        source_text,
        derived: "merge".to_string(),
    });
    out.extend_from_slice(&rows[end..]);
    renumber(out).ok_or(EditError::OutOfRange)
}

/// Rewrites `idx` as `0..len` in place. Ordering is the transcript's, never a
/// sort: a caller that produced rows out of order gets them renumbered in the
/// order it passed, and the store's `ORDER BY idx` reads back exactly that.
fn renumber(mut rows: Vec<SegmentRow>) -> Option<Vec<SegmentRow>> {
    if rows.len() > u32::MAX as usize {
        return None;
    }
    for (idx, row) in rows.iter_mut().enumerate() {
        row.idx = idx as u32;
    }
    Some(rows)
}

/// One applied transaction: the rows before and after. Undo writes `before`
/// back, redo writes `after` — exact, because both are the real rows, not a
/// recomputation of the inverse operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSnapshot {
    pub transcript_id: uuid::Uuid,
    pub before: Vec<SegmentRow>,
    pub after: Vec<SegmentRow>,
}

impl SegmentSnapshot {
    /// How many rows the transaction added (`+n`) or removed (`-n`).
    pub fn delta(&self) -> i64 {
        self.after.len() as i64 - self.before.len() as i64
    }
}

/// How an entry entered the log (for the status line and the report).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Split,
    Merge,
}

impl fmt::Display for EditKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditKind::Split => write!(f, "split"),
            EditKind::Merge => write!(f, "merge"),
        }
    }
}

/// The bounded, in-memory undo/redo log for one open transcript.
///
/// - depth: [`UNDO_DEPTH`] entries; the oldest falls off the back (a bounded
///   log is enough for this feature — it is a session convenience, not history).
/// - a push clears the redo stack (the classic linear rule);
/// - [`SegmentHistory::clear`] runs when the editor closes or switches
///   transcripts, so a fresh app session offers no undo it did not witness.
#[derive(Debug, Clone, Default)]
pub struct SegmentHistory {
    undo: VecDeque<(EditKind, SegmentSnapshot)>,
    redo: Vec<(EditKind, SegmentSnapshot)>,
}

impl SegmentHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an applied transaction and drops any redo branch.
    pub fn push(&mut self, kind: EditKind, snapshot: SegmentSnapshot) {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.pop_front();
        }
        self.undo.push_back((kind, snapshot));
        self.redo.clear();
    }

    /// The transaction an undo would revert (`None` when the log is empty).
    pub fn next_undo(&self) -> Option<(EditKind, SegmentSnapshot)> {
        self.undo.back().cloned()
    }

    /// The transaction a redo would re-apply.
    pub fn next_redo(&self) -> Option<(EditKind, SegmentSnapshot)> {
        self.redo.last().cloned()
    }

    /// Pops the newest undo entry onto the redo stack.
    pub fn undo(&mut self) -> Option<(EditKind, SegmentSnapshot)> {
        let entry = self.undo.pop_back()?;
        self.redo.push(entry.clone());
        Some(entry)
    }

    /// Pops the newest redo entry back onto the undo stack.
    pub fn redo(&mut self) -> Option<(EditKind, SegmentSnapshot)> {
        let entry = self.redo.pop()?;
        if self.undo.len() == UNDO_DEPTH {
            self.undo.pop_front();
        }
        self.undo.push_back(entry.clone());
        Some(entry)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn depth(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<SegmentRow> {
        vec![
            SegmentRow::new(0, 0, 1_000, "halo dunia"),
            SegmentRow::new(1, 1_000, 2_000, "selamat pagi semua"),
            SegmentRow::new(2, 2_000, 3_000, "terima kasih"),
        ]
    }

    // ---- text partitioning ---------------------------------------------------

    #[test]
    fn a_caret_inside_a_word_snaps_to_the_nearest_word_boundary() {
        // "halo dunia": caret inside "dunia" (byte 7) → the boundary *before*
        // that word, because there is no whitespace at/after the caret before
        // the end.
        let (left, right) = split_text("halo dunia", 7).expect("split");
        assert_eq!(left, "halo");
        assert_eq!(right, "dunia");

        // A caret inside "halo" (2) → the whitespace after it: "ha" is not a
        // boundary, so the split lands after "halo".
        let (left, right) = split_text("halo dunia", 2).expect("split");
        assert_eq!(left, "halo");
        assert_eq!(right, "dunia");

        // A caret exactly on the space (4) → the run at/after it is the same
        // space; both halves are trimmed.
        let (left, right) = split_text("halo dunia", 4).expect("split");
        assert_eq!((left.as_str(), right.as_str()), ("halo", "dunia"));
    }

    #[test]
    fn multi_byte_text_splits_on_character_boundaries_only() {
        let text = "selamat pagi — semua";
        let caret = text.find('—').unwrap();
        let (left, right) = split_text(text, caret).expect("split at the em dash");
        assert_eq!(left, "selamat pagi");
        assert_eq!(right, "— semua");

        // Mid-character: a caret one byte into "é" is not a boundary.
        let text = "café au lait";
        let inside = text.find('é').unwrap() + 1;
        assert_eq!(split_text(text, inside), Err(EditError::NotCharBoundary));
    }

    #[test]
    fn a_segment_with_one_unbreakable_token_cannot_be_split() {
        assert_eq!(
            split_text("pertanggungjawaban", 5),
            Err(EditError::NoWordBoundary)
        );
        // The ends are empty halves, before any boundary search.
        assert_eq!(split_text("halo dunia", 0), Err(EditError::EmptySide));
        assert_eq!(split_text("halo dunia", 10), Err(EditError::EmptySide));
        // Interior whitespace only ("a   b") still yields two non-empty sides.
        let (left, right) = split_text("a   b", 2).expect("split");
        assert_eq!((left.as_str(), right.as_str()), ("a", "b"));
    }

    // ---- time boundaries -----------------------------------------------------

    #[test]
    fn a_split_span_is_half_open_and_never_crosses() {
        let span = split_span(0, 1_000, Some(400), ZeroDuration::OneFrame).expect("span");
        assert_eq!(
            span,
            SplitSpan {
                left_start: 0,
                left_end: 400,
                right_start: 400,
                right_end: 1_000
            }
        );
        assert!(span.left_end > span.left_start);
        assert!(span.right_end > span.right_start);
        assert_eq!(span.left_end, span.right_start, "no gap, no overlap");
    }

    #[test]
    fn cut_times_on_or_outside_the_edges_are_refused() {
        for at in [0, 1_000, -5, 2_000] {
            assert_eq!(
                split_span(0, 1_000, Some(at), ZeroDuration::OneFrame),
                Err(EditError::CrossingBoundary {
                    start_ms: 0,
                    end_ms: 1_000
                }),
                "at={at}"
            );
        }
        // The default is the midpoint, which is always strictly inside.
        let span = split_span(0, 1_000, None, ZeroDuration::OneFrame).unwrap();
        assert_eq!((span.left_end, span.right_start), (500, 500));
        // An odd span still lands strictly inside.
        let span = split_span(0, 999, None, ZeroDuration::OneFrame).unwrap();
        assert!(span.left_end > 0 && span.right_end > span.right_start);
        assert_eq!(span.left_end + (999 - span.left_end), 999);
    }

    #[test]
    fn a_zero_duration_segment_follows_the_exporters_policy() {
        assert_eq!(
            split_span(1_000, 1_000, None, ZeroDuration::Skip),
            Err(EditError::ZeroDuration {
                start_ms: 1_000,
                end_ms: 1_000
            })
        );
        // OneFrame widens to 40 ms so the midpoint cut has two positive sides.
        let span = split_span(1_000, 1_000, None, ZeroDuration::OneFrame).expect("one frame");
        assert_eq!(span.right_end, 1_040);
        assert!(span.left_end > span.left_start && span.right_end > span.right_start);
        // …and a negative span is treated the same way, never a negative cue.
        let span = split_span(500, 400, None, ZeroDuration::OneFrame).expect("one frame");
        assert_eq!((span.left_start, span.right_end), (500, 540));
    }

    // ---- row transformations -------------------------------------------------

    #[test]
    fn splitting_a_row_inserts_one_row_and_renumbers_the_tail() {
        let out = split_rows(&rows(), 1, 7, Some(1_500), ZeroDuration::OneFrame).expect("split");
        assert_eq!(out.len(), 4);
        let texts: Vec<&str> = out.iter().map(|row| row.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["halo dunia", "selamat", "pagi semua", "terima kasih"]
        );
        assert_eq!(
            out.iter().map(|row| row.idx).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        // The split halves tile the parent's span exactly.
        assert_eq!((out[1].start_ms, out[1].end_ms), (1_000, 1_500));
        assert_eq!((out[2].start_ms, out[2].end_ms), (1_500, 2_000));
        assert_eq!(out[1].derived, "split");
        assert_eq!(out[2].derived, "split");
        // Unrelated rows are untouched, byte for byte.
        assert_eq!(out[0], rows()[0]);
        assert_eq!(out[3].text, "terima kasih");
        assert_eq!(out[3].start_ms, 2_000);
    }

    #[test]
    fn a_split_duplicates_the_parents_inference_text_on_both_halves() {
        let out = split_rows(&rows(), 1, 7, Some(1_500), ZeroDuration::OneFrame).unwrap();
        assert_eq!(out[1].source_text.as_deref(), Some("selamat pagi semua"));
        assert_eq!(out[2].source_text.as_deref(), Some("selamat pagi semua"));
    }

    #[test]
    fn merging_joins_contiguous_rows_with_the_chosen_separator() {
        for (separator, expected) in [
            (" ", "selamat pagi semua terima kasih"),
            ("\n", "selamat pagi semua\nterima kasih"),
            (" · ", "selamat pagi semua · terima kasih"),
        ] {
            let out = merge_rows(&rows(), 1, 2, separator).expect("merge");
            assert_eq!(out.len(), 2);
            assert_eq!(out[1].text, expected);
            assert_eq!((out[1].start_ms, out[1].end_ms), (1_000, 3_000));
            assert_eq!(out[1].derived, "merge");
            assert_eq!(out.iter().map(|r| r.idx).collect::<Vec<_>>(), vec![0, 1]);
            assert_eq!(out[0], rows()[0], "the untouched row is byte-identical");
        }
        // Merging all rows leaves one.
        let out = merge_rows(&rows(), 0, 3, " ").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].idx, 0);
        assert_eq!(out[0].text, "halo dunia selamat pagi semua terima kasih");
    }

    #[test]
    fn a_merge_concatenates_provenance_instead_of_dropping_it() {
        let out = merge_rows(&rows(), 1, 2, " ").unwrap();
        assert_eq!(
            out[1].source_text.as_deref(),
            Some("selamat pagi semua\nterima kasih")
        );
        // A part with no provenance contributes nothing but does not erase it.
        let mut rows = rows();
        rows[2].source_text = None;
        let out = merge_rows(&rows, 1, 2, " ").unwrap();
        assert_eq!(out[1].source_text.as_deref(), Some("selamat pagi semua"));
        rows[1].source_text = None;
        let out = merge_rows(&rows, 1, 2, " ").unwrap();
        assert_eq!(out[1].source_text, None);
    }

    #[test]
    fn merge_refuses_ranges_that_are_not_contiguous_or_in_range() {
        assert_eq!(
            merge_rows(&rows(), 1, 1, " "),
            Err(EditError::NothingToMerge)
        );
        assert_eq!(merge_rows(&rows(), 1, 9, " "), Err(EditError::OutOfRange));
        assert_eq!(merge_rows(&rows(), 9, 2, " "), Err(EditError::NotFound(9)));
        // A hole in the numbering (a caller's mistake) is refused, not guessed.
        let mut holed = rows();
        holed[1].idx = 7;
        assert_eq!(merge_rows(&holed, 7, 2, " "), Err(EditError::NotContiguous));
    }

    #[test]
    fn structural_edits_round_trip_through_their_snapshots() {
        let before = rows();
        let after = split_rows(&before, 0, 5, None, ZeroDuration::OneFrame).unwrap();
        let snapshot = SegmentSnapshot {
            transcript_id: uuid::Uuid::nil(),
            before: before.clone(),
            after: after.clone(),
        };
        assert_eq!(snapshot.delta(), 1);
        // Undo is the stored `before`, not a recomputation: byte-exact.
        assert_eq!(snapshot.before, before);
        assert_eq!(snapshot.after, after);
    }

    // ---- the bounded history -------------------------------------------------

    fn snapshot(n: u32) -> SegmentSnapshot {
        SegmentSnapshot {
            transcript_id: uuid::Uuid::nil(),
            before: vec![SegmentRow::new(0, 0, 100, format!("before {n}"))],
            after: vec![
                SegmentRow::new(0, 0, 50, format!("after {n}")),
                SegmentRow::new(1, 50, 100, "tail"),
            ],
        }
    }

    #[test]
    fn the_history_is_bounded_and_clears_redo_on_a_new_edit() {
        let mut history = SegmentHistory::new();
        for n in 0..(UNDO_DEPTH as u32 + 5) {
            history.push(EditKind::Split, snapshot(n));
        }
        assert_eq!(history.depth(), UNDO_DEPTH, "the log is bounded");
        // The oldest entries fell off: the newest undo is the last push.
        let (kind, newest) = history.next_undo().expect("undo entry");
        assert_eq!(kind, EditKind::Split);
        assert_eq!(
            newest.before[0].text,
            format!("before {}", UNDO_DEPTH as u32 + 4)
        );

        // Undo then a fresh edit: the redo branch is dropped.
        assert!(history.undo().is_some());
        assert!(history.can_redo());
        history.push(EditKind::Merge, snapshot(99));
        assert!(!history.can_redo(), "a new edit clears the redo branch");
    }

    #[test]
    fn undo_and_redo_walk_the_same_entries_in_order() {
        let mut history = SegmentHistory::new();
        history.push(EditKind::Split, snapshot(1));
        history.push(EditKind::Merge, snapshot(2));

        let (kind, first) = history.undo().expect("undo 1");
        assert_eq!(kind, EditKind::Merge);
        assert_eq!(first.before[0].text, "before 2");
        let (kind, second) = history.undo().expect("undo 2");
        assert_eq!(kind, EditKind::Split);
        assert_eq!(second.before[0].text, "before 1");
        assert!(!history.can_undo());
        assert_eq!(history.undo(), None, "an empty log is a no-op");

        // Redo replays newest-first-back: split(1) then merge(2).
        assert_eq!(history.redo().expect("redo 1").0, EditKind::Split);
        assert_eq!(history.redo().expect("redo 2").0, EditKind::Merge);
        assert_eq!(history.redo(), None);
        assert!(history.can_undo() && !history.can_redo());

        // Closing the editor drops everything.
        history.clear();
        assert!(!history.can_undo() && !history.can_redo());
        assert_eq!(history.depth(), 0);
    }
}
