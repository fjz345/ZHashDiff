use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexSide {
    First,
    Second,
}

/// Bytes past the end of the shorter input, present only on `longer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexTail {
    pub longer: HexSide,
    pub range: Range<usize>,
}

/// Offset-aligned comparison result. `ranges` are sorted, disjoint, non-adjacent and lie inside
/// the common length; the size-mismatch tail is kept separate in `tail`, even when a differing
/// range ends where it starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HexDiff {
    pub ranges: Vec<Range<usize>>,
    pub tail: Option<HexTail>,
}

/// Equal chunks are skipped with one slice compare (memcmp), so only chunks that differ are
/// walked byte by byte. The cancel flag is checked once per chunk.
const CHUNK: usize = 64 * 1024;

/// Compares byte N of `first` against byte N of `second`. `None` when cancelled.
pub fn hex_diff(first: &[u8], second: &[u8], cancel_flag: &AtomicBool) -> Option<HexDiff> {
    if cancel_flag.load(Ordering::Acquire) {
        return None;
    }

    let common = first.len().min(second.len());
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut chunk_start = 0;
    while chunk_start < common {
        if cancel_flag.load(Ordering::Acquire) {
            return None;
        }
        let chunk_end = (chunk_start + CHUNK).min(common);
        if first[chunk_start..chunk_end] != second[chunk_start..chunk_end] {
            for offset in chunk_start..chunk_end {
                if first[offset] != second[offset] {
                    match ranges.last_mut() {
                        Some(last) if last.end == offset => last.end = offset + 1,
                        _ => ranges.push(offset..offset + 1),
                    }
                }
            }
        }
        chunk_start = chunk_end;
    }

    let tail = if first.len() > common {
        Some(HexTail {
            longer: HexSide::First,
            range: common..first.len(),
        })
    } else if second.len() > common {
        Some(HexTail {
            longer: HexSide::Second,
            range: common..second.len(),
        })
    } else {
        None
    };

    Some(HexDiff { ranges, tail })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn diff(first: &[u8], second: &[u8]) -> HexDiff {
        hex_diff(first, second, &AtomicBool::new(false)).expect("not cancelled")
    }

    #[test]
    fn identical_inputs_have_no_ranges_and_no_tail() {
        assert_eq!(diff(b"abcdef", b"abcdef"), HexDiff::default());
        assert_eq!(diff(b"", b""), HexDiff::default());
    }

    #[test]
    fn single_differing_byte_is_one_range() {
        let d = diff(b"abcdef", b"abXdef");
        assert_eq!(d.ranges, vec![2..3]);
        assert_eq!(d.tail, None);
    }

    #[test]
    fn separate_differences_are_separate_ranges() {
        let d = diff(b"abcdefgh", b"Xbcd__gZ");
        assert_eq!(d.ranges, vec![0..1, 4..6, 7..8]);
    }

    #[test]
    fn adjacent_differing_bytes_merge_into_one_range() {
        let d = diff(b"abcdef", b"aXYZef");
        assert_eq!(d.ranges, vec![1..4]);
    }

    #[test]
    fn a_differing_run_across_a_chunk_boundary_is_one_range() {
        let first = vec![0u8; CHUNK * 2];
        let mut second = first.clone();
        second[CHUNK - 2..CHUNK + 2].fill(1);
        assert_eq!(diff(&first, &second).ranges, vec![CHUNK - 2..CHUNK + 2]);

        // A run ending exactly at a boundary and one starting exactly at it also merge.
        let mut second = first.clone();
        second[CHUNK - 1] = 1;
        second[CHUNK] = 1;
        assert_eq!(diff(&first, &second).ranges, vec![CHUNK - 1..CHUNK + 1]);
    }

    #[test]
    fn one_side_empty_is_all_tail() {
        let d = diff(b"", b"abc");
        assert!(d.ranges.is_empty());
        assert_eq!(
            d.tail,
            Some(HexTail {
                longer: HexSide::Second,
                range: 0..3,
            })
        );

        let d = diff(b"abc", b"");
        assert_eq!(
            d.tail,
            Some(HexTail {
                longer: HexSide::First,
                range: 0..3,
            })
        );
    }

    #[test]
    fn size_mismatch_tail_is_the_longer_sides_extra_bytes() {
        let d = diff(b"abcdef", b"abcd");
        assert!(d.ranges.is_empty());
        assert_eq!(
            d.tail,
            Some(HexTail {
                longer: HexSide::First,
                range: 4..6,
            })
        );
    }

    #[test]
    fn a_range_ending_at_the_common_length_stays_separate_from_the_tail() {
        let d = diff(b"abcX", b"abcdef");
        assert_eq!(d.ranges, vec![3..4]);
        assert_eq!(
            d.tail,
            Some(HexTail {
                longer: HexSide::Second,
                range: 4..6,
            })
        );
    }

    #[test]
    fn large_buffer_completes_quickly() {
        let len = 64 * 1024 * 1024;
        let first = vec![0xABu8; len];
        let mut second = first.clone();
        second[17] = 0;
        second[len / 2..len / 2 + 100].fill(0);
        second[len - 1] = 0;

        let start = Instant::now();
        let d = diff(&first, &second);
        // Generous, so a debug build on a slow machine still passes.
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(d.ranges, vec![17..18, len / 2..len / 2 + 100, len - 1..len]);
        assert_eq!(d.tail, None);
    }

    #[test]
    fn cancellation_returns_none() {
        let first = vec![0u8; 8 * CHUNK];
        let second = vec![1u8; 8 * CHUNK];
        assert_eq!(hex_diff(&first, &second, &AtomicBool::new(true)), None);
        assert_eq!(hex_diff(b"", b"", &AtomicBool::new(true)), None);
    }
}
