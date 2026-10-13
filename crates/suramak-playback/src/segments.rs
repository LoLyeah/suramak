//! Position → active segment, and the `mm:ss` readout (M8) — pure, unit-tested.

use suramak_core::transcript::Segment;

/// Formats milliseconds as `mm:ss` (hours fold into the minutes field, like the
/// record view's clock): `0 → "00:00"`, `75_000 → "01:15"`, `3_600_000 → "60:00"`.
pub fn format_mmss(ms: u32) -> String {
    let total_secs = ms / 1_000;
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

/// Index into `segments` of the segment **containing** `position_ms`
/// (`start_ms <= position_ms < end_ms`), by binary search over the starts.
///
/// Returns `None` before the first segment, inside a gap between segments and
/// after the last one — those are the four boundary cases the tests pin down.
/// A degenerate segment (`end_ms <= start_ms`) contains exactly its start.
///
/// `segments` must be sorted by `start_ms` (the order the store returns).
pub fn segment_index_at(segments: &[Segment], position_ms: i64) -> Option<usize> {
    let after = segments.partition_point(|segment| segment.start_ms <= position_ms);
    let index = after.checked_sub(1)?;
    let segment = &segments[index];
    let contains = position_ms < segment.end_ms || segment.end_ms <= segment.start_ms;
    contains.then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(idx: u32, start_ms: i64, end_ms: i64) -> Segment {
        Segment {
            idx,
            start_ms,
            end_ms,
            text: format!("segment {idx}"),
        }
    }

    /// Two segments with a gap between them (2 000–2 400 ms).
    fn gapped() -> Vec<Segment> {
        vec![segment(0, 0, 2_000), segment(1, 2_400, 5_000)]
    }

    #[test]
    fn the_readout_is_mm_ss() {
        assert_eq!(format_mmss(0), "00:00");
        assert_eq!(format_mmss(999), "00:00");
        assert_eq!(format_mmss(1_000), "00:01");
        assert_eq!(format_mmss(59_999), "00:59");
        assert_eq!(format_mmss(60_000), "01:00");
        assert_eq!(format_mmss(754_000), "12:34");
        assert_eq!(format_mmss(3_600_000), "60:00");
    }

    #[test]
    fn a_position_inside_a_segment_selects_it() {
        assert_eq!(segment_index_at(&gapped(), 0), Some(0));
        assert_eq!(segment_index_at(&gapped(), 1_999), Some(0));
        assert_eq!(segment_index_at(&gapped(), 2_400), Some(1));
        assert_eq!(segment_index_at(&gapped(), 4_999), Some(1));
    }

    #[test]
    fn a_position_before_in_a_gap_or_after_has_no_active_segment() {
        let segments = gapped();
        // Before the first start.
        assert_eq!(segment_index_at(&segments, -1), None);
        // At/after the first end but before the next start: the gap.
        assert_eq!(segment_index_at(&segments, 2_000), None);
        assert_eq!(segment_index_at(&segments, 2_399), None);
        // At/after the last end.
        assert_eq!(segment_index_at(&segments, 5_000), None);
        assert_eq!(segment_index_at(&segments, 90_000), None);
    }

    #[test]
    fn contiguous_segments_hand_the_boundary_to_the_next_one() {
        let segments = vec![segment(0, 0, 1_000), segment(1, 1_000, 2_000)];
        assert_eq!(segment_index_at(&segments, 999), Some(0));
        assert_eq!(segment_index_at(&segments, 1_000), Some(1));
        assert_eq!(segment_index_at(&segments, 2_000), None);
    }

    #[test]
    fn a_zero_length_segment_contains_its_start() {
        let segments = vec![segment(0, 500, 500), segment(1, 900, 1_200)];
        assert_eq!(segment_index_at(&segments, 499), None);
        assert_eq!(segment_index_at(&segments, 500), Some(0));
        assert_eq!(segment_index_at(&segments, 900), Some(1));
    }

    #[test]
    fn an_empty_transcript_has_no_active_segment() {
        assert_eq!(segment_index_at(&[], 0), None);
    }

    #[test]
    fn the_index_is_found_by_the_search_not_a_linear_scan() {
        // 100 tidy segments, every start probed: the binary search must agree
        // with the obvious answer.
        let segments: Vec<Segment> = (0..100)
            .map(|i| segment(i, i as i64 * 1_000, i as i64 * 1_000 + 1_000))
            .collect();
        for (index, segment) in segments.iter().enumerate() {
            assert_eq!(segment_index_at(&segments, segment.start_ms), Some(index));
            assert_eq!(segment_index_at(&segments, segment.end_ms - 1), Some(index));
            // Contiguous segments: a boundary belongs to the next segment…
            let boundary = segment_index_at(&segments, segment.end_ms);
            if index + 1 < segments.len() {
                assert_eq!(boundary, Some(index + 1));
            } else {
                // …and past the last end nothing is active.
                assert_eq!(boundary, None);
            }
        }
    }
}
