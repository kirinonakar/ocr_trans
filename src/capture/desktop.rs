//! Physical desktop coordinates, monitor lookup, and region capture.
use super::CaptureRect;
use anyhow::{Context, Result};
use image::{GenericImageView, Rgba, RgbaImage};
use xcap::Monitor;

/// Read fallible xcap geometry once so capture and recording use consistent bounds.
pub(super) fn monitor_rect(monitor: &Monitor) -> Result<CaptureRect> {
    Ok(CaptureRect {
        x: monitor.x().context("Failed to read monitor x coordinate")?,
        y: monitor.y().context("Failed to read monitor y coordinate")?,
        width: monitor.width().context("Failed to read monitor width")? as i32,
        height: monitor.height().context("Failed to read monitor height")? as i32,
    })
}

/// Union of monitor bounds in physical virtual-desktop pixels. Windows anchors the primary
/// monitor at (0, 0) and lets other monitors sit to its left or above, so the origin can be
/// negative. The selector and the Win32 APIs use this same coordinate space.
pub fn virtual_desktop_rect(monitors: &[Monitor]) -> Result<Option<CaptureRect>> {
    let rects = monitors.iter().map(monitor_rect).collect::<Result<Vec<_>>>()?;
    Ok(union_rects(rects.into_iter()))
}

fn union_rects(rects: impl Iterator<Item = CaptureRect>) -> Option<CaptureRect> {
    let mut bounds: Option<(i32, i32, i32, i32)> = None;
    for rect in rects {
        if rect.width <= 0 || rect.height <= 0 {
            continue;
        }
        let right = rect.x.saturating_add(rect.width);
        let bottom = rect.y.saturating_add(rect.height);
        bounds = Some(match bounds {
            Some((left, top, old_right, old_bottom)) => (
                left.min(rect.x),
                top.min(rect.y),
                old_right.max(right),
                old_bottom.max(bottom),
            ),
            None => (rect.x, rect.y, right, bottom),
        });
    }
    bounds.map(|(left, top, right, bottom)| CaptureRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn monitors_overlapping<'a>(
    rect: &CaptureRect,
    monitors: &'a [Monitor],
) -> Result<Vec<(&'a Monitor, CaptureRect)>> {
    let right = rect.x.saturating_add(rect.width.max(1));
    let bottom = rect.y.saturating_add(rect.height.max(1));
    let mut overlapping = Vec::new();
    for monitor in monitors {
        let bounds = monitor_rect(monitor)?;
        let monitor_right = bounds.x.saturating_add(bounds.width);
        let monitor_bottom = bounds.y.saturating_add(bounds.height);
        if bounds.x < right
            && monitor_right > rect.x
            && bounds.y < bottom
            && monitor_bottom > rect.y
        {
            overlapping.push((monitor, bounds));
        }
    }
    Ok(overlapping)
}

/// Physical bounds of every monitor combined. This is the surface the selector covers so a
/// region can be dragged across monitors.
pub fn virtual_desktop_capture_rect() -> Result<CaptureRect> {
    let monitors = Monitor::all().context("Failed to get monitors")?;
    virtual_desktop_rect(&monitors)?.context("No monitors found")
}

/// Captures the complete virtual desktop in physical pixels, stitching every monitor together.
pub fn capture_virtual_desktop() -> Result<(CaptureRect, RgbaImage)> {
    let monitors = Monitor::all().context("Failed to get monitors")?;
    let rect = virtual_desktop_rect(&monitors)?.context("No monitors found")?;
    let image = capture_area(&rect, &Some(monitors))?;
    Ok((rect, image))
}

/// Copies only the requested desktop rectangle, including negative virtual-desktop coordinates.
#[cfg(target_os = "windows")]
fn capture_area_gdi(rect: &CaptureRect) -> Result<RgbaImage> {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let screen = GetDC(None);
        anyhow::ensure!(!screen.is_invalid(), "GetDC failed");
        let memory = CreateCompatibleDC(Some(screen));
        if memory.is_invalid() {
            ReleaseDC(None, screen);
            anyhow::bail!("CreateCompatibleDC failed");
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: rect.width,
                biHeight: -rect.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = (|| -> Result<RgbaImage> {
            let bitmap = CreateDIBSection(
                Some(screen),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )?;
            let previous = SelectObject(memory, bitmap.into());
            let result = (|| -> Result<RgbaImage> {
                anyhow::ensure!(
                    !previous.is_invalid() && previous.0 as isize != -1,
                    "SelectObject failed"
                );
                BitBlt(
                    memory,
                    0,
                    0,
                    rect.width,
                    rect.height,
                    Some(screen),
                    rect.x,
                    rect.y,
                    SRCCOPY | CAPTUREBLT,
                )?;
                anyhow::ensure!(GdiFlush().as_bool(), "GdiFlush failed");
                anyhow::ensure!(!bits.is_null(), "The capture buffer is empty");
                let size = rect.width as usize * rect.height as usize * 4;
                let mut pixels = std::slice::from_raw_parts(bits.cast::<u8>(), size).to_vec();
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                    pixel[3] = 255;
                }
                RgbaImage::from_raw(rect.width as u32, rect.height as u32, pixels)
                    .context("Invalid capture dimensions")
            })();
            SelectObject(memory, previous);
            let _ = DeleteObject(bitmap.into());
            result
        })();
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        result
    }
}

pub fn capture_area(rect: &CaptureRect, monitors: &Option<Vec<Monitor>>) -> Result<RgbaImage> {
    anyhow::ensure!(rect.valid(), "The capture area is invalid");
    // With xcap's default features the fallback below also uses GDI. A black successful
    // BitBlt is not evidence of failure (the scene may be black), and retrying that backend
    // cannot recover protected/overlay video. See README's capture backend limitations.
    #[cfg(target_os = "windows")]
    match capture_area_gdi(rect) {
        Ok(image) => return Ok(image),
        Err(error) => {
            log::debug!("Region BitBlt failed; falling back to monitor capture: {error:?}")
        }
    }

    let local_monitors;
    let monitors_ref = if let Some(m) = monitors {
        m
    } else {
        local_monitors = Monitor::all().context("Failed to get monitors")?;
        &local_monitors
    };

    if monitors_ref.is_empty() {
        anyhow::bail!("No monitors found");
    }

    let width = rect.width.max(1) as u32;
    let height = rect.height.max(1) as u32;
    // A region can span several monitors. Capture every monitor that intersects it and stitch
    // the pieces into one image; the selector, Win32 and xcap all share these physical
    // virtual-desktop coordinates.
    let overlapping = monitors_overlapping(rect, monitors_ref)?;
    if overlapping.is_empty() {
        anyhow::bail!("Capture area is outside every monitor");
    }
    if overlapping.len() == 1 {
        let (monitor, bounds) = overlapping[0];
        let img = monitor
            .capture_image()
            .context("Failed to capture monitor")?;
        let local_x = (rect.x - bounds.x).max(0) as u32;
        let local_y = (rect.y - bounds.y).max(0) as u32;
        if local_x >= img.width() || local_y >= img.height() {
            anyhow::bail!("Capture area is outside the selected monitor");
        }

        let w = width.min(img.width() - local_x);
        let h = height.min(img.height() - local_y);
        return Ok(img.view(local_x, local_y, w, h).to_image());
    }

    let mut canvas = RgbaImage::new(width, height);
    let mut captured = 0usize;
    let mut last_error: Option<String> = None;
    for (monitor, bounds) in overlapping {
        let image = match monitor.capture_image() {
            Ok(image) => image,
            Err(error) => {
                last_error = Some(format!("{error:?}"));
                continue;
            }
        };
        // Intersection of this monitor and the requested region, in both coordinate spaces.
        let left = rect.x.max(bounds.x);
        let top = rect.y.max(bounds.y);
        let right = rect
            .x
            .saturating_add(width as i32)
            .min(bounds.x.saturating_add(bounds.width));
        let bottom = rect
            .y
            .saturating_add(height as i32)
            .min(bounds.y.saturating_add(bounds.height));
        if right <= left || bottom <= top {
            continue;
        }
        let source_x = (left - bounds.x).max(0) as u32;
        let source_y = (top - bounds.y).max(0) as u32;
        if source_x >= image.width() || source_y >= image.height() {
            continue;
        }
        let copy_width = ((right - left) as u32)
            .min(image.width() - source_x)
            .min(width - (left - rect.x) as u32);
        let copy_height = ((bottom - top) as u32)
            .min(image.height() - source_y)
            .min(height - (top - rect.y) as u32);
        if copy_width == 0 || copy_height == 0 {
            continue;
        }
        let piece = image
            .view(source_x, source_y, copy_width, copy_height)
            .to_image();
        image::imageops::replace(
            &mut canvas,
            &piece,
            (left - rect.x) as i64,
            (top - rect.y) as i64,
        );
        captured += 1;
    }

    if captured == 0 {
        anyhow::bail!(
            "Failed to capture the monitors for this region{}",
            last_error
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        );
    }
    Ok(canvas)
}

pub fn monitor_rect_at_point(x: i32, y: i32) -> Result<CaptureRect> {
    let monitor = Monitor::from_point(x, y).context("Failed to find monitor")?;
    monitor_rect(&monitor)
}

pub fn capture_monitor_at_point(x: i32, y: i32) -> Result<(CaptureRect, RgbaImage)> {
    let rect = monitor_rect_at_point(x, y)?;
    let image = capture_area(&rect, &None)?;
    Ok((rect, image))
}

/// Captures the primary monitor, retained for the original OCR selector path.
pub fn capture_full_screen() -> Result<RgbaImage> {
    let monitors = Monitor::all().context("Failed to get monitors")?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .context("No monitors found")?;
    monitor.capture_image().context("Failed to capture monitor")
}

pub fn cursor_position() -> (i32, i32) {
    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }
        #[link(name = "user32")]
        extern "system" {
            fn GetCursorPos(point: *mut Point) -> i32;
        }
        let mut point = Point { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut point) } != 0 {
            return (point.x, point.y);
        }
    }
    (0, 0)
}

/// Finds the monitor under a screen point without going through xcap. This is used only for
/// positioning the compact toolbar, so mode switching does not depend on a live capture handle.
#[cfg(target_os = "windows")]
pub fn native_monitor_rect_at_point(x: i32, y: i32) -> Option<CaptureRect> {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        flags: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn MonitorFromPoint(point: Point, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    }

    let mut info = MonitorInfo {
        cb_size: std::mem::size_of::<MonitorInfo>() as u32,
        rc_monitor: Rect::default(),
        rc_work: Rect::default(),
        flags: 0,
    };
    let monitor = unsafe { MonitorFromPoint(Point { x, y }, 2) };
    if monitor == 0 || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }

    let width = info.rc_monitor.right - info.rc_monitor.left;
    let height = info.rc_monitor.bottom - info.rc_monitor.top;
    (width > 1 && height > 1).then_some(CaptureRect {
        x: info.rc_monitor.left,
        y: info.rc_monitor.top,
        width,
        height,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn native_monitor_rect_at_point(_x: i32, _y: i32) -> Option<CaptureRect> {
    None
}

pub fn sample_pixel_at_point(x: i32, y: i32) -> Result<Rgba<u8>> {
    let monitor = Monitor::from_point(x, y).context("Failed to find monitor")?;
    let image = monitor
        .capture_image()
        .context("Failed to capture monitor")?;
    let local_x = (x - monitor.x().context("Failed to read monitor x coordinate")?).max(0) as u32;
    let local_y = (y - monitor.y().context("Failed to read monitor y coordinate")?).max(0) as u32;
    if local_x >= image.width() || local_y >= image.height() {
        anyhow::bail!("The selected point is outside the monitor");
    }
    Ok(*image.get_pixel(local_x, local_y))
}

#[cfg(test)]
mod tests {
    use super::{union_rects, CaptureRect};

    #[test]
    fn union_rect_covers_monitors_left_and_right_of_the_primary() {
        let bounds = union_rects(
            [
                CaptureRect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                CaptureRect {
                    x: -1080,
                    y: 0,
                    width: 1080,
                    height: 1920,
                },
                CaptureRect {
                    x: 1920,
                    y: 0,
                    width: 2560,
                    height: 1440,
                },
            ]
            .into_iter(),
        )
        .unwrap();

        assert_eq!(bounds.x, -1080);
        assert_eq!(bounds.y, 0);
        assert_eq!(bounds.width, 5560);
        assert_eq!(bounds.height, 1920);
    }

    #[test]
    fn union_rect_ignores_empty_monitor_entries() {
        assert!(union_rects(std::iter::empty()).is_none());
        assert!(union_rects(
            [CaptureRect {
                x: 10,
                y: 10,
                width: 0,
                height: 0,
            }]
            .into_iter()
        )
        .is_none());
    }
}
