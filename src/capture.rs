use anyhow::{Context, Result};
use image::{GenericImageView, Rgba, RgbaImage};
use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use xcap::Monitor;

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
    capture_area(&target.bounds, &None)
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

pub fn unique_output_path_in(
    prefix: &str,
    extension: &str,
    configured_folder: Option<&str>,
) -> Result<PathBuf> {
    let directory = configured_output_directory(configured_folder);
    std::fs::create_dir_all(&directory).context("Failed to create capture directory")?;
    for index in 0.. {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!(" ({index})")
        };
        let path = directory.join(format!("{prefix}{suffix}.{extension}"));
        if !path.exists() {
            return Ok(path);
        }
    }
    unreachable!()
}

pub fn save_png_and_copy_to(
    image: &RgbaImage,
    prefix: &str,
    configured_folder: Option<&str>,
) -> Result<PathBuf> {
    let path = unique_output_path_in(prefix, "png", configured_folder)?;
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

    let center_x = target.bounds.x + target.bounds.width / 2;
    let center_y = target.bounds.y + target.bounds.height / 2;
    let recipient = native_window_at_point(center_x, center_y).unwrap_or(target.handle);
    let point = make_point_parameter(center_x, center_y);

    scroll_to_top(target.handle, recipient, point);
    let mut previous = stable_capture(target.bounds)?;
    let mut segments = vec![previous.clone()];
    let mut total_height = target.bounds.height as u32;
    let maximum_height = (60_000u32)
        .min((80_000_000u64 / target.bounds.width as u64) as u32)
        .max(total_height);
    let mut unchanged_steps = 0;

    for _ in 0..180 {
        if total_height >= maximum_height {
            break;
        }
        send_wheel_step(recipient, point, -120);
        let current = stable_capture(target.bounds)?;
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
        segments.push(copy_bottom_rows(&current, shift));
        total_height += shift;
        previous = current;
    }

    scroll_to_top(target.handle, recipient, point);
    let mut result = RgbaImage::new(target.bounds.width as u32, total_height);
    let mut y = 0u32;
    for segment in segments {
        for row in 0..segment.height() {
            let dst_y = y + row;
            if dst_y >= result.height() {
                break;
            }
            for x in 0..segment.width() {
                result.put_pixel(x, dst_y, *segment.get_pixel(x, row));
            }
        }
        y += segment.height();
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
    child: Option<Child>,
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
        let framerate = fps.to_string();
        let mut child = Command::new("ffmpeg")
            .args([
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
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("FFmpeg was not found. Install ffmpeg and make sure it is on PATH.")?;
        let mut stdin = child.stdin.take().context("Failed to open FFmpeg input")?;
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
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
            child: Some(child),
        })
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(mut child) = self.child.take() {
            let status = child.wait().context("Failed to close FFmpeg")?;
            if !status.success() {
                anyhow::bail!("FFmpeg could not finish the recording");
            }
        }
        Ok(())
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
