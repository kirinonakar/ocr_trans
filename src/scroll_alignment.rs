//! Fallback registration for sparse charts and low-contrast report layouts.
//! Row signatures preserve horizontal position; vertical derivatives find horizontal grid/bar
//! edges as well as text. Blank backgrounds contribute no evidence to the correlation.
use image::RgbaImage;

const BINS: usize = 24;

struct Rows {
    colors: Vec<[f64; BINS]>,
    edges: Vec<[f64; BINS]>,
}

impl Rows {
    fn new(image: &RgbaImage) -> Self {
        let width = image.width() as usize;
        let height = image.height() as usize;
        let mut colors = vec![[0.0; BINS]; height];
        for (y, row) in colors.iter_mut().enumerate() {
            for (bin, value) in row.iter_mut().enumerate() {
                let start = width * bin / BINS;
                let end = (width * (bin + 1) / BINS).max(start + 1).min(width);
                let mut count = 0;
                for x in (start..end).step_by(2) {
                    let p = image.get_pixel(x as u32, y as u32);
                    *value +=
                        (p[0] as f64 * 0.2126) + (p[1] as f64 * 0.7152) + (p[2] as f64 * 0.0722);
                    count += 1;
                }
                if count > 0 {
                    *value /= count as f64;
                }
            }
        }
        let mut edges = vec![[0.0; BINS]; height];
        // A small vertical blur tolerates Chromium's fractional scroll offsets/antialiasing
        // while retaining thin horizontal chart strokes.
        for y in 2..height.saturating_sub(2) {
            for bin in 0..BINS {
                edges[y][bin] = (colors[y + 2][bin] + 2.0 * colors[y + 1][bin]
                    - 2.0 * colors[y - 1][bin]
                    - colors[y - 2][bin])
                    / 4.0;
            }
        }
        Self { colors, edges }
    }
}

fn correlation(old: &Rows, new: &Rows, shift: usize, ignore_fixed: bool) -> Option<f64> {
    let height = old.colors.len();
    let start = (height / 8).max(2);
    let end = height.checked_sub(shift + (height / 20).max(2))?;
    if end <= start + 8 {
        return None;
    }
    let (mut dot, mut a_energy, mut b_energy, mut error) = (0.0, 0.0, 0.0, 0.0);
    let (mut samples, mut rows) = (0, 0);
    let (mut first_row, mut last_row) = (None, 0);
    let mut columns = [false; BINS];
    for y in start..end {
        let mut informative_row = false;
        for (bin, column) in columns.iter_mut().enumerate() {
            let a = old.edges[y + shift][bin];
            let b = new.edges[y][bin];
            if a.abs().max(b.abs()) < 2.0 {
                continue;
            }
            // Ignore browser chrome and fixed controls, which remain at their original position.
            if ignore_fixed
                && (old.colors[y][bin] - new.colors[y][bin]).abs() < 1.0
                && (old.edges[y][bin] - b).abs() < 1.0
            {
                continue;
            }
            dot += a * b;
            a_energy += a * a;
            b_energy += b * b;
            error += (old.colors[y + shift][bin] - new.colors[y][bin]).abs();
            samples += 1;
            informative_row = true;
            *column = true;
        }
        if informative_row {
            rows += 1;
            first_row.get_or_insert(y);
            last_row = y;
        }
    }
    if samples < 24
        || rows < 4
        || first_row.is_none_or(|first| last_row - first < 24)
        || columns.iter().filter(|column| **column).count() < 2
        || a_energy < 64.0
        || b_energy < 64.0
        || error / samples as f64 > 18.0
    {
        return None;
    }
    Some(dot / (a_energy * b_energy).sqrt())
}

pub(crate) fn find_shift(previous: &RgbaImage, current: &RgbaImage, expected: Option<u32>) -> u32 {
    if previous.dimensions() != current.dimensions()
        || previous.width() < BINS as u32
        || previous.height() < 32
    {
        return 0;
    }
    let (old, new) = (Rows::new(previous), Rows::new(current));
    if let Some(expected) = expected {
        let mut nearby = Vec::new();
        for shift in expected.saturating_sub(2).max(1)
            ..=expected.saturating_add(2).min(previous.height() * 4 / 5)
        {
            if let Some(score) = correlation(&old, &new, shift as usize, true) {
                if score >= 0.98 {
                    nearby.push((shift, score));
                }
            }
        }
        if let Some((shift, _)) = nearby.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
            return shift;
        }
    }
    let maximum = previous.height() * 4 / 5;
    let mut candidates = Vec::new();
    for shift in 1..=maximum {
        if let Some(score) = correlation(&old, &new, shift as usize, true) {
            if score >= 0.88 {
                candidates.push((shift, score));
            }
        }
    }
    let Some(best) = candidates
        .iter()
        .map(|(_, score)| *score)
        .max_by(f64::total_cmp)
    else {
        return 0;
    };
    candidates.retain(|(_, score)| *score >= best - 0.025);
    if let Some(expected) = expected {
        candidates.sort_by(|a, b| {
            a.0.abs_diff(expected)
                .cmp(&b.0.abs_diff(expected))
                .then_with(|| b.1.total_cmp(&a.1))
        });
    } else {
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        // A grid alone can match several line periods. Do not invent a distance without a prior.
        if candidates
            .iter()
            .skip(1)
            .any(|candidate| candidate.0.abs_diff(candidates[0].0) > 3)
        {
            log::debug!("Sparse scroll alignment is ambiguous without a calibrated distance");
            return 0;
        }
    }
    let (shift, score) = candidates[0];
    log::debug!(
        "Sparse scroll alignment: shift={shift}, correlation={score:.3}, expected={expected:?}"
    );
    shift
}

/// Check every row so thin chart strokes moving between the old 20px sample rows cannot be
/// mistaken for a settled scroll. Static frames still need only the normal two observations.
pub(crate) fn frames_stable(previous: &RgbaImage, current: &RgbaImage) -> bool {
    if previous.dimensions() != current.dimensions() || previous.is_empty() {
        return false;
    }
    let samples = current.width().div_ceil(8) as u64 * current.height() as u64;
    let maximum_changed = (samples / 2000).max(1);
    let (mut changed, mut difference) = (0, 0u64);
    for y in 0..current.height() {
        for x in (0..current.width()).step_by(8) {
            let (a, b) = (previous.get_pixel(x, y), current.get_pixel(x, y));
            let delta: u64 = (0..3).map(|i| a[i].abs_diff(b[i]) as u64).sum();
            difference += delta;
            if delta > 12 {
                changed += 1;
                if changed > maximum_changed {
                    return false;
                }
            }
        }
    }
    difference as f64 / (samples as f64) < 0.6
}

pub(crate) fn same_position(previous: &RgbaImage, current: &RgbaImage) -> bool {
    if previous.dimensions() != current.dimensions()
        || previous.width() < BINS as u32
        || previous.height() < 32
    {
        return false;
    }
    correlation(&Rows::new(previous), &Rows::new(current), 0, false)
        .is_some_and(|score| score >= 0.97)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn chart_frame(offset: u32) -> RgbaImage {
        RgbaImage::from_fn(480, 600, |x, y| {
            if y < 75 {
                return Rgba([32, 38, 46, 255]);
            } // Fixed browser header.
            let y = y + offset;
            // Wide, low-contrast horizontal bars with irregular boundaries. Within each bar
            // there is no left/right edge, which the original matcher required.
            let in_bar = [
                (100, 124),
                (183, 212),
                (291, 326),
                (410, 437),
                (493, 524),
                (612, 657),
                (738, 769),
                (851, 883),
                (934, 969),
            ]
            .iter()
            .any(|(top, bottom)| y >= *top && y < *bottom);
            if in_bar && (30..450).contains(&x) {
                Rgba([225, 230, 238, 255])
            } else {
                Rgba([244, 246, 250, 255])
            }
        })
    }

    #[test]
    fn aligns_sparse_low_contrast_horizontal_chart_edges() {
        assert_eq!(find_shift(&chart_frame(0), &chart_frame(173), None), 173);
        assert_eq!(
            find_shift(&chart_frame(173), &chart_frame(179), Some(300)),
            6
        );
    }

    #[test]
    fn tolerates_fractional_scroll_rendering_on_chart_edges() {
        let (a, b) = (chart_frame(173), chart_frame(174));
        let halfway = RgbaImage::from_fn(a.width(), a.height(), |x, y| {
            let (a, b) = (a.get_pixel(x, y), b.get_pixel(x, y));
            Rgba(std::array::from_fn(|i| {
                ((a[i] as u16 + b[i] as u16) / 2) as u8
            }))
        });
        let shift = find_shift(&chart_frame(0), &halfway, None);
        assert!((173..=174).contains(&shift), "{shift}");
    }

    #[test]
    fn moving_thin_chart_lines_are_not_mistaken_for_settled_frames() {
        let frame = |line| {
            RgbaImage::from_fn(480, 600, |_, y| {
                if y == line {
                    Rgba([225, 230, 238, 255])
                } else {
                    Rgba([244, 246, 250, 255])
                }
            })
        };
        assert!(!frames_stable(&frame(213), &frame(214)));
        assert!(frames_stable(&frame(213), &frame(213)));
    }

    #[test]
    fn distinguishes_unchanged_chart_position_from_another_scroll_position() {
        assert!(same_position(&chart_frame(0), &chart_frame(0)));
        assert!(!same_position(&chart_frame(0), &chart_frame(173)));
    }

    #[test]
    fn refuses_blank_or_ambiguous_grid_frames_without_evidence() {
        let blank = RgbaImage::from_pixel(480, 600, Rgba([245, 245, 245, 255]));
        assert_eq!(find_shift(&blank, &blank, Some(300)), 0);
        let grid = |offset| {
            RgbaImage::from_fn(480, 600, |_, y| {
                if (y + offset) % 40 < 2 {
                    Rgba([220, 220, 220, 255])
                } else {
                    Rgba([245, 245, 245, 255])
                }
            })
        };
        assert_eq!(find_shift(&grid(0), &grid(13), None), 0);
        assert_eq!(find_shift(&grid(0), &grid(13), Some(173)), 173);
    }
}
