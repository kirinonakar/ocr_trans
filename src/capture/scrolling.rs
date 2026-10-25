//! Scroll control, stable frame sampling, and assembly of complete or partial captures.
#[cfg(target_os = "windows")]
mod alignment;

#[cfg(target_os = "windows")]
use self::alignment::aligned_scroll_shift;
use super::{capture_area, CaptureRect, WindowTarget};
use anyhow::Result;
use image::{Rgba, RgbaImage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

fn check_scroll_cancel(cancel: &AtomicBool) -> Result<()> {
    anyhow::ensure!(
        !cancel.load(Ordering::Relaxed),
        "Scrolling capture canceled"
    );
    Ok(())
}

/// Calibrates wheel distance in each application, keeping about half a viewport of overlap.
/// Wheel settings, font sizes and applications all produce different pixels per wheel notch.
struct ScrollStep {
    target_pixels: u32,
    notches: u32,
    maximum_notches: u32,
    pixels_per_notch: Option<f64>,
}

impl ScrollStep {
    fn new(viewport_height: u32) -> Self {
        Self {
            target_pixels: (viewport_height / 2).max(1),
            notches: if viewport_height < 600 { 1 } else { 2 },
            maximum_notches: 16,
            pixels_per_notch: None,
        }
    }

    fn expected_shift(&self) -> Option<u32> {
        self.pixels_per_notch
            .map(|pixels| (pixels * self.notches as f64).round() as u32)
    }

    fn measured(&mut self, pixels: u32) {
        if pixels == 0 {
            return;
        }
        let pixels_per_notch = pixels as f64 / self.notches as f64;
        self.pixels_per_notch = Some(pixels_per_notch);
        self.notches = (self.target_pixels as f64 / pixels_per_notch)
            .round()
            .clamp(1.0, self.maximum_notches.min(self.notches * 3) as f64)
            as u32;
    }

    fn reduce(&mut self) {
        self.maximum_notches = (self.notches / 2).max(1);
        self.notches = self.maximum_notches;
    }
}

pub(crate) struct ScrollingCapture {
    pub(crate) image: RgbaImage,
    /// Captures with verified frames still produce a usable result if later alignment fails.
    pub(crate) warning: Option<String>,
}

/// Scrolls a window and stitches the newly exposed rows, following AIMediaWorker's behavior.
#[cfg(target_os = "windows")]
pub fn scrolling_capture(
    target: WindowTarget,
    cancel: &AtomicBool,
    progress: impl Fn(u32, u32),
) -> Result<ScrollingCapture> {
    if !target.bounds.valid() || target.handle == 0 {
        anyhow::bail!("A valid window is required for scrolling capture");
    }

    // The native window bounds include the 1-2px frame that stays fixed while the client area
    // scrolls. Capturing that frame in every segment creates a visible horizontal seam at each
    // join, so stitch only the inner client image.
    // 좌/우를 다르게 잘라야 한다. 우측 수직 스크롤바(약 12~15px)는 스크롤 중
    // 썸 위치가 바뀌어 매 조각에 다르게 찍히고, 이어붙이면 계단/이중선으로 보인다.
    // 스크롤바를 최종 이미지에 포함하지 않도록 우측을 넓게 제외한다.
    const SCROLL_CAPTURE_LEFT_MARGIN: i32 = 2;
    const SCROLL_CAPTURE_RIGHT_MARGIN: i32 = 16;
    const SCROLL_CAPTURE_TOP_MARGIN: i32 = 2;
    // The bottom of a browser window often contains a thin horizontal scrollbar/resize edge.
    // It is fixed while the page scrolls, so leaving it in each segment creates a repeated line
    // at every stitch. Keep the side/top crop small, but remove that fixed bottom band entirely.
    // The browser's fixed bottom frame can sit several pixels above the native window edge; keep
    // enough distance to exclude the whole band from every segment instead of stitching it into
    // the long image repeatedly.
    const SCROLL_CAPTURE_BOTTOM_MARGIN: i32 = 14;
    // 하드 컷으로 이어붙이면 미세한 정렬/서브픽셀 차이가 가로 이음새로 남는다.
    // 직전 프레임과 겹치는 위쪽 몇 행을 크로스페이드해 경계를 감춘다.
    const SCROLL_CAPTURE_SEAM_FADE: u32 = 6;
    let scroll_bounds = CaptureRect {
        x: target.bounds.x + SCROLL_CAPTURE_LEFT_MARGIN,
        y: target.bounds.y + SCROLL_CAPTURE_TOP_MARGIN,
        width: target.bounds.width - SCROLL_CAPTURE_LEFT_MARGIN - SCROLL_CAPTURE_RIGHT_MARGIN,
        height: target.bounds.height - SCROLL_CAPTURE_TOP_MARGIN - SCROLL_CAPTURE_BOTTOM_MARGIN,
    };
    if !scroll_bounds.valid() {
        anyhow::bail!("The selected window is too small for scrolling capture");
    }

    let center_x = target.bounds.x + target.bounds.width / 2;
    let center_y = target.bounds.y + target.bounds.height / 2;
    let recipient = native_window_at_point(center_x, center_y).unwrap_or(target.handle);
    let point = make_point_parameter(center_x, center_y);

    scroll_to_top(target.handle, recipient, point);
    let mut previous = stable_capture(scroll_bounds, cancel)?;
    let mut segments = vec![(previous.clone(), 0u32)];
    let mut total_height = scroll_bounds.height as u32;
    let maximum_height = (60_000u32)
        .min((80_000_000u64 / scroll_bounds.width as u64) as u32)
        .max(total_height);
    let mut unchanged_steps = 0;
    let mut scroll_step = ScrollStep::new(scroll_bounds.height as u32);
    // 180 스텝 상한은 세로로 긴 문서에서 바닥에 닿기 전에 소진된다. 높이 상한에
    // 도달할 때까지 계속하되, 스크롤이 전혀 먹지 않는 경우에만 멈추도록 안전 상한을 둔다.
    let maximum_steps = (maximum_height / 24).clamp(180, 600);

    let capture_result: Result<()> = (|| {
        for step in 0..maximum_steps {
            check_scroll_cancel(cancel)?;
            progress(step + 1, maximum_steps);
            if total_height >= maximum_height {
                break;
            }
            send_wheel_step(recipient, point, -(scroll_step.notches as i32), cancel)?;
            // Start sampling sooner; stable_capture still requires two matching frames, so a
            // smooth-scroll animation cannot be stitched just because this short delay elapsed.
            thread::sleep(Duration::from_millis(40));
            let mut current = stable_capture(scroll_bounds, cancel)?;
            if crate::scroll_alignment::frames_stable(&previous, &current) {
                // 한두 번의 변화 없음은 스크롤 이벤트 유실이나 지연 렌더링일 수 있다.
                // 곧바로 끝내지 않고 몇 번 더 확인한 뒤에만 바닥으로 판단한다.
                unchanged_steps += 1;
                if unchanged_steps < 3 {
                    continue;
                }
                break;
            }
            unchanged_steps = 0;
            // Scale the matching prior with the wheel burst. Reusing the old shift after increasing
            // the burst would favor the wrong repetition on tables or repeated paragraphs.
            let expected = scroll_step.expected_shift();
            let mut shift = aligned_scroll_shift(&previous, &current, expected);
            for _ in 0..2 {
                if shift > 0 {
                    break;
                }
                // Delayed rendering must not cause another scroll and lose the remaining overlap.
                current = stable_capture(scroll_bounds, cancel)?;
                shift = aligned_scroll_shift(&previous, &current, expected);
            }
            if shift == 0 {
                if scroll_step.notches > 1 {
                    // Some apps accelerate multi-notch scrolling. Restore the last verified position
                    // before trying a smaller burst; never stitch or advance across an unknown gap.
                    send_wheel_step(recipient, point, scroll_step.notches as i32, cancel)?;
                    thread::sleep(Duration::from_millis(40));
                    let restored = stable_capture(scroll_bounds, cancel)?;
                    anyhow::ensure!(
                        crate::scroll_alignment::frames_stable(&previous, &restored)
                            || crate::scroll_alignment::same_position(&previous, &restored),
                        "Could not restore the scrolling position after an unaligned frame"
                    );
                    scroll_step.reduce();
                    continue;
                }
                anyhow::bail!(
                    "Could not align scrolling frames; capture stopped to avoid missing content"
                );
            }
            scroll_step.measured(shift);
            shift = shift.min(maximum_height - total_height);
            if shift == 0 {
                break;
            }
            // 직전 프레임과 겹치는 위쪽 행을 함께 잘라 두어 최종 결합에서 크로스페이드한다.
            let fade = SCROLL_CAPTURE_SEAM_FADE
                .min(shift)
                .min(current.height().saturating_sub(shift));
            segments.push((copy_bottom_rows(&current, shift + fade), fade));
            total_height += shift;
            previous = current;
        }

        Ok(())
    })();

    // A failed later frame must not discard the frames we already aligned. Explicit Escape
    // remains cancellation; every other failure produces a marked partial image.
    check_scroll_cancel(cancel)?;
    if capture_result.is_ok() {
        scroll_to_top(target.handle, recipient, point);
    }
    finish_scrolling_capture(
        segments,
        scroll_bounds.width as u32,
        total_height,
        cancel,
        capture_result,
    )
}

#[cfg(target_os = "windows")]
fn finish_scrolling_capture(
    segments: Vec<(RgbaImage, u32)>,
    width: u32,
    total_height: u32,
    cancel: &AtomicBool,
    capture_result: Result<()>,
) -> Result<ScrollingCapture> {
    check_scroll_cancel(cancel)?;
    let warning = capture_result.err().map(|error| {
        log::warn!("Saving partial scrolling capture: {error:#}");
        error.to_string()
    });
    let mut result = RgbaImage::new(width, total_height);
    let mut y = 0u32;
    for (segment, overlap) in segments.into_iter() {
        // 겹침 행은 직전 세그먼트가 이미 기록한 행의 재렌더링이다. 기록 지점을
        // overlap 만큼 위로 올려 그 구간을 크로스페이드하면, 미세한 정렬/서브픽셀
        // 차이가 하드 컷처럼 드러나지 않는다.
        let start = y.saturating_sub(overlap);
        for row in 0..segment.height() {
            check_scroll_cancel(cancel)?;
            let dst_y = start + row;
            if dst_y >= result.height() {
                break;
            }
            if overlap > 0 && dst_y < y {
                let alpha = (row + 1) as f32 / (overlap + 1) as f32;
                for x in 0..segment.width() {
                    let src = *segment.get_pixel(x, row);
                    let dst = *result.get_pixel(x, dst_y);
                    result.put_pixel(
                        x,
                        dst_y,
                        Rgba([
                            blend_channel(dst[0], src[0], alpha),
                            blend_channel(dst[1], src[1], alpha),
                            blend_channel(dst[2], src[2], alpha),
                            255,
                        ]),
                    );
                }
            } else {
                for x in 0..segment.width() {
                    result.put_pixel(x, dst_y, *segment.get_pixel(x, row));
                }
            }
        }
        y = start + segment.height();
        if y >= result.height() {
            break;
        }
    }
    Ok(ScrollingCapture {
        image: result,
        warning,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn scrolling_capture(
    _target: WindowTarget,
    _cancel: &AtomicBool,
    _progress: impl Fn(u32, u32),
) -> Result<ScrollingCapture> {
    anyhow::bail!("Scrolling capture is available on Windows only")
}

#[cfg(target_os = "windows")]
fn native_window_at_point(x: i32, y: i32) -> Option<isize> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn WindowFromPoint(point: Point) -> isize;
        fn GetAncestor(window: isize, flags: u32) -> isize;
    }
    let child = unsafe { WindowFromPoint(Point { x, y }) };
    (child != 0)
        .then(|| unsafe { GetAncestor(child, 2) })
        .filter(|handle| *handle != 0)
}

#[cfg(target_os = "windows")]
fn send_message(window: isize, message: u32, wparam: usize, lparam: isize) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn SendMessageTimeoutW(
            window: isize,
            message: u32,
            wparam: usize,
            lparam: isize,
            flags: u32,
            timeout_ms: u32,
            result: *mut usize,
        ) -> isize;
    }
    let mut result = 0usize;
    unsafe { SendMessageTimeoutW(window, message, wparam, lparam, 0x0002, 100, &mut result) != 0 }
}

#[cfg(target_os = "windows")]
fn make_point_parameter(x: i32, y: i32) -> isize {
    (((y as u32 & 0xffff) << 16) | (x as u32 & 0xffff)) as isize
}

#[cfg(target_os = "windows")]
fn scroll_to_top(window: isize, recipient: isize, point: isize) {
    // WM_VSCROLL / SB_TOP followed by several large wheel messages handles browsers and
    // ordinary scroll viewers, just like the reference implementation.
    let _ = send_message(window, 0x0115, 6, 0);
    for _ in 0..64 {
        if !send_message(recipient, 0x020A, (120i32 << 16) as u32 as usize, point) {
            break;
        }
    }
    thread::sleep(Duration::from_millis(250));
}

#[cfg(target_os = "windows")]
fn send_wheel_step(
    recipient: isize,
    point: isize,
    notches: i32,
    cancel: &AtomicBool,
) -> Result<()> {
    let delta = if notches < 0 { -120i32 } else { 120i32 };
    for _ in 0..notches.unsigned_abs() {
        check_scroll_cancel(cancel)?;
        anyhow::ensure!(
            send_message(recipient, 0x020A, ((delta << 16) as u32) as usize, point),
            "The selected window is not responding to scrolling"
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn stable_capture(rect: CaptureRect, cancel: &AtomicBool) -> Result<RgbaImage> {
    check_scroll_cancel(cancel)?;
    let mut latest = capture_area(&rect, &None)?;
    let mut stable = 0;
    // 한 번의 일치는 스크롤 감속/관성 애니메이션이 잠깐 멈춘 순간일 수 있다.
    // 두 번 연속 일치해야 정지로 보고, 이동 중 프레임이 섞여 shift가 틀어지는 것을 막는다.
    for _ in 0..16 {
        check_scroll_cancel(cancel)?;
        thread::sleep(Duration::from_millis(60));
        let next = capture_area(&rect, &None)?;
        if crate::scroll_alignment::frames_stable(&latest, &next) {
            stable += 1;
            if stable >= 2 {
                return Ok(next);
            }
        } else {
            stable = 0;
        }
        latest = next;
    }
    Ok(latest)
}

#[cfg(target_os = "windows")]
fn copy_bottom_rows(image: &RgbaImage, rows: u32) -> RgbaImage {
    image::imageops::crop_imm(image, 0, image.height() - rows, image.width(), rows).to_image()
}

#[cfg(target_os = "windows")]
/// alpha=0이면 base, 1이면 over. 이음새 크로스페이드에만 쓰는 저비용 채널 보간.
fn blend_channel(base: u8, over: u8, alpha: f32) -> u8 {
    let blended = base as f32 + (over as f32 - base as f32) * alpha;
    blended.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    #[test]
    fn scroll_distance_adapts_to_app_speed_and_preserves_overlap() {
        let mut step = super::ScrollStep::new(1080);
        assert_eq!(step.notches, 2);
        step.measured(120); // 60 pixels per notch.
        assert_eq!(step.notches, 6); // Limit the first increase while calibrating.
        assert_eq!(step.expected_shift(), Some(360));
        step.measured(360);
        assert_eq!(step.notches, 9);
        assert_eq!(step.expected_shift(), Some(540));
        step.measured(540);
        assert_eq!(step.notches, 9);

        let mut fast = super::ScrollStep::new(1080);
        fast.measured(600);
        assert_eq!(fast.notches, 2);
        assert_eq!(fast.expected_shift(), Some(600));
        let small = super::ScrollStep::new(300);
        assert_eq!(small.notches, 1);
    }

    #[test]
    fn rejected_large_scrolls_reduce_the_limit_and_do_not_repeat_the_same_burst() {
        let mut step = super::ScrollStep::new(1080);
        step.measured(120);
        step.reduce();
        assert_eq!(step.notches, 3);
        assert_eq!(step.expected_shift(), Some(180));
        step.measured(180);
        assert_eq!(step.notches, 3);
        step.reduce();
        assert_eq!(step.notches, 1);
        step.measured(60);
        assert_eq!(step.notches, 1);
    }

    #[test]
    fn calibrated_scrolling_reduces_capture_steps_for_long_documents() {
        let mut step = super::ScrollStep::new(1080);
        let distance = 30_000u32;
        let mut captured = 0;
        let mut captures = 0;
        while captured < distance {
            let shift = (step.notches * 60).min(distance - captured);
            captured += shift;
            captures += 1;
            step.measured(shift);
        }
        let old_captures = distance.div_ceil(120);
        assert!(captures < old_captures / 3, "{captures} vs {old_captures}");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn failed_alignment_preserves_all_verified_frames_in_partial_png() {
        let pixel = |y: u32| Rgba([y as u8, y.wrapping_mul(3) as u8, 80, 255]);
        let first = RgbaImage::from_fn(24, 80, |_, y| pixel(y));
        let next = RgbaImage::from_fn(24, 80, |_, y| pixel(y + 30));
        let segments = vec![(first, 0), (super::copy_bottom_rows(&next, 36), 6)];
        let capture = super::finish_scrolling_capture(
            segments,
            24,
            110,
            &super::AtomicBool::new(false),
            Err(anyhow::anyhow!("alignment failed")),
        )
        .unwrap();
        assert_eq!(capture.warning.as_deref(), Some("alignment failed"));
        assert_eq!(capture.image.dimensions(), (24, 110));
        for y in 0..110 {
            assert_eq!(*capture.image.get_pixel(10, y), pixel(y));
        }
        let path = std::env::temp_dir().join(format!("ocr-partial-{}.png", uuid::Uuid::new_v4()));
        capture.image.save(&path).unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8(), capture.image);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn first_frame_survives_alignment_failure_but_escape_still_cancels() {
        let first = RgbaImage::from_pixel(24, 80, Rgba([40, 60, 80, 255]));
        let capture = super::finish_scrolling_capture(
            vec![(first.clone(), 0)],
            24,
            80,
            &super::AtomicBool::new(false),
            Err(anyhow::anyhow!("alignment failed")),
        )
        .unwrap();
        assert_eq!(capture.image, first);
        assert!(capture.warning.is_some());
        assert!(super::finish_scrolling_capture(
            vec![(first, 0)],
            24,
            80,
            &super::AtomicBool::new(true),
            Err(anyhow::anyhow!("canceled"))
        )
        .is_err());
    }

    #[test]
    fn scroll_cancellation_is_reported_before_work() {
        let canceled = super::AtomicBool::new(true);
        assert!(super::check_scroll_cancel(&canceled)
            .unwrap_err()
            .to_string()
            .contains("canceled"));
    }
}
