use anyhow::{Context, Result};
use image::{GenericImageView, Rgba, RgbaImage};
use std::borrow::Cow;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use xcap::Monitor;

#[cfg(target_os = "windows")]
use std::net::{TcpListener, TcpStream};

static OUTPUT_PATH_STATE: Mutex<Option<(String, u64)>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl CaptureRect {
    pub fn valid(self) -> bool {
        self.width > 1 && self.height > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowTarget {
    pub bounds: CaptureRect,
    /// Native HWND on Windows. It is zero on platforms without Win32.
    pub handle: isize,
}

pub fn capture_area(rect: &CaptureRect, monitors: &Option<Vec<Monitor>>) -> Result<RgbaImage> {
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

    // Find the monitor containing the top-left point. This keeps the coordinate system in
    // physical virtual-desktop pixels, which is also what the selector and Win32 use.
    let monitor = monitors_ref
        .iter()
        .find(|m| {
            rect.x >= m.x()
                && rect.x < m.x() + m.width() as i32
                && rect.y >= m.y()
                && rect.y < m.y() + m.height() as i32
        })
        .unwrap_or(&monitors_ref[0]);

    let img = monitor
        .capture_image()
        .context("Failed to capture monitor")?;
    let local_x = (rect.x - monitor.x()).max(0) as u32;
    let local_y = (rect.y - monitor.y()).max(0) as u32;
    if local_x >= img.width() || local_y >= img.height() {
        anyhow::bail!("Capture area is outside the selected monitor");
    }

    let w = (rect.width.max(1) as u32).min(img.width() - local_x);
    let h = (rect.height.max(1) as u32).min(img.height() - local_y);
    Ok(img.view(local_x, local_y, w, h).to_image())
}

pub fn monitor_rect_at_point(x: i32, y: i32) -> Result<CaptureRect> {
    let monitor = Monitor::from_point(x, y).context("Failed to find monitor")?;
    Ok(CaptureRect {
        x: monitor.x(),
        y: monitor.y(),
        width: monitor.width() as i32,
        height: monitor.height() as i32,
    })
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
        .find(|monitor| monitor.is_primary())
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

/// Returns the top-level external window under a physical screen point.
/// The current process is ignored so the selector/toolbar cannot select itself.
#[cfg(target_os = "windows")]
pub fn window_target_at_point(x: i32, y: i32) -> Option<WindowTarget> {
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
            parameter: isize,
        ) -> i32;
        fn GetWindowRect(window: isize, rect: *mut Rect) -> i32;
        fn IsWindowVisible(window: isize) -> i32;
        fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmGetWindowAttribute(
            window: isize,
            attribute: u32,
            value: *mut Rect,
            value_size: u32,
        ) -> i32;
    }

    fn visible_window_rect(window: isize, fallback: Rect) -> Rect {
        // DWMWA_EXTENDED_FRAME_BOUNDS (9) excludes the invisible resize border that
        // GetWindowRect includes on modern Windows. That border was the source of the margin
        // around window captures. Keep GetWindowRect as a fallback for classic/non-DWM windows.
        let mut visible = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let result = unsafe {
            DwmGetWindowAttribute(window, 9, &mut visible, std::mem::size_of::<Rect>() as u32)
        };
        if result == 0 && visible.right > visible.left && visible.bottom > visible.top {
            visible
        } else {
            fallback
        }
    }

    #[repr(C)]
    struct SearchContext {
        x: i32,
        y: i32,
        current_process: u32,
        window: isize,
        rect: Rect,
    }

    unsafe extern "system" fn enum_window_proc(window: isize, parameter: isize) -> i32 {
        let context = &mut *(parameter as *mut SearchContext);
        if window == 0 || IsWindowVisible(window) == 0 {
            return 1;
        }
        let mut process_id = 0u32;
        GetWindowThreadProcessId(window, &mut process_id);
        if process_id == context.current_process {
            return 1;
        }
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(window, &mut rect) == 0 {
            return 1;
        }
        rect = visible_window_rect(window, rect);
        if context.x >= rect.left
            && context.x < rect.right
            && context.y >= rect.top
            && context.y < rect.bottom
            && rect.right - rect.left > 1
            && rect.bottom - rect.top > 1
        {
            context.window = window;
            context.rect = rect;
            return 0;
        }
        1
    }

    let mut context = SearchContext {
        x,
        y,
        current_process: std::process::id(),
        window: 0,
        rect: Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    };
    unsafe {
        EnumWindows(
            Some(enum_window_proc),
            &mut context as *mut SearchContext as isize,
        );
    }
    if context.window == 0 {
        return None;
    }
    let bounds = CaptureRect {
        x: context.rect.left,
        y: context.rect.top,
        width: context.rect.right - context.rect.left,
        height: context.rect.bottom - context.rect.top,
    };
    bounds.valid().then_some(WindowTarget {
        bounds,
        handle: context.window,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn window_target_at_point(_x: i32, _y: i32) -> Option<WindowTarget> {
    None
}

pub fn capture_window(target: WindowTarget) -> Result<RgbaImage> {
    let image = capture_area(&target.bounds, &None)?;
    const WINDOW_CAPTURE_MARGIN: u32 = 2;
    let crop_width = image.width().saturating_sub(WINDOW_CAPTURE_MARGIN * 2);
    let crop_height = image.height().saturating_sub(WINDOW_CAPTURE_MARGIN * 2);
    if crop_width == 0 || crop_height == 0 {
        anyhow::bail!("The selected window is too small to crop");
    }
    Ok(image::imageops::crop_imm(
        &image,
        WINDOW_CAPTURE_MARGIN,
        WINDOW_CAPTURE_MARGIN,
        crop_width,
        crop_height,
    )
    .to_image())
}

pub fn sample_pixel_at_point(x: i32, y: i32) -> Result<Rgba<u8>> {
    let monitor = Monitor::from_point(x, y).context("Failed to find monitor")?;
    let image = monitor
        .capture_image()
        .context("Failed to capture monitor")?;
    let local_x = (x - monitor.x()).max(0) as u32;
    let local_y = (y - monitor.y()).max(0) as u32;
    if local_x >= image.width() || local_y >= image.height() {
        anyhow::bail!("The selected point is outside the monitor");
    }
    Ok(*image.get_pixel(local_x, local_y))
}

pub fn output_directory() -> PathBuf {
    let pictures = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|path| path.join("Pictures"));
    if let Some(path) = pictures.filter(|path| path.is_dir()) {
        return path;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn configured_output_directory(configured: Option<&str>) -> PathBuf {
    if let Some(folder) = configured
        .map(str::trim)
        .filter(|folder| !folder.is_empty())
    {
        let path = PathBuf::from(folder);
        if path.is_dir() {
            return path;
        }
    }
    output_directory()
}

#[cfg(target_os = "windows")]
fn current_date_for_filename() -> String {
    #[repr(C)]
    struct WindowsSystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(system_time: *mut WindowsSystemTime);
    }

    let mut time = WindowsSystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    unsafe { GetLocalTime(&mut time) };
    if (1..=9999).contains(&time.year)
        && (1..=12).contains(&time.month)
        && (1..=31).contains(&time.day)
    {
        format!("{:04}-{:02}-{:02}", time.year, time.month, time.day)
    } else {
        // GetLocalTime is not expected to fail, but keep the filename usable if it does.
        "0000-00-00".to_string()
    }
}

#[cfg(not(target_os = "windows"))]
fn current_date_for_filename() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    let (year, month, day) = civil_date_from_days(days as i64);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

#[cfg(not(target_os = "windows"))]
fn civil_date_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    // Howard Hinnant's proleptic Gregorian calendar conversion.
    let shifted = days_since_unix_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted / 146_097
    } else {
        (shifted - 146_096) / 146_097
    };
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

pub fn unique_output_path_in(extension: &str, configured_folder: Option<&str>) -> Result<PathBuf> {
    let directory = configured_output_directory(configured_folder);
    std::fs::create_dir_all(&directory).context("Failed to create capture directory")?;
    let date = current_date_for_filename();
    let mut path_state = OUTPUT_PATH_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut index = path_state
        .as_ref()
        .filter(|(last_date, _)| last_date == &date)
        .map(|(_, next_index)| *next_index)
        .unwrap_or(1);

    loop {
        let stem = format!("{date}_{index:03}");
        // Keep the sequence unique across both capture and recording files, even though their
        // extensions differ.
        let taken = ["png", "mp4", extension]
            .into_iter()
            .any(|candidate| directory.join(format!("{stem}.{candidate}")).exists());
        if !taken {
            // Reserve the number before releasing the lock so overlapping capture/record jobs
            // cannot be handed the same filename before either one reaches its save step.
            *path_state = Some((date.clone(), index.saturating_add(1)));
            return Ok(directory.join(format!("{stem}.{extension}")));
        }
        index = index.saturating_add(1);
    }
}

pub fn save_png_and_copy_to(image: &RgbaImage, configured_folder: Option<&str>) -> Result<PathBuf> {
    let path = unique_output_path_in("png", configured_folder)?;
    image.save(&path).context("Failed to save PNG capture")?;
    copy_image_to_clipboard(image)?;
    Ok(path)
}

pub fn copy_image_to_clipboard(image: &RgbaImage) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("Failed to open clipboard")?;
    clipboard
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: Cow::Borrowed(image.as_raw()),
        })
        .context("Failed to copy image to clipboard")
}

pub fn copy_text_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("Failed to open clipboard")?;
    clipboard
        .set_text(text)
        .context("Failed to copy text to clipboard")
}

/// Scrolls a window and stitches the newly exposed rows, following AIMediaWorker's behavior.
#[cfg(target_os = "windows")]
pub fn scrolling_capture(target: WindowTarget) -> Result<RgbaImage> {
    if !target.bounds.valid() || target.handle == 0 {
        anyhow::bail!("A valid window is required for scrolling capture");
    }

    // The native window bounds include the 1-2px frame that stays fixed while the client area
    // scrolls. Capturing that frame in every segment creates a visible horizontal seam at each
    // join, so stitch only the inner client image.
    const SCROLL_CAPTURE_SIDE_MARGIN: i32 = 2;
    const SCROLL_CAPTURE_TOP_MARGIN: i32 = 2;
    // The bottom of a browser window often contains a thin horizontal scrollbar/resize edge.
    // It is fixed while the page scrolls, so leaving it in each segment creates a repeated line
    // at every stitch. Keep the side/top crop small, but remove that fixed bottom band entirely.
    // The browser's fixed bottom frame can sit several pixels above the native window edge; keep
    // enough distance to exclude the whole band from every segment instead of stitching it into
    // the long image repeatedly.
    const SCROLL_CAPTURE_BOTTOM_MARGIN: i32 = 14;
    let scroll_bounds = CaptureRect {
        x: target.bounds.x + SCROLL_CAPTURE_SIDE_MARGIN,
        y: target.bounds.y + SCROLL_CAPTURE_TOP_MARGIN,
        width: target.bounds.width - SCROLL_CAPTURE_SIDE_MARGIN * 2,
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
    let mut previous = stable_capture(scroll_bounds)?;
    let mut segments = vec![(previous.clone(), 0u32)];
    let mut total_height = scroll_bounds.height as u32;
    let maximum_height = (60_000u32)
        .min((80_000_000u64 / scroll_bounds.width as u64) as u32)
        .max(total_height);
    let mut unchanged_steps = 0;

    for _ in 0..180 {
        if total_height >= maximum_height {
            break;
        }
        send_wheel_step(recipient, point, -120);
        let current = stable_capture(scroll_bounds)?;
        if equivalent(&previous, &current) {
            unchanged_steps += 1;
            if unchanged_steps >= 2 {
                break;
            }
            continue;
        }
        unchanged_steps = 0;
        let mut shift = find_vertical_shift(&previous, &current);
        if shift == 0 {
            break;
        }
        shift = shift.min(maximum_height - total_height);
        // Keep a small overlap at every join. With an exact shift this is a no-op, while a
        // one-pixel timing/scroll difference is blended instead of becoming a visible seam.
        let overlap = 8u32
            .min(shift)
            .min(previous.height().saturating_sub(shift));
        segments.push((
            copy_bottom_rows(&current, shift + overlap),
            overlap,
        ));
        total_height += shift;
        previous = current;
    }

    scroll_to_top(target.handle, recipient, point);
    let mut result = RgbaImage::new(scroll_bounds.width as u32, total_height);
    let mut y = 0u32;
    for (segment_index, (segment, overlap)) in segments.into_iter().enumerate() {
        let start_y = if segment_index == 0 {
            0
        } else {
            y.saturating_sub(overlap)
        };
        for row in 0..segment.height() {
            let dst_y = start_y + row;
            if dst_y >= result.height() {
                break;
            }
            for x in 0..segment.width() {
                let next = *segment.get_pixel(x, row);
                if segment_index > 0 && row < overlap {
                    let previous = *result.get_pixel(x, dst_y);
                    let current_weight = row + 1;
                    let previous_weight = overlap + 1 - current_weight;
                    result.put_pixel(
                        x,
                        dst_y,
                        Rgba([
                            ((previous[0] as u32 * previous_weight
                                + next[0] as u32 * current_weight)
                                / (overlap + 1)) as u8,
                            ((previous[1] as u32 * previous_weight
                                + next[1] as u32 * current_weight)
                                / (overlap + 1)) as u8,
                            ((previous[2] as u32 * previous_weight
                                + next[2] as u32 * current_weight)
                                / (overlap + 1)) as u8,
                            ((previous[3] as u32 * previous_weight
                                + next[3] as u32 * current_weight)
                                / (overlap + 1)) as u8,
                        ]),
                    );
                } else {
                    result.put_pixel(x, dst_y, next);
                }
            }
        }
        y = start_y + segment.height();
        if y >= result.height() {
            break;
        }
    }
    Ok(result)
}

#[cfg(not(target_os = "windows"))]
pub fn scrolling_capture(_target: WindowTarget) -> Result<RgbaImage> {
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
fn send_wheel_step(recipient: isize, point: isize, delta: i32) {
    for _ in 0..4 {
        if !send_message(recipient, 0x020A, ((delta << 16) as u32) as usize, point) {
            break;
        }
    }
}

#[cfg(target_os = "windows")]
fn stable_capture(rect: CaptureRect) -> Result<RgbaImage> {
    let mut latest = capture_area(&rect, &None)?;
    for _ in 0..6 {
        thread::sleep(Duration::from_millis(70));
        let next = capture_area(&rect, &None)?;
        if equivalent(&latest, &next) {
            return Ok(next);
        }
        latest = next;
    }
    Ok(latest)
}

fn equivalent(first: &RgbaImage, second: &RgbaImage) -> bool {
    if first.dimensions() != second.dimensions() {
        return false;
    }
    let mut difference = 0i64;
    let mut samples = 0i64;
    for y in (0..first.height()).step_by(20) {
        for x in (0..first.width()).step_by(20) {
            let a = first.get_pixel(x, y);
            let b = second.get_pixel(x, y);
            difference += color_distance(a, b) as i64;
            samples += 1;
        }
    }
    samples == 0 || difference as f64 / ((samples * 3) as f64) < 1.5
}

#[cfg(target_os = "windows")]
fn find_vertical_shift(previous: &RgbaImage, current: &RgbaImage) -> u32 {
    let width = previous.width();
    let height = previous.height();
    let minimum = 12u32.max(height / 60);
    let maximum = minimum.max(height * 2 / 3);
    let mut best_shift = 0;
    let mut best_score = 0.0;
    for shift in (minimum..=maximum).step_by(4) {
        let score = score_shift(previous, current, shift);
        if score > best_score {
            best_score = score;
            best_shift = shift;
        }
    }
    if best_shift == 0 {
        return 0;
    }
    let coarse = best_shift;
    for shift in minimum.max(coarse.saturating_sub(3))..=maximum.min(coarse + 3) {
        let score = score_shift(previous, current, shift);
        if score > best_score {
            best_score = score;
            best_shift = shift;
        }
    }
    let _ = width;
    if best_score >= 0.52 {
        best_shift
    } else {
        0
    }
}

#[cfg(target_os = "windows")]
fn score_shift(previous: &RgbaImage, current: &RgbaImage, shift: u32) -> f64 {
    let width = previous.width();
    let height = previous.height();
    let top_margin = 4u32.max(height / 6);
    let bottom = height
        .saturating_sub(shift)
        .saturating_sub(4u32.max(height / 16));
    let left = 2u32.max(width / 10);
    let right = width.saturating_sub(left);
    let mut informative = 0u32;
    let mut matches = 0u32;
    let mut y = top_margin;
    while y < bottom {
        let mut x = left;
        while x < right {
            let same = previous.get_pixel(x, y);
            if color_distance(same, current.get_pixel(x, y)) >= 18 {
                let old = previous.get_pixel(x, y + shift);
                let neighbor = previous.get_pixel(x.saturating_sub(2), y + shift);
                if color_distance(old, neighbor) >= 24 {
                    informative += 1;
                    if color_distance(old, current.get_pixel(x, y)) <= 36 {
                        matches += 1;
                    }
                }
            }
            x += 14;
        }
        y += 7;
    }
    if informative < 25 {
        0.0
    } else {
        matches as f64 / informative as f64
    }
}

#[cfg(target_os = "windows")]
fn copy_bottom_rows(image: &RgbaImage, rows: u32) -> RgbaImage {
    image::imageops::crop_imm(image, 0, image.height() - rows, image.width(), rows).to_image()
}

fn color_distance(first: &Rgba<u8>, second: &Rgba<u8>) -> u32 {
    (first[0] as i32 - second[0] as i32).unsigned_abs()
        + (first[1] as i32 - second[1] as i32).unsigned_abs()
        + (first[2] as i32 - second[2] as i32).unsigned_abs()
}

/// Lightweight ffmpeg-backed recorder. The worker writes raw RGBA frames and the UI owns the
/// stop/pause handles, giving the compact toolbar the same recording lifecycle as AIMediaWorker.
pub struct ScreenRecorder {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    audio_worker: Option<JoinHandle<()>>,
    audio_error: Arc<Mutex<Option<String>>>,
    ffmpeg_stderr: Option<JoinHandle<String>>,
    child: Option<Arc<Mutex<Child>>>,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WasapiRawAudioFormat {
    Unsigned8,
    Signed16Le,
    Signed24Le,
    Signed32Le,
    Float32Le,
}

#[cfg(target_os = "windows")]
impl WasapiRawAudioFormat {
    fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Unsigned8 => "u8",
            Self::Signed16Le => "s16le",
            Self::Signed24Le => "s24le",
            Self::Signed32Le => "s32le",
            Self::Float32Le => "f32le",
        }
    }

    fn bytes_per_sample(self) -> usize {
        match self {
            Self::Unsigned8 => 1,
            Self::Signed16Le => 2,
            Self::Signed24Le => 3,
            Self::Signed32Le | Self::Float32Le => 4,
        }
    }
}

#[cfg(target_os = "windows")]
fn silent_audio_bytes(frames: usize, format: WasapiAudioFormat) -> Vec<u8> {
    let silence_sample = match format.raw_format {
        // Windows PCM 8-bit samples are unsigned, so the zero-amplitude midpoint is 128.
        WasapiRawAudioFormat::Unsigned8 => 0x80,
        WasapiRawAudioFormat::Signed16Le
        | WasapiRawAudioFormat::Signed24Le
        | WasapiRawAudioFormat::Signed32Le
        | WasapiRawAudioFormat::Float32Le => 0,
    };
    vec![silence_sample; frames.saturating_mul(format.block_align)]
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WasapiAudioFormat {
    sample_rate: u32,
    channels: u16,
    block_align: usize,
    raw_format: WasapiRawAudioFormat,
}

#[cfg(target_os = "windows")]
fn parse_wasapi_audio_format(
    format_ptr: *const windows::Win32::Media::Audio::WAVEFORMATEX,
) -> Result<WasapiAudioFormat> {
    use windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;

    if format_ptr.is_null() {
        anyhow::bail!("Windows returned an empty WASAPI mix format");
    }

    let format = unsafe { std::ptr::read_unaligned(format_ptr) };
    let format_tag = format.wFormatTag;
    let bits_per_sample = format.wBitsPerSample;
    let (is_pcm, is_float) = match format_tag {
        1 => (true, false),     // WAVE_FORMAT_PCM
        3 => (false, true),     // WAVE_FORMAT_IEEE_FLOAT
        0xfffe => {
            // WAVE_FORMAT_EXTENSIBLE stores the real PCM/float tag in SubFormat.Data1.
            if format.cbSize < 22 {
                (false, false)
            } else {
                let extensible = unsafe {
                    std::ptr::read_unaligned(format_ptr as *const WAVEFORMATEXTENSIBLE)
                };
                match extensible.SubFormat.data1 {
                    1 => (true, false),
                    3 => (false, true),
                    _ => (false, false),
                }
            }
        }
        _ => (false, false),
    };
    if !is_pcm && !is_float {
        anyhow::bail!(
            "Unsupported Windows audio format tag: {}",
            format_tag
        );
    }
    if format.nChannels == 0 || format.nSamplesPerSec == 0 {
        anyhow::bail!("Windows returned an invalid WASAPI channel or sample rate");
    }

    let raw_format = if is_float {
        if bits_per_sample != 32 {
            anyhow::bail!(
                "Unsupported WASAPI float depth: {} bits",
                bits_per_sample
            );
        }
        WasapiRawAudioFormat::Float32Le
    } else {
        match bits_per_sample {
            8 => WasapiRawAudioFormat::Unsigned8,
            16 => WasapiRawAudioFormat::Signed16Le,
            24 => WasapiRawAudioFormat::Signed24Le,
            32 => WasapiRawAudioFormat::Signed32Le,
            bits => anyhow::bail!("Unsupported WASAPI PCM depth: {bits} bits"),
        }
    };
    let expected_block_align = format.nChannels as usize * raw_format.bytes_per_sample();
    let block_align = format.nBlockAlign as usize;
    if block_align != expected_block_align {
        anyhow::bail!(
            "Unsupported WASAPI block alignment: {block_align} (expected {expected_block_align})"
        );
    }

    Ok(WasapiAudioFormat {
        sample_rate: format.nSamplesPerSec,
        channels: format.nChannels,
        block_align,
        raw_format,
    })
}

#[cfg(target_os = "windows")]
fn probe_wasapi_loopback_format() -> Result<WasapiAudioFormat> {
    // COM apartment state belongs to the calling thread. Probe on a short-lived MTA so this
    // remains safe even when Slint/Winit has initialized the UI thread differently.
    thread::spawn(probe_wasapi_loopback_format_on_mta)
        .join()
        .map_err(|_| anyhow::anyhow!("WASAPI format probe thread panicked"))?
}

#[cfg(target_os = "windows")]
fn probe_wasapi_loopback_format_on_mta() -> Result<WasapiAudioFormat> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
        CLSCTX_ALL,
    };

    let init_result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if init_result.is_err() {
        anyhow::bail!("Failed to initialize COM for WASAPI: {init_result:?}");
    }

    let result = (|| {
        let enumerator: IMMDeviceEnumerator = unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        }
        .context("Failed to create the Windows audio device enumerator")?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .context("Failed to find the default Windows playback device")?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .context("Failed to activate the default Windows playback device")?;
        let format_ptr = unsafe { client.GetMixFormat() }
            .context("Failed to read the Windows playback mix format")?;
        let result = parse_wasapi_audio_format(format_ptr);
        unsafe {
            CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
        }
        result
    })();

    unsafe { CoUninitialize() };
    result
}

#[cfg(target_os = "windows")]
fn accept_audio_stream(listener: TcpListener, stop: &AtomicBool) -> Result<Option<TcpStream>> {
    listener
        .set_nonblocking(true)
        .context("Failed to configure the Windows audio loopback pipe")?;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_write_timeout(Some(Duration::from_millis(250)))
                    .context("Failed to configure the Windows audio loopback stream")?;
                return Ok(Some(stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error).context("Failed to accept the Windows audio stream"),
        }
    }
}

#[cfg(target_os = "windows")]
fn capture_wasapi_loopback(
    listener: TcpListener,
    expected_format: WasapiAudioFormat,
    stop: Arc<AtomicBool>,
    error_slot: Arc<Mutex<Option<String>>>,
) {
    let result = capture_wasapi_loopback_on_mta(listener, expected_format, &stop);
    if let Err(error) = result {
        log::warn!("Windows audio loopback stopped: {error:?}");
        if let Ok(mut error_slot) = error_slot.lock() {
            *error_slot = Some(format!("{error:#}"));
        }
        stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(target_os = "windows")]
fn capture_wasapi_loopback_on_mta(
    listener: TcpListener,
    expected_format: WasapiAudioFormat,
    stop: &AtomicBool,
) -> Result<()> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
        CLSCTX_ALL,
    };

    let init_result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if init_result.is_err() {
        anyhow::bail!("Failed to initialize COM for Windows audio capture: {init_result:?}");
    }

    let result = (|| {
        let enumerator: IMMDeviceEnumerator = unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        }
        .context("Failed to create the Windows audio device enumerator")?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .context("Failed to open the default Windows playback device")?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .context("Failed to activate Windows audio loopback")?;
        let format_ptr = unsafe { client.GetMixFormat() }
            .context("Failed to read the Windows playback mix format")?;
        let actual_format = parse_wasapi_audio_format(format_ptr);
        let actual_format = match actual_format {
            Ok(format) => format,
            Err(error) => {
                unsafe {
                    CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
                }
                return Err(error);
            }
        };
        if actual_format != expected_format {
            unsafe {
                CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
            }
            anyhow::bail!("The default Windows playback format changed while recording started");
        }

        let initialize_result = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                10_000_000,
                0,
                format_ptr,
                None,
            )
        };
        unsafe {
            CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
        }
        initialize_result.context("Failed to initialize Windows audio loopback")?;

        let Some(mut stream) = accept_audio_stream(listener, stop)? else {
            return Ok(());
        };
        let capture_client: IAudioCaptureClient = unsafe { client.GetService() }
            .context("Failed to access the Windows audio capture buffer")?;
        unsafe { client.Start() }.context("Failed to start Windows audio loopback")?;

        let capture_result = (|| {
            let silent_frames = (expected_format.sample_rate as usize / 100).max(1);
            let silent_packet = silent_audio_bytes(silent_frames, expected_format);
            let silent_duration = Duration::from_secs_f64(
                silent_frames as f64 / expected_format.sample_rate as f64,
            );
            let mut next_silent_packet = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let packet_frames = unsafe { capture_client.GetNextPacketSize() }
                    .context("Failed to read the Windows audio packet size")?;
                if packet_frames == 0 {
                    // Some Windows output devices expose no loopback packet while silent. Keep
                    // the raw audio input alive with real-time paced silence; otherwise FFmpeg's
                    // `-shortest` sees an empty audio stream and discards the video as well.
                    let now = Instant::now();
                    if now >= next_silent_packet {
                        if stream.write_all(&silent_packet).is_err() {
                            break;
                        }
                        next_silent_packet = now + silent_duration;
                    } else {
                        thread::sleep(
                            (next_silent_packet - now).min(Duration::from_millis(5)),
                        );
                    }
                    continue;
                }

                let mut data_ptr = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                unsafe {
                    capture_client
                        .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
                }
                .context("Failed to read the Windows audio capture buffer")?;

                let byte_len = frames as usize * expected_format.block_align;
                let bytes = if flags & 2 != 0 || data_ptr.is_null() {
                    // AUDCLNT_BUFFERFLAGS_SILENT: the loopback engine still advances the
                    // timeline, so write an equally sized silent packet instead of dropping it.
                    silent_audio_bytes(frames as usize, expected_format)
                } else {
                    unsafe { std::slice::from_raw_parts(data_ptr as *const u8, byte_len) }
                        .to_vec()
                };
                unsafe { capture_client.ReleaseBuffer(frames) }
                    .context("Failed to release the Windows audio capture buffer")?;

                if stream.write_all(&bytes).is_err() {
                    break;
                }
                next_silent_packet = Instant::now() +
                    Duration::from_secs_f64(frames as f64 / expected_format.sample_rate as f64);
            }
            Ok::<(), anyhow::Error>(())
        })();

        let stop_result = unsafe { client.Stop() };
        stop_result.context("Failed to stop Windows audio loopback")?;
        capture_result
    })();

    unsafe { CoUninitialize() };
    result
}

impl ScreenRecorder {
    pub fn start(rect: CaptureRect, path: PathBuf, fps: u32) -> Result<Self> {
        if !rect.valid() {
            anyhow::bail!("The recording area is too small");
        }
        let width = rect.width & !1;
        let height = rect.height & !1;
        if width < 2 || height < 2 {
            anyhow::bail!("The recording area must be at least 2x2");
        }
        let rect = CaptureRect {
            width,
            height,
            ..rect
        };
        let fps = fps.clamp(1, 60);

        let video_size = format!("{}x{}", width, height);
        #[cfg(target_os = "windows")]
        let audio_format = probe_wasapi_loopback_format()?;
        #[cfg(target_os = "windows")]
        let audio_listener = TcpListener::bind(("127.0.0.1", 0))
            .context("Failed to create the Windows audio loopback stream")?;
        #[cfg(target_os = "windows")]
        let audio_url = format!(
            "tcp://127.0.0.1:{}",
            audio_listener
                .local_addr()
                .context("Failed to get the Windows audio loopback address")?
                .port()
        );
        let framerate = fps.to_string();
        let mut ffmpeg = Command::new("ffmpeg");
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;

            // CREATE_NO_WINDOW prevents the console-hosted ffmpeg process from opening a
            // terminal window while recording from the toolbar.
            ffmpeg.creation_flags(0x0800_0000);
        }
        ffmpeg.args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            video_size.as_str(),
            "-framerate",
            framerate.as_str(),
            "-i",
            "-",
        ]);
        #[cfg(target_os = "windows")]
        {
            // FFmpeg builds without a WASAPI demuxer are still common on Windows. Feed it the
            // native default-render loopback through localhost, so the recording contains
            // Windows playback audio instead of whichever webcam microphone DirectShow lists.
            let audio_sample_rate = audio_format.sample_rate.to_string();
            let audio_channels = audio_format.channels.to_string();
            ffmpeg.args([
                "-thread_queue_size",
                "512",
                "-f",
                audio_format.raw_format.ffmpeg_name(),
                "-ar",
                audio_sample_rate.as_str(),
                "-ac",
                audio_channels.as_str(),
                "-i",
            ]);
            ffmpeg.arg(audio_url);
            ffmpeg.args([
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                // Downmix multichannel output cleanly and always write a standard stereo AAC
                // track at a stable rate. The resampler also keeps the two inputs synchronized.
                "-af",
                "aresample=async=1:first_pts=0",
                "-ar",
                "48000",
                "-ac",
                "2",
                "-channel_layout",
                "stereo",
                "-c:a",
                "aac",
                "-profile:a",
                "aac_low",
                "-b:a",
                "192k",
                "-shortest",
                "-pix_fmt",
                "yuv420p",
            ]);
        }
        #[cfg(not(target_os = "windows"))]
        ffmpeg.args([
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ]);
        let mut child = ffmpeg
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("FFmpeg was not found. Install ffmpeg and make sure it is on PATH.")?;
        let ffmpeg_stderr = child.stderr.take().map(|mut stderr| {
            thread::spawn(move || {
                let mut output = Vec::new();
                let _ = stderr.read_to_end(&mut output);
                String::from_utf8_lossy(&output).trim().to_string()
            })
        });
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Failed to open FFmpeg input");
            }
        };
        let child = Arc::new(Mutex::new(child));
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let audio_error = Arc::new(Mutex::new(None));

        #[cfg(target_os = "windows")]
        let audio_worker = {
            let audio_stop = stop.clone();
            let audio_error = audio_error.clone();
            Some(thread::spawn(move || {
                capture_wasapi_loopback(audio_listener, audio_format, audio_stop, audio_error)
            }))
        };
        #[cfg(not(target_os = "windows"))]
        let audio_worker = None;

        let worker_stop = stop.clone();
        let worker_paused = paused.clone();
        let interval = Duration::from_secs_f64(1.0 / fps as f64);
        let worker = thread::spawn(move || {
            record_frames(&mut stdin, rect, interval, worker_stop, worker_paused)
        });

        Ok(Self {
            stop,
            paused,
            worker: Some(worker),
            audio_worker,
            audio_error,
            ffmpeg_stderr,
            child: Some(child),
        })
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);

        // A blocked pipe write can otherwise keep a worker alive forever if FFmpeg exited early
        // (for example after an audio-device change). Give the normal graceful close a short
        // window, then terminate only the recorder child as a last resort so closing the app
        // cannot leave an orphaned FFmpeg process behind.
        let child = self.child.take();
        let stop_watchdog = Arc::new(AtomicBool::new(false));
        let watchdog = child.as_ref().map(|child| {
            let child = child.clone();
            let stop_watchdog = stop_watchdog.clone();
            thread::spawn(move || {
                for _ in 0..20 {
                    if stop_watchdog.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                if !stop_watchdog.load(Ordering::Relaxed) {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                }
            })
        });
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(audio_worker) = self.audio_worker.take() {
            let _ = audio_worker.join();
        }
        stop_watchdog.store(true, Ordering::Relaxed);
        if let Some(watchdog) = watchdog {
            let _ = watchdog.join();
        }
        let audio_error = self
            .audio_error
            .lock()
            .ok()
            .and_then(|mut error| error.take());
        let mut ffmpeg_error = None;
        if let Some(child) = child {
            let status = child
                .lock()
                .map_err(|_| anyhow::anyhow!("FFmpeg process state was poisoned"))?
                .wait()
                .context("Failed to close FFmpeg")?;
            ffmpeg_error = self
                .ffmpeg_stderr
                .take()
                .and_then(|worker| worker.join().ok())
                .filter(|error| !error.is_empty());
            if !status.success() {
                if let Some(error) = ffmpeg_error {
                    anyhow::bail!("FFmpeg could not finish the recording: {error}");
                }
                anyhow::bail!("FFmpeg could not finish the recording");
            }
        }
        if let Some(error) = ffmpeg_error {
            log::warn!("FFmpeg reported a recorder diagnostic: {error}");
        }
        if let Some(error) = audio_error {
            anyhow::bail!("Windows playback audio could not be recorded: {error}");
        }
        Ok(())
    }
}

impl Drop for ScreenRecorder {
    fn drop(&mut self) {
        // Keep an unexpected drop from orphaning FFmpeg. The normal UI stop path still performs
        // the graceful flush in `stop`; this is only the emergency cleanup path.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(child) = self.child.as_ref() {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }
    }
}

fn record_frames(
    stdin: &mut ChildStdin,
    rect: CaptureRect,
    interval: Duration,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        let frame_started = Instant::now();
        if !paused.load(Ordering::Relaxed) {
            match capture_area(&rect, &None) {
                Ok(image) => {
                    if stdin.write_all(image.as_raw()).is_err() {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                Err(error) => {
                    log::warn!("Recording frame capture failed: {error:?}");
                }
            }
        }
        let elapsed = frame_started.elapsed();
        if elapsed < interval {
            thread::sleep(interval - elapsed);
        }
    }
}

/// Comparison logic to check if the screen changed enough to trigger the original VLM worker.
pub fn is_changed(prev: &Option<RgbaImage>, curr: &RgbaImage, _threshold: f32) -> bool {
    let prev_img = match prev {
        Some(p) => p,
        None => return true,
    };
    if prev_img.dimensions() != curr.dimensions() {
        return true;
    }

    let mut diff_sum = 0u64;
    let mut total_pixels = 0u64;
    let (width, height) = prev_img.dimensions();
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let p = prev_img.get_pixel(x, y);
            let c = curr.get_pixel(x, y);
            let diff = (p[0] as i32 - c[0] as i32).unsigned_abs()
                + (p[1] as i32 - c[1] as i32).unsigned_abs()
                + (p[2] as i32 - c[2] as i32).unsigned_abs();
            if diff > 80 {
                diff_sum += 1;
            }
            total_pixels += 1;
        }
    }
    total_pixels != 0 && diff_sum as f32 / total_pixels as f32 >= 0.01
}
