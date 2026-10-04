use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

/// A straight (not premultiplied) RGBA8 image, row-major.
#[derive(Debug, Clone, Copy)]
pub struct RgbaBuffer<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// The smallest rectangle holding every changed pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelBounds {
    pub x: Range<u32>,
    pub y: Range<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PixelStats {
    pub changed: u64,
    /// Pixels on the union canvas.
    pub total: u64,
    pub bounds: Option<PixelBounds>,
}

impl PixelStats {
    pub fn changed_percent(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.changed as f64 * 100.0 / self.total as f64
    }
}

/// Comparison on the union canvas (the larger width by the larger height), both images aligned
/// at its top-left corner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDiff {
    pub width: u32,
    pub height: u32,
    /// Row-major over the canvas: whether each pixel changed.
    pub mask: Vec<bool>,
    pub stats: PixelStats,
}

/// A pixel changed when any channel differs by more than `tolerance`. Pixels outside either image
/// count as changed. `None` when cancelled.
pub fn pixel_diff(
    first: RgbaBuffer,
    second: RgbaBuffer,
    tolerance: u8,
    cancel_flag: &AtomicBool,
) -> Option<PixelDiff> {
    for image in [&first, &second] {
        assert_eq!(
            image.rgba.len(),
            image.width as usize * image.height as usize * 4,
            "RGBA length doesn't match {} x {}",
            image.width,
            image.height
        );
    }
    if cancel_flag.load(Ordering::Acquire) {
        return None;
    }

    let (width, height) = (
        first.width.max(second.width) as usize,
        first.height.max(second.height) as usize,
    );
    let (overlap_width, overlap_height) = (
        first.width.min(second.width) as usize,
        first.height.min(second.height) as usize,
    );
    // Everything outside the overlap stays changed.
    let mut mask = vec![true; width * height];
    let mut changed = 0;
    let mut bounds: Option<PixelBounds> = None;
    for y in 0..height {
        if cancel_flag.load(Ordering::Acquire) {
            return None;
        }
        let row = &mut mask[y * width..(y + 1) * width];
        if y < overlap_height {
            let a = overlap_row(&first, y, overlap_width);
            let b = overlap_row(&second, y, overlap_width);
            // Equal rows (most of them, in a typical edit) skip the per-pixel walk.
            if a == b {
                row[..overlap_width].fill(false);
            } else {
                for (x, (a, b)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
                    row[x] = a.iter().zip(b).any(|(a, b)| a.abs_diff(*b) > tolerance);
                }
            }
        }

        let (Some(first_x), Some(last_x)) =
            (row.iter().position(|&c| c), row.iter().rposition(|&c| c))
        else {
            continue;
        };
        changed += row.iter().filter(|&&c| c).count() as u64;
        let (x, y) = (first_x as u32..last_x as u32 + 1, y as u32);
        bounds = Some(match bounds {
            None => PixelBounds { x, y: y..y + 1 },
            Some(b) => PixelBounds {
                x: b.x.start.min(x.start)..b.x.end.max(x.end),
                y: b.y.start..y + 1,
            },
        });
    }

    Some(PixelDiff {
        width: width as u32,
        height: height as u32,
        mask,
        stats: PixelStats {
            changed,
            total: (width * height) as u64,
            bounds,
        },
    })
}

fn overlap_row<'a>(image: &RgbaBuffer<'a>, y: usize, overlap_width: usize) -> &'a [u8] {
    let start = y * image.width as usize * 4;
    &image.rgba[start..start + overlap_width * 4]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(width: u32, height: u32, rgba: &[u8]) -> RgbaBuffer<'_> {
        RgbaBuffer {
            width,
            height,
            rgba,
        }
    }

    fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        pixel.repeat((width * height) as usize)
    }

    fn diff(first: RgbaBuffer, second: RgbaBuffer, tolerance: u8) -> PixelDiff {
        pixel_diff(first, second, tolerance, &AtomicBool::new(false)).expect("not cancelled")
    }

    fn changed_at(diff: &PixelDiff) -> Vec<(u32, u32)> {
        (0..diff.height)
            .flat_map(|y| (0..diff.width).map(move |x| (x, y)))
            .filter(|&(x, y)| diff.mask[(y * diff.width + x) as usize])
            .collect()
    }

    #[test]
    fn identical_images_have_nothing_changed() {
        let rgba = solid(3, 2, [10, 20, 30, 255]);
        let d = diff(buffer(3, 2, &rgba), buffer(3, 2, &rgba), 0);
        assert_eq!((d.width, d.height), (3, 2));
        assert_eq!(d.mask, vec![false; 6]);
        assert_eq!(
            d.stats,
            PixelStats {
                changed: 0,
                total: 6,
                bounds: None,
            }
        );
        assert_eq!(d.stats.changed_percent(), 0.0);
    }

    #[test]
    fn a_single_changed_pixel_is_the_only_one_masked() {
        let first = solid(4, 3, [0, 0, 0, 255]);
        let mut second = first.clone();
        // Pixel (2, 1).
        second[(4 + 2) * 4] = 1;
        let d = diff(buffer(4, 3, &first), buffer(4, 3, &second), 0);
        assert_eq!(changed_at(&d), vec![(2, 1)]);
        assert_eq!(d.stats.changed, 1);
        assert_eq!(d.stats.bounds, Some(PixelBounds { x: 2..3, y: 1..2 }));
    }

    #[test]
    fn a_difference_equal_to_the_tolerance_is_unchanged() {
        for channel in 0..4 {
            let first = [100, 100, 100, 100];
            let mut equal = first;
            equal[channel] += 7;
            let mut over = first;
            over[channel] += 8;
            let mut under = first;
            under[channel] -= 8;

            let d = diff(buffer(1, 1, &first), buffer(1, 1, &equal), 7);
            assert_eq!(d.stats.changed, 0, "channel {channel}");
            let d = diff(buffer(1, 1, &first), buffer(1, 1, &over), 7);
            assert_eq!(d.stats.changed, 1, "channel {channel}");
            // Absolute difference: lower on the second side counts the same.
            let d = diff(buffer(1, 1, &first), buffer(1, 1, &under), 7);
            assert_eq!(d.stats.changed, 1, "channel {channel}");
        }
    }

    #[test]
    fn the_largest_tolerance_accepts_any_difference_inside_the_overlap() {
        let black = solid(2, 2, [0, 0, 0, 0]);
        let white = solid(2, 2, [255; 4]);
        let d = diff(buffer(2, 2, &black), buffer(2, 2, &white), 255);
        assert_eq!(d.stats.changed, 0);
        let d = diff(buffer(2, 2, &black), buffer(2, 2, &white), 254);
        assert_eq!(d.stats.changed, 4);
    }

    #[test]
    fn pixels_outside_the_overlap_count_as_changed() {
        // 3x1 against 1x2: the canvas is 3x2 and only (0, 0) is in both images. (1, 1) and (2, 1)
        // are in neither and count as changed too.
        let first = solid(3, 1, [5, 5, 5, 255]);
        let second = solid(1, 2, [5, 5, 5, 255]);
        let d = diff(buffer(3, 1, &first), buffer(1, 2, &second), 255);
        assert_eq!((d.width, d.height), (3, 2));
        assert_eq!(changed_at(&d), vec![(1, 0), (2, 0), (0, 1), (1, 1), (2, 1)]);
        assert_eq!(d.stats.changed, 5);
        assert_eq!(d.stats.total, 6);
        assert_eq!(d.stats.bounds, Some(PixelBounds { x: 0..3, y: 0..2 }));

        // Sides swapped: the same canvas and mask.
        let swapped = diff(buffer(1, 2, &second), buffer(3, 1, &first), 255);
        assert_eq!(swapped, d);
    }

    #[test]
    fn a_larger_image_with_an_identical_overlap_changes_only_its_extra_pixels() {
        let small = solid(2, 2, [9, 8, 7, 255]);
        let large = solid(3, 2, [9, 8, 7, 255]);
        let d = diff(buffer(2, 2, &small), buffer(3, 2, &large), 0);
        assert_eq!(changed_at(&d), vec![(2, 0), (2, 1)]);
        assert_eq!(d.stats.bounds, Some(PixelBounds { x: 2..3, y: 0..2 }));
    }

    #[test]
    fn an_alpha_only_difference_is_a_change() {
        // Straight RGBA: the same color at another opacity, and two fully transparent pixels whose
        // hidden colors differ.
        let first = [200, 100, 50, 255, 1, 2, 3, 0];
        let second = [200, 100, 50, 254, 9, 9, 9, 0];
        let d = diff(buffer(2, 1, &first), buffer(2, 1, &second), 0);
        assert_eq!(d.mask, vec![true, true]);
    }

    #[test]
    fn fully_different_images_are_all_changed() {
        let first = solid(5, 4, [0, 0, 0, 255]);
        let second = solid(5, 4, [255, 255, 255, 255]);
        let d = diff(buffer(5, 4, &first), buffer(5, 4, &second), 10);
        assert_eq!(d.mask, vec![true; 20]);
        assert_eq!(
            d.stats,
            PixelStats {
                changed: 20,
                total: 20,
                bounds: Some(PixelBounds { x: 0..5, y: 0..4 }),
            }
        );
        assert_eq!(d.stats.changed_percent(), 100.0);
    }

    #[test]
    fn statistics_count_changed_pixels_and_their_bounds() {
        let first = solid(10, 10, [0, 0, 0, 255]);
        let mut second = first.clone();
        for (x, y) in [(1, 2), (7, 3), (4, 8)] {
            second[(y * 10 + x) * 4 + 1] = 50;
        }
        let d = diff(buffer(10, 10, &first), buffer(10, 10, &second), 0);
        assert_eq!(
            d.stats,
            PixelStats {
                changed: 3,
                total: 100,
                bounds: Some(PixelBounds { x: 1..8, y: 2..9 }),
            }
        );
        assert_eq!(d.stats.changed_percent(), 3.0);
        assert_eq!(PixelStats::default().changed_percent(), 0.0);
    }

    #[test]
    fn empty_images_compare_to_an_empty_canvas() {
        let d = diff(buffer(0, 0, &[]), buffer(0, 0, &[]), 0);
        assert_eq!((d.width, d.height), (0, 0));
        assert!(d.mask.is_empty());
        assert_eq!(d.stats.changed_percent(), 0.0);
    }

    #[test]
    #[should_panic(expected = "RGBA length")]
    fn a_buffer_that_does_not_match_its_size_is_rejected() {
        diff(buffer(2, 2, &[0; 12]), buffer(2, 2, &[0; 16]), 0);
    }

    #[test]
    fn cancellation_returns_none() {
        let rgba = solid(4, 4, [0; 4]);
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            pixel_diff(buffer(4, 4, &rgba), buffer(4, 4, &rgba), 0, &cancelled),
            None
        );
    }
}
