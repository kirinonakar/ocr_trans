//! Scroll-offset matching and pixel-level refinement for captured frames.
use image::{Rgba, RgbaImage};

#[cfg(target_os = "windows")]
pub(super) fn aligned_scroll_shift(
    previous: &RgbaImage,
    current: &RgbaImage,
    expected: Option<u32>,
) -> u32 {
    // Once calibrated, the next shift is usually known to within a pixel. Accept only a very
    // strong match here; short final steps or changed scroll behavior use the full search below.
    if let Some(expected) = expected {
        for candidate in [
            expected,
            expected.saturating_sub(1),
            expected.saturating_add(1),
            expected.saturating_sub(2),
            expected.saturating_add(2),
        ] {
            if candidate > 0
                && candidate < previous.height()
                && score_shift(previous, current, candidate, false) >= 0.95
            {
                return refine_vertical_shift(previous, current, candidate);
            }
        }
    }
    let mut shift = find_vertical_shift(previous, current, false, expected);
    if shift == 0 {
        shift = find_vertical_shift(previous, current, true, expected);
    }
    if shift > 0 {
        refine_vertical_shift(previous, current, shift)
    } else {
        crate::scroll_alignment::find_shift(previous, current, expected)
    }
}

#[cfg(target_os = "windows")]
fn find_vertical_shift(
    previous: &RgbaImage,
    current: &RgbaImage,
    relaxed: bool,
    expected: Option<u32>,
) -> u32 {
    let height = previous.height();
    // 완화 모드에서는 더 작은 이동도, 창 높이에 가까운 큰 이동도 허용한다.
    let minimum = if relaxed {
        8u32.max(height / 80)
    } else {
        12u32.max(height / 60)
    };
    let mut maximum = minimum.max(if relaxed {
        height * 4 / 5
    } else {
        height * 2 / 3
    });
    // 직전 스텝의 이동량을 알고 있으면 그보다 훨씬 큰 후보(완화 모드의 창 높이 근처 값 등)를
    // 배제한다. 모호한 반복 패턴은 아래에서 "직전 값에 가장 가까운 후보"를 골라 해결하므로,
    // 상한은 실제 이동량을 잘라내지 않도록 넉넉하게 둔다.
    if let Some(expected) = expected {
        maximum = maximum
            .min(expected.saturating_mul(4).saturating_add(32))
            .max(minimum);
    }
    // 4px 간격 coarse 탐색은 1px 최적점을 놓쳐 이웃 봉우리에 걸리기 쉽다.
    // 2px 간격으로 상위 후보 3개를 추린 뒤 각 후보 ±2를 1px 단위로 정밀 탐색한다.
    let mut coarse_scores: Vec<(u32, f64)> = Vec::new();
    let mut shift = minimum;
    while shift <= maximum {
        let score = score_shift(previous, current, shift, relaxed);
        if score > 0.0 {
            coarse_scores.push((shift, score));
        }
        shift += 2;
    }
    // Text normally gives a broad enough peak for 2px sampling. Fine detail can have a
    // one-pixel peak (including the shorter final scroll), so check the skipped phase before
    // declaring alignment failure. The normal, well-aligned path keeps its existing cost.
    let required = if relaxed { 0.6 } else { 0.68 };
    if !coarse_scores.iter().any(|(_, score)| *score >= required) {
        let mut shift = minimum + 1;
        while shift <= maximum {
            let score = score_shift(previous, current, shift, relaxed);
            if score > 0.0 {
                coarse_scores.push((shift, score));
            }
            shift += 2;
        }
    }
    if coarse_scores.is_empty() {
        return 0;
    }
    let best_coarse = coarse_scores
        .iter()
        .map(|(_, score)| *score)
        .fold(0.0f64, f64::max);
    // 차트/그리드처럼 세로 반복이 강한 페이지는 주기의 배수 위치에서도 점수가 비슷하게
    // 높다. 최고점만 고르면 실제보다 큰 이동량으로 잘못 이어붙여 내용이 겹쳐 보이므로,
    // 최고점과 비슷한 후보 중 직전 스텝 이동량에 가장 가까운 값을 고른다.
    let mut candidates: Vec<(u32, f64)> = coarse_scores
        .into_iter()
        .filter(|(_, score)| *score >= best_coarse - 0.06)
        .collect();
    if let Some(expected) = expected {
        candidates.sort_by(|a, b| {
            a.0.abs_diff(expected)
                .cmp(&b.0.abs_diff(expected))
                .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
        });
    } else {
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }
    candidates.truncate(3);
    let mut best_shift = 0u32;
    let mut best_score = 0.0f64;
    for (coarse, _) in candidates {
        let start = minimum.max(coarse.saturating_sub(2));
        let end = maximum.min(coarse + 2);
        for candidate in start..=end {
            let score = score_shift(previous, current, candidate, relaxed);
            if score > best_score {
                best_score = score;
                best_shift = candidate;
            }
        }
    }
    // 텍스트 경계에서는 1px 어긋나도 점수가 0.9 이상으로 높게 나온다.
    // 봉우리가 뭉툭하면 잘못된 shift로 하드 컷 되므로 임계값을 높여 확실할 때만 잇는다.
    // 엄격 모드에서는 확실할 때만 인정하고, 완화 모드에서는 더 낮은 점수도 받아들인다.
    if best_score >= required {
        if !relaxed {
            // 이웃 shift와 점수 차가 너무 작으면(평탄한 봉우리) 오측정 위험이 있어 파기한다.
            let neighbor_best = [best_shift.saturating_sub(1), best_shift + 1]
                .into_iter()
                .filter(|s| *s >= minimum && *s <= maximum && *s != best_shift)
                .map(|s| score_shift(previous, current, s, relaxed))
                .fold(0.0f64, f64::max);
            if best_score - neighbor_best < 0.015 {
                // 거의 완벽한 일치(0.95 이상)는 평탄해도 정답으로 인정한다. 그 외에는
                // 직전 스텝 이동량을 알고 있으면 그 근처 값을 신뢰하고, 모르면 포기한다.
                if best_score < 0.95 && expected.is_none() {
                    return 0;
                }
            }
        }
        best_shift
    } else {
        0
    }
}

#[cfg(target_os = "windows")]
fn score_shift(previous: &RgbaImage, current: &RgbaImage, shift: u32, relaxed: bool) -> f64 {
    let width = previous.width();
    let height = previous.height();
    // 상단 고정 UI(탭/주소창)와 하단 고정 밴드를 매칭에서 제외한다.
    let top_margin = 4u32.max(height / 8);
    let bottom = height
        .saturating_sub(shift)
        .saturating_sub(4u32.max(height / 20));
    let left = 2u32.max(width / 12);
    let right = width.saturating_sub(left);
    if bottom <= top_margin + 8 || right <= left + 8 {
        return 0.0;
    }
    let mut informative = 0u32;
    let mut matches = 0u32;
    let mut y = top_margin;
    while y < bottom {
        let mut x = left;
        while x < right {
            let prev_here = previous.get_pixel(x, y);
            let curr_here = current.get_pixel(x, y);
            // 스크롤되지 않은 고정 영역(헤더/배경)은 판별에 쓰지 않는다.
            if color_distance(prev_here, curr_here) < 30 {
                x += 6;
                continue;
            }
            let old = previous.get_pixel(x, y + shift);
            // 평탄한 배경은 어떤 shift에서도 맞으므로 제외하고,
            // 수평 엣지(텍스트 경계)만으로 1px 정밀도를 확보한다.
            let neighbor_x = x.saturating_add(2).min(width - 1);
            if color_distance(old, previous.get_pixel(neighbor_x, y + shift)) < 30 {
                x += 6;
                continue;
            }
            informative += 1;
            if color_distance(old, curr_here) <= 24 {
                matches += 1;
            }
            x += 6;
        }
        y += 4;
    }
    let required_informative = if relaxed { 32 } else { 80 };
    if informative < required_informative {
        0.0
    } else {
        matches as f64 / informative as f64
    }
}

#[cfg(target_os = "windows")]
/// 희소 샘플링으로 고른 shift는 1px 어긋나기 쉽고, 그 1px이 이음새(가로줄)로 그대로 남는다.
/// 겹치는 영역을 촘촘히 비교해 후보 ±2 중 실제 오차가 가장 작은 값을 확정한다.
fn refine_vertical_shift(previous: &RgbaImage, current: &RgbaImage, shift: u32) -> u32 {
    let height = previous.height();
    let width = previous.width();
    if shift == 0 || width < 8 || height < 16 {
        return shift;
    }
    let top_margin = 4u32.max(height / 8);
    let bottom_fixed = 4u32.max(height / 20);
    let lower = shift.saturating_sub(2).max(1);
    let upper = shift + 2;
    let mut best = shift;
    let mut best_error = f64::INFINITY;
    for candidate in lower..=upper {
        if candidate + bottom_fixed + top_margin >= height {
            continue;
        }
        let limit = height - candidate - bottom_fixed;
        let mut error = 0f64;
        let mut count = 0f64;
        let mut y = top_margin;
        while y < limit {
            let mut x = 2u32;
            while x + 2 < width {
                error += color_distance(
                    previous.get_pixel(x, y + candidate),
                    current.get_pixel(x, y),
                ) as f64;
                count += 1.0;
                x += 2;
            }
            y += 2;
        }
        if count > 0.0 {
            let mean = error / count;
            if mean < best_error {
                best_error = mean;
                best = candidate;
            }
        }
    }
    best
}

fn color_distance(first: &Rgba<u8>, second: &Rgba<u8>) -> u32 {
    (first[0] as i32 - second[0] as i32).unsigned_abs()
        + (first[1] as i32 - second[1] as i32).unsigned_abs()
        + (first[2] as i32 - second[2] as i32).unsigned_abs()
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    #[cfg(target_os = "windows")]
    #[test]
    fn scroll_alignment_handles_large_steps_and_short_final_steps() {
        fn frame(offset: u32) -> RgbaImage {
            RgbaImage::from_fn(240, 1000, |x, y| {
                let value = x.wrapping_mul(73856093) ^ (y + offset).wrapping_mul(19349663);
                Rgba([value as u8, (value >> 8) as u8, (value >> 16) as u8, 255])
            })
        }
        let first = frame(0);
        let second = frame(550);
        assert_eq!(super::aligned_scroll_shift(&first, &second, Some(550)), 550);
        // Near the bottom the remaining distance is smaller than the calibrated burst.
        let last = frame(673);
        assert_eq!(super::aligned_scroll_shift(&second, &last, Some(550)), 123);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn chart_only_frames_use_the_sparse_alignment_fallback() {
        let frame = |offset| {
            RgbaImage::from_fn(240, 600, |_, y| {
                if y < 75 {
                    return Rgba([32, 38, 46, 255]);
                }
                let y = y + offset;
                if [
                    (100, 124),
                    (183, 212),
                    (291, 326),
                    (410, 437),
                    (493, 524),
                    (612, 657),
                ]
                .iter()
                .any(|(a, b)| y >= *a && y < *b)
                {
                    Rgba([225, 230, 238, 255])
                } else {
                    Rgba([244, 246, 250, 255])
                }
            })
        };
        let (old, new) = (frame(0), frame(173));
        assert_eq!(super::score_shift(&old, &new, 173, false), 0.0);
        assert_eq!(super::aligned_scroll_shift(&old, &new, None), 173);
    }
}
