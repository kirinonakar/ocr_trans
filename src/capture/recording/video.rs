//! Video source selection, output dimensions, and paced raw-frame delivery.
use super::super::{capture_area, CaptureRect};
#[cfg(target_os = "windows")]
use anyhow::{Context, Result};
use std::io::Write;
use std::process::ChildStdin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(target_os = "windows")]
use xcap::Monitor;

const RECORDING_MAX_WIDTH: i32 = 2560;
const RECORDING_MAX_HEIGHT: i32 = 1440;

fn recording_output_size(width: i32, height: i32) -> (i32, i32) {
    let scale = (RECORDING_MAX_WIDTH as f64 / width as f64)
        .min(RECORDING_MAX_HEIGHT as f64 / height as f64)
        .min(1.0);
    let scaled_width = ((width as f64 * scale).floor() as i32 & !1).max(2);
    let scaled_height = ((height as f64 * scale).floor() as i32 & !1).max(2);
    (scaled_width, scaled_height)
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
pub(super) struct DdaGrabConfig {
    pub(super) input_filter: String,
    pub(super) output_width: i32,
    pub(super) output_height: i32,
}

#[cfg(target_os = "windows")]
pub(super) fn ddagrab_config(rect: CaptureRect, fps: u32) -> Result<Option<DdaGrabConfig>> {
    let monitors = Monitor::all().context("Failed to enumerate monitors for ddagrab")?;
    let Some((enumeration_index, monitor)) = monitors.iter().enumerate().find(|(_, monitor)| {
        let right = rect.x.saturating_add(rect.width);
        let bottom = rect.y.saturating_add(rect.height);
        rect.x >= monitor.x()
            && rect.y >= monitor.y()
            && right <= monitor.x().saturating_add(monitor.width() as i32)
            && bottom <= monitor.y().saturating_add(monitor.height() as i32)
    }) else {
        // One ddagrab source addresses one DXGI output. Keep cross-monitor selections on the
        // existing raw-frame path instead of silently recording the wrong display.
        log::warn!("Recording spans multiple monitors; falling back to raw capture");
        return Ok(None);
    };

    // DISPLAY<n> is a persistent Windows device label, not a DXGI output index (the primary
    // display can legitimately be DISPLAY2). EnumDisplayMonitors and ddagrab enumerate active
    // outputs in the same primary-first order, so use that zero-based position directly.
    let output_index = enumeration_index;
    let offset_x = rect.x - monitor.x();
    let offset_y = rect.y - monitor.y();
    let (output_width, output_height) = recording_output_size(rect.width, rect.height);
    let mut input_filter = format!(
        "ddagrab=output_idx={output_index}:draw_mouse=1:framerate={fps}:video_size={}x{}:offset_x={offset_x}:offset_y={offset_y}",
        rect.width, rect.height
    );
    // h264_nvenc accepts BGRA and performs the final color conversion on the encoder. Make the
    // D3D11 frame transition explicit; otherwise an output `-pix_fmt yuv420p` makes FFmpeg try
    // to insert an unsupported software conversion directly after a hardware frame.
    input_filter.push_str(",hwdownload,format=bgra");
    if output_width != rect.width || output_height != rect.height {
        // Cap large sources at 1440p while preserving aspect ratio. NVENC uploads and encodes
        // the resulting BGRA frames on NVIDIA.
        input_filter.push_str(&format!(
            ",scale={output_width}:{output_height}:flags=fast_bilinear"
        ));
    }

    Ok(Some(DdaGrabConfig {
        input_filter,
        output_width,
        output_height,
    }))
}

pub(super) fn record_frames(
    stdin: &mut ChildStdin,
    rect: CaptureRect,
    interval: Duration,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    mut last_frame: Option<Vec<u8>>,
) {
    // 매 프레임 Monitor::all()을 호출하면 열거 비용으로 30fps를 못 지켜
    // 입력보다 짧은 영상이 되며 빨리감기처럼 보인다. 미리 한 번만 조회해 재사용한다.
    let cached_monitors: Option<Vec<xcap::Monitor>> =
        xcap::Monitor::all().ok().filter(|m| !m.is_empty());
    let mut next_frame = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let is_paused = paused.load(Ordering::Relaxed);
        if !is_paused {
            match capture_area(&rect, &cached_monitors) {
                Ok(image) => {
                    let bytes = image.as_raw().to_vec();
                    if stdin.write_all(&bytes).is_err() {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                    last_frame = Some(bytes);
                }
                Err(error) => {
                    log::warn!("Recording frame capture failed: {error:?}");
                    // 캡처가 느리거나 실패해도 프레임 슬롯을 비우면
                    // 실제보다 짧은 영상이 되어 빨리감기 재생이 된다.
                    // 직전 프레임을 복제해 초당 fps를 일정하게 유지한다.
                    if let Some(prev) = last_frame.as_ref() {
                        if stdin.write_all(prev).is_err() {
                            stop.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
        } else if let Some(prev) = last_frame.as_ref() {
            // 일시정지 중에도 비디오 타임라인은 멈추면 안 된다.
            // 오디오(무음)는 계속 흐르는데 영상만 멈추면 A/V 길이가 어긋나
            // aresample 보정으로 소리가 끊기게 되므로 마지막 프레임을 반복한다.
            if stdin.write_all(prev).is_err() {
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
        // 절대 시각 기준으로 다음 프레임까지 대기해 드리프트를 막는다.
        // 캡처가 interval을 초과하면 밀린 만큼 따라잡지 않고 현재 시각으로 리셋한다.
        next_frame += interval;
        let now = Instant::now();
        if next_frame > now {
            thread::sleep(next_frame - now);
        } else if now.duration_since(next_frame) > interval * 5 {
            next_frame = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::recording_output_size;

    #[test]
    fn recording_size_caps_4k_at_1440p() {
        assert_eq!(recording_output_size(3840, 2160), (2560, 1440));
    }

    #[test]
    fn recording_size_preserves_wide_and_portrait_aspect_ratios() {
        assert_eq!(recording_output_size(3440, 1440), (2560, 1070));
        assert_eq!(recording_output_size(1600, 2000), (1152, 1440));
    }

    #[test]
    fn recording_size_does_not_upscale_small_frames() {
        assert_eq!(recording_output_size(1920, 1080), (1920, 1080));
    }
}
