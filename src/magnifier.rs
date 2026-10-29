//! Live window preview with original-resolution capture and one-pass resampling.
//! DWM remains available while high-quality frames are pending or unsupported.
use crate::{
    capture, state::AppState, win_utils, CaptureToolbarWindow, MagnifierWindow, MainWindow,
    RecordingBorderWindow, TextboxWindow,
};
use anyhow::{Context, Result};
use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{
        Dwm::{
            DwmFlush, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
            DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY,
            DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
        },
        Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    },
    UI::WindowsAndMessaging::{GetWindowTextW, IsIconic, IsWindow},
};

mod input;
mod quality;
mod source_window;
mod window_bounds;
pub(crate) use source_window::{restore_all as restore_source_windows, run_restore_watchdog};

const HEADER: f64 = 42.0;
const INSET: f64 = 6.0;
const RESTORE_SOURCE_NOTICE: &str = "Restore the selected window to resume the preview";
const PARTLY_VISIBLE_NOTICE: &str = "Source window is partly visible to keep the preview live";

fn snap_zoom_percent(percent: f32) -> f32 {
    if !percent.is_finite() {
        return 200.0;
    }
    percent.round().clamp(50.0, 400.0)
}

fn maximum_zoom_percent(source: (i32, i32), work: (i32, i32), dpi: f64, frame: (i32, i32)) -> f32 {
    if source.0 <= 0 || source.1 <= 0 {
        return 400.0;
    }
    let width = (work.0 - frame.0) as f64 - (INSET * dpi).ceil() * 2.0;
    let height = (work.1 - frame.1) as f64 - (HEADER * dpi).ceil() - (INSET * dpi).ceil();
    let percent = (width.max(1.0) / source.0 as f64).min(height.max(1.0) / source.1 as f64) * 100.0;
    percent.floor().clamp(50.0, 400.0) as f32
}

fn bounded_zoom_percent(percent: f32, maximum: f32) -> f32 {
    snap_zoom_percent(percent).min(maximum.max(50.0).floor())
}

fn quality_presentation(
    rendered: Option<quality::FrameKey>,
    requested: quality::FrameKey,
    failed: bool,
) -> (bool, bool) {
    let visible = rendered.is_some() && !failed;
    (visible, visible && rendered == Some(requested))
}

struct Thumbnail {
    handle: isize,
    source: HWND,
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        unsafe {
            let _ = DwmUnregisterThumbnail(self.handle);
        }
    }
}

/// Owns both the Slint window and the native thumbnail until the application exits.
pub(crate) struct WindowMagnifier {
    input: RefCell<Option<input::InputBridge>>,
    input_attempted: Cell<bool>,
    movement_guard: RefCell<Option<window_bounds::MovementGuard>>,
    window: MagnifierWindow,
    thumbnail: RefCell<Option<Thumbnail>>,
    quality: RefCell<Option<quality::QualitySession>>,
    quality_frame_key: Cell<Option<quality::FrameKey>>,
    quality_pixels: RefCell<Option<Arc<image::RgbaImage>>>,
    thumbnail_properties: Cell<Option<(i32, i32, i32, i32, bool)>>,
    last_stack_check: Cell<Instant>,
    quality_source_size: Cell<Option<(u32, u32)>>,
    zoom: Cell<f64>,
    pending_zoom_resize: Cell<bool>,
    owns_recording: Cell<bool>,
    toolbar: slint::Weak<CaptureToolbarWindow>,
    capture_rect: Cell<Option<capture::CaptureRect>>,
    timer: slint::Timer,
    state: Arc<Mutex<AppState>>,
}

fn hwnd(window: &MagnifierWindow) -> Result<HWND> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window
        .window()
        .with_winit_window(|native| {
            let handle = native.window_handle().ok()?;
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return None;
            };
            Some(HWND(handle.hwnd.get() as _))
        })
        .flatten()
        .context("The magnifier window is not ready")
}

fn work_area(owner: HWND) -> Result<capture::CaptureRect> {
    unsafe {
        let monitor = MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        anyhow::ensure!(
            GetMonitorInfoW(monitor, &mut info).as_bool(),
            "Unable to read monitor work area"
        );
        Ok(capture::CaptureRect {
            x: info.rcWork.left,
            y: info.rcWork.top,
            width: info.rcWork.right - info.rcWork.left,
            height: info.rcWork.bottom - info.rcWork.top,
        })
    }
}

/// Size the entire viewer, including its controls, within the monitor work area.
fn fitted_window_size(source: (i32, i32), zoom: f64, dpi: f64, work: (i32, i32)) -> (u32, u32) {
    // Use the same per-edge rounding as preview_rect at fractional DPI scales.
    let horizontal = (INSET * dpi).ceil() * 2.0;
    let vertical = (HEADER * dpi).ceil() + (INSET * dpi).ceil();
    let available_w = (work.0 as f64 - horizontal).max(1.0);
    let available_h = (work.1 as f64 - vertical).max(1.0);
    let scale = zoom
        .min(available_w / source.0.max(1) as f64)
        .min(available_h / source.1.max(1) as f64);
    let width = (source.0.max(1) as f64 * scale + horizontal)
        .ceil()
        .max((480.0 * dpi).min(work.0 as f64))
        .min(work.0 as f64);
    let height = (source.1.max(1) as f64 * scale + vertical)
        .ceil()
        .max((180.0 * dpi).min(work.1 as f64))
        .min(work.1 as f64);
    (width.max(1.0) as u32, height.max(1.0) as u32)
}

/// Letterbox the whole source; resizing never crops or distorts it. Zooms are
/// measured in physical pixels, so moving between monitors keeps their meaning.
fn preview_rect(
    source: (i32, i32),
    client: (i32, i32),
    dpi: f64,
    zoom: f64,
) -> Option<(capture::CaptureRect, f64)> {
    let left = (INSET * dpi).ceil() as i32;
    let top = (HEADER * dpi).ceil() as i32;
    let right = client.0 - left;
    let bottom = client.1 - left;
    if source.0 <= 0 || source.1 <= 0 || right <= left || bottom <= top {
        return None;
    }
    let scale = zoom
        .min((right - left) as f64 / source.0 as f64)
        .min((bottom - top) as f64 / source.1 as f64);
    let width = (source.0 as f64 * scale).floor().max(1.0) as i32;
    let height = (source.1 as f64 * scale).floor().max(1.0) as i32;
    Some((
        capture::CaptureRect {
            x: left + (right - left - width) / 2,
            y: top + (bottom - top - height) / 2,
            width,
            height,
        },
        scale,
    ))
}

impl WindowMagnifier {
    pub(crate) fn new(
        toolbar: &CaptureToolbarWindow,
        state: Arc<Mutex<AppState>>,
    ) -> Result<Rc<Self>> {
        let this = Rc::new(Self {
            input: RefCell::new(None),
            input_attempted: Cell::new(false),
            movement_guard: RefCell::new(None),
            window: MagnifierWindow::new()?,
            thumbnail: RefCell::new(None),
            quality: RefCell::new(None),
            quality_frame_key: Cell::new(None),
            quality_pixels: RefCell::new(None),
            thumbnail_properties: Cell::new(None),
            last_stack_check: Cell::new(Instant::now() - Duration::from_secs(1)),
            quality_source_size: Cell::new(None),
            zoom: Cell::new(2.0),
            pending_zoom_resize: Cell::new(false),
            owns_recording: Cell::new(false),
            toolbar: toolbar.as_weak(),
            capture_rect: Cell::new(None),
            timer: slint::Timer::default(),
            state,
        });
        let toolbar_status = toolbar.as_weak();
        this.window.on_status_changed(move |message| {
            if let Some(toolbar) = toolbar_status.upgrade() {
                if message.is_empty()
                    && ![RESTORE_SOURCE_NOTICE, PARTLY_VISIBLE_NOTICE]
                        .contains(&toolbar.get_status_text().as_str())
                {
                    return;
                }
                crate::capture_workflow::set_capture_toolbar_status(&toolbar, message.to_string());
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.on_close_clicked(move || {
            let weak = weak.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(this) = weak.upgrade() {
                    this.close();
                }
            });
        });
        let weak = Rc::downgrade(&this);
        this.window.window().on_close_requested(move || {
            let weak = weak.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(this) = weak.upgrade() {
                    this.close();
                }
            });
            slint::CloseRequestResponse::KeepWindowShown
        });
        let weak = Rc::downgrade(&this);
        this.window.on_drag_requested(move || {
            let weak = weak.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(this) = weak.upgrade() {
                    if !this.window.get_recording() {
                        if let Ok(owner) = hwnd(&this.window) {
                            win_utils::begin_window_drag(owner);
                        }
                    }
                }
            });
        });
        let weak = Rc::downgrade(&this);
        this.window.on_zoom_changed(move |percent| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let percent = bounded_zoom_percent(percent, this.window.get_max_zoom_percent());
            this.window.set_zoom_percent(percent);
            this.zoom.set(percent as f64 / 100.0);
            this.pending_zoom_resize.set(true);
            this.window.set_status_text("".into());
        });
        let weak = Rc::downgrade(&this);
        let toolbar = toolbar.as_weak();
        this.window.on_capture_clicked(move || {
            let weak = weak.clone();
            let toolbar = toolbar.clone();
            slint::Timer::single_shot(Duration::from_millis(16), move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if this.window.get_capture_busy() || !this.window.get_source_available() {
                    return;
                }
                if let Err(error) = this.update() {
                    this.window
                        .set_status_text(format!("Error: {error}").into());
                    return;
                }
                if this.window.get_quality_active() && !this.window.get_quality_exact() {
                    let weak = weak.clone();
                    slint::Timer::single_shot(Duration::from_millis(16), move || {
                        if let Some(this) = weak.upgrade() {
                            this.window.invoke_capture_clicked();
                        }
                    });
                    return;
                }
                let Some(rect) = this.capture_rect.get() else {
                    return;
                };
                // Save filtered pixels directly, even before Slint's next paint.
                // Standard previews use the displayed thumbnail without controls.
                let filtered = if this.window.get_quality_active() {
                    this.quality_pixels.borrow().clone()
                } else {
                    None
                };
                if filtered.is_none() {
                    unsafe {
                        let _ = DwmFlush();
                    }
                }
                let folder = this.state.lock().unwrap().capture_folder.clone();
                this.window.set_capture_busy(true);
                this.window.set_status_text("Saving capture...".into());
                let task_weak = weak.clone();
                if let Err(error) = slint::spawn_local(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let image = match filtered {
                            Some(image) => image,
                            None => Arc::new(capture::capture_area(&rect, &None)?),
                        };
                        capture::save_png_and_copy_to(&image, Some(&folder))
                    })
                    .await;
                    let message = match result {
                        Ok(Ok(path)) => path.status("Saved magnified window"),
                        Ok(Err(error)) => format!("Capture failed: {error}"),
                        Err(error) => format!("Capture worker stopped: {error}"),
                    };
                    if let Some(this) = task_weak.upgrade() {
                        this.window.set_capture_busy(false);
                        this.window.set_status_text(message.clone().into());
                    }
                    if let Some(toolbar) = toolbar.upgrade() {
                        crate::capture_workflow::set_capture_toolbar_status(&toolbar, message);
                    }
                }) {
                    this.window.set_capture_busy(false);
                    this.window
                        .set_status_text(format!("Capture failed: {error}").into());
                }
            });
        });
        Ok(this)
    }

    pub(crate) fn register_recording(
        self: &Rc<Self>,
        main: &MainWindow,
        border: &RecordingBorderWindow,
        textbox: &TextboxWindow,
        recorder: Arc<Mutex<Option<capture::ScreenRecorder>>>,
        http: reqwest::Client,
    ) {
        let weak = Rc::downgrade(self);
        let main = main.as_weak();
        let border = border.as_weak();
        let textbox = textbox.as_weak();
        self.window.on_record_clicked(move || {
            let weak = weak.clone();
            let main = main.clone();
            let border = border.clone();
            let textbox = textbox.clone();
            let recorder = recorder.clone();
            let http = http.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if this.owns_recording.get() {
                    this.stop_own_recording();
                    return;
                }
                let Some(toolbar) = this.toolbar.upgrade() else {
                    return;
                };
                if toolbar.get_recording() {
                    return;
                }
                if let Err(error) = this.update() {
                    this.window
                        .set_status_text(format!("Recording failed: {error}").into());
                    return;
                }
                let Some(rect) = this.capture_rect.get() else {
                    return;
                };
                let state = this.state.clone();
                let toolbar = toolbar.as_weak();
                let task_weak = weak.clone();
                if let Err(error) = slint::spawn_local(async move {
                    crate::capture_workflow::run_toolbar_action(
                        crate::state::SelectionPurpose::Record,
                        rect,
                        None,
                        None,
                        false,
                        border,
                        main,
                        toolbar.clone(),
                        state,
                        recorder,
                        http,
                        textbox,
                    )
                    .await;
                    if let Some(this) = task_weak.upgrade() {
                        if let Some(toolbar) = toolbar.upgrade() {
                            this.owns_recording.set(toolbar.get_recording());
                            this.window.set_status_text(if toolbar.get_recording() {
                                "Recording — press Stop to save".into()
                            } else {
                                toolbar.get_status_text()
                            });
                        }
                        let _ = this.update();
                        if this.input.borrow().is_some() {
                            if let Ok(viewer) = hwnd(&this.window) {
                                let _ = win_utils::set_preview_topmost(viewer);
                            }
                        } else {
                            this.window
                                .window()
                                .with_winit_window(|native| native.focus_window());
                        }
                    }
                }) {
                    this.window
                        .set_status_text(format!("Recording failed: {error}").into());
                }
            });
        });
    }

    fn stop_own_recording(&self) {
        if self.owns_recording.replace(false) {
            if let Some(toolbar) = self.toolbar.upgrade() {
                // The shared stop callback finalizes MP4 before returning.
                toolbar.invoke_stop_recording_clicked();
                self.window.set_status_text(toolbar.get_status_text());
            }
            self.window.set_recording(false);
            self.window
                .window()
                .with_winit_window(|native| native.set_resizable(true));
        }
    }

    pub(crate) fn open(self: &Rc<Self>, target: capture::WindowTarget, dark: bool) -> Result<()> {
        self.stop_own_recording();
        self.timer.stop();
        self.input.borrow_mut().take();
        self.input_attempted.set(false);
        self.thumbnail.borrow_mut().take();
        self.reset_quality();
        self.capture_rect.set(None);
        self.window.set_dark_theme(dark);
        self.window.set_status_text("".into());
        self.window.set_source_available(false);
        self.window.show()?;
        input::set_owner(Rc::downgrade(self));
        let result = (|| -> Result<()> {
            let owner = hwnd(&self.window)?;
            win_utils::set_title_bar_theme(owner, dark);
            win_utils::set_tool_window(owner, true);
            win_utils::disable_window_transitions(owner);
            let _ = win_utils::set_preview_topmost(owner);
            if self.movement_guard.borrow().as_ref().map(|guard| guard.0) != Some(owner) {
                *self.movement_guard.borrow_mut() = Some(window_bounds::MovementGuard::new(owner)?);
            }
            let source = HWND(target.handle as _);
            let mut title = [0u16; 512];
            let length = unsafe { GetWindowTextW(source, &mut title) } as usize;
            self.window.set_source_title(if length == 0 {
                "Window magnifier".into()
            } else {
                String::from_utf16_lossy(&title[..length]).into()
            });
            let handle = unsafe { DwmRegisterThumbnail(owner, source)? };
            *self.thumbnail.borrow_mut() = Some(Thumbnail { handle, source });
            match quality::QualitySession::start(target.handle) {
                Ok(session) => *self.quality.borrow_mut() = Some(session),
                Err(error) => log::warn!("Unable to start high-quality preview: {error:#}"),
            }
            self.resize_to_zoom()?;
            self.update()?;
            Ok(())
        })();
        if result.is_err() {
            self.close();
            return result;
        }
        let weak = Rc::downgrade(self);
        // Capture/resampling runs on its worker. Publish only the latest matching
        // frame and geometry on the UI thread, including while the slider is held.
        self.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(8),
            move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if let Err(error) = this.update() {
                    this.input.borrow_mut().take();
                    this.stop_own_recording();
                    this.thumbnail.borrow_mut().take();
                    this.reset_quality();
                    this.capture_rect.set(None);
                    this.window.set_source_available(false);
                    this.window
                        .set_status_text(format!("Preview unavailable: {error}").into());
                    this.timer.stop();
                }
            },
        );
        Ok(())
    }

    fn close(&self) {
        if let Some(input) = self.input.borrow_mut().as_mut() {
            input.suspend_mouse();
        }
        self.stop_own_recording();
        self.timer.stop();
        self.input.borrow_mut().take();
        self.thumbnail.borrow_mut().take();
        self.reset_quality();
        self.capture_rect.set(None);
        self.window.set_source_available(false);
        let _ = self.window.hide();
    }

    fn resize_to_zoom(&self) -> Result<()> {
        let thumbnail = self.thumbnail.borrow();
        let Some(thumbnail) = thumbnail.as_ref() else {
            return Ok(());
        };
        let size = unsafe { DwmQueryThumbnailSourceSize(thumbnail.handle)? };
        let size = self
            .quality
            .borrow()
            .as_ref()
            .filter(|quality| !quality.failed())
            .and_then(|quality| quality.source_size())
            .map(|(width, height)| (width as i32, height as i32))
            .unwrap_or((size.cx, size.cy));
        let work = work_area(hwnd(&self.window)?)?;
        let dpi = self.window.window().scale_factor() as f64;
        let (frame_w, frame_h) = self.frame_extent();
        let (width, height) = fitted_window_size(
            size,
            self.zoom.get(),
            dpi,
            (
                (work.width - frame_w).max(1),
                (work.height - frame_h).max(1),
            ),
        );
        let needs_resize = self
            .window
            .window()
            .with_winit_window(|native| {
                let size = native.inner_size();
                (size.width, size.height) != (width, height)
            })
            .unwrap_or(true);
        if needs_resize {
            self.window
                .window()
                .set_size(slint::PhysicalSize::new(width, height));
        }
        self.window.window().with_winit_window(|native| {
            let position = native
                .outer_position()
                .unwrap_or(winit::dpi::PhysicalPosition::new(work.x, work.y));
            let bounded_position = winit::dpi::PhysicalPosition::new(
                position.x.clamp(
                    work.x,
                    (work.x + work.width - width as i32 - frame_w).max(work.x),
                ),
                position.y.clamp(
                    work.y,
                    (work.y + work.height - height as i32 - frame_h).max(work.y),
                ),
            );
            if position != bounded_position {
                native.set_outer_position(bounded_position);
            }
        });
        Ok(())
    }

    fn update(&self) -> Result<()> {
        if input::take_close_requested() {
            self.close();
            return Ok(());
        }
        // Slint can raise the viewer on activation/resize, even before native
        // source interaction starts or while falling back to a DWM thumbnail.
        let overlay_rects = if self.last_stack_check.get().elapsed() >= Duration::from_millis(32) {
            self.last_stack_check.set(Instant::now());
            Some(
                hwnd(&self.window)
                    .map(win_utils::set_preview_topmost)
                    .unwrap_or_default(),
            )
        } else {
            None
        };
        let restart_capture = if let Some(input) = self.input.borrow_mut().as_mut() {
            if let Some(rects) = overlay_rects {
                input.set_overlay_rects(rects);
            }
            input.flush();
            if let Some(error) = input.take_error() {
                self.window.set_status_text(error.into());
            }
            let cursor = input.preview_cursor();
            self.window.set_preview_cursor_visible(cursor.is_some());
            if let Some((x, y)) = cursor {
                let dpi = self.window.window().scale_factor();
                self.window.set_preview_cursor_x(x as f32 / dpi);
                self.window.set_preview_cursor_y(y as f32 / dpi);
            }
            input.take_capture_restart()
        } else {
            self.window.set_preview_cursor_visible(false);
            false
        };
        if restart_capture {
            if let Some(source) = self
                .thumbnail
                .borrow()
                .as_ref()
                .map(|thumbnail| thumbnail.source)
            {
                match quality::QualitySession::start(source.0 as isize) {
                    Ok(session) => {
                        *self.quality.borrow_mut() = Some(session);
                    }
                    Err(error) => log::warn!(
                        "Unable to restart window capture after moving the source: {error:#}"
                    ),
                }
            }
        }
        let recording = self.state.lock().unwrap().recording;
        if !recording {
            self.owns_recording.set(false);
        }
        let own_recording = self.owns_recording.get() && recording;
        self.window.set_record_available(!recording);
        if self.window.get_recording() != own_recording {
            self.window.set_recording(own_recording);
            if !own_recording {
                if let Some(toolbar) = self.toolbar.upgrade() {
                    self.window.set_status_text(toolbar.get_status_text());
                }
            }
            self.window
                .window()
                .with_winit_window(|native| native.set_resizable(!own_recording));
        }
        // Resize on the next animation tick, including while the slider is held.
        // Native geometry changes stay outside Slint's pointer callback.
        if self.pending_zoom_resize.get() && !own_recording {
            self.pending_zoom_resize.set(false);
            self.resize_to_zoom()?;
        }
        self.constrain_to_work_area()?;
        let thumbnail = self.thumbnail.borrow();
        let Some(thumbnail) = thumbnail.as_ref() else {
            return Ok(());
        };
        anyhow::ensure!(
            unsafe { IsWindow(Some(thumbnail.source)).as_bool() },
            "The selected window was closed"
        );
        let minimized = unsafe { IsIconic(thumbnail.source).as_bool() };
        self.window.set_source_available(!minimized);
        self.capture_rect.set(None);
        if minimized {
            self.thumbnail_properties.set(None);
            self.window.set_quality_active(false);
            if let Some(quality) = self.quality.borrow().as_ref() {
                quality.request(None);
            }
            unsafe {
                DwmUpdateThumbnailProperties(
                    thumbnail.handle,
                    &DWM_THUMBNAIL_PROPERTIES {
                        dwFlags: DWM_TNP_VISIBLE,
                        fVisible: false.into(),
                        ..Default::default()
                    },
                )?;
            }
            self.window.set_preview_notice(RESTORE_SOURCE_NOTICE.into());
            return Ok(());
        }
        let source = unsafe { DwmQueryThumbnailSourceSize(thumbnail.handle)? };
        let source = if let Some(size) = self
            .quality
            .borrow()
            .as_ref()
            .filter(|quality| !quality.failed())
            .and_then(|quality| quality.source_size())
        {
            if self.quality_source_size.replace(Some(size)) != Some(size) && !own_recording {
                self.pending_zoom_resize.set(true);
            }
            (size.0 as i32, size.1 as i32)
        } else {
            (source.cx, source.cy)
        };
        if !own_recording && source.0 > 0 && source.1 > 0 {
            let work = work_area(hwnd(&self.window)?)?;
            let maximum = maximum_zoom_percent(
                source,
                (work.width, work.height),
                self.window.window().scale_factor() as f64,
                self.frame_extent(),
            );
            self.window.set_max_zoom_percent(maximum);
            let selected = bounded_zoom_percent((self.zoom.get() * 100.0) as f32, maximum);
            if selected != self.window.get_zoom_percent()
                || (selected as f64 / 100.0 - self.zoom.get()).abs() > 0.00001
            {
                self.window.set_zoom_percent(selected);
                self.zoom.set(selected as f64 / 100.0);
                self.pending_zoom_resize.set(true);
            }
        }
        let geometry = self
            .window
            .window()
            .with_winit_window(|native| {
                let client = native.inner_size();
                let position = native.inner_position().ok()?;
                let (rect, _) = preview_rect(
                    source,
                    (client.width as i32, client.height as i32),
                    native.scale_factor(),
                    self.zoom.get(),
                )?;
                Some((rect, position, native.scale_factor() as f32))
            })
            .flatten();
        let Some((rect, position, dpi)) = geometry else {
            return Ok(());
        };
        let key = quality::FrameKey {
            source: (source.0 as u32, source.1 as u32),
            output: (rect.width as u32, rect.height as u32),
        };
        let mut quality_failed = false;
        if let Some(quality) = self.quality.borrow().as_ref() {
            quality.request(Some(key.output));
            quality_failed = quality.failed();
            if let Some(frame) = quality
                .take_frame()
                .filter(|frame| frame.key.source == key.source)
            {
                self.window
                    .set_quality_image(slint::Image::from_rgba8(frame.buffer));
                self.quality_frame_key.set(Some(frame.key));
                *self.quality_pixels.borrow_mut() = Some(frame.pixels);
            }
        }
        let (quality_active, quality_exact) =
            quality_presentation(self.quality_frame_key.get(), key, quality_failed);
        let was_quality_active = self.window.get_quality_active();
        self.window.set_quality_x(rect.x as f32 / dpi);
        self.window.set_quality_y(rect.y as f32 / dpi);
        self.window.set_quality_width(rect.width as f32 / dpi);
        self.window.set_quality_height(rect.height as f32 / dpi);
        self.window.set_quality_active(quality_active);
        self.window.set_quality_exact(quality_exact);
        if quality_active && !self.input_attempted.replace(true) {
            match input::InputBridge::new(thumbnail.source, hwnd(&self.window)?) {
                Ok(input) => *self.input.borrow_mut() = Some(input),
                Err(error) => self
                    .window
                    .set_status_text(format!("Window interaction unavailable: {error}").into()),
            }
        }
        if let Some(input) = self.input.borrow_mut().as_mut() {
            input.set_view(
                rect,
                self.quality_frame_key
                    .get()
                    .map_or(key.source, |frame| frame.source),
            );
        }
        let visible = !quality_active || !was_quality_active;
        let thumbnail_properties = (rect.x, rect.y, rect.width, rect.height, visible);
        if self.thumbnail_properties.get() != Some(thumbnail_properties) {
            unsafe {
                DwmUpdateThumbnailProperties(
                    thumbnail.handle,
                    &DWM_THUMBNAIL_PROPERTIES {
                        dwFlags: DWM_TNP_RECTDESTINATION
                            | DWM_TNP_VISIBLE
                            | DWM_TNP_OPACITY
                            | DWM_TNP_SOURCECLIENTAREAONLY,
                        rcDestination: RECT {
                            left: rect.x,
                            top: rect.y,
                            right: rect.x + rect.width,
                            bottom: rect.y + rect.height,
                        },
                        opacity: 255,
                        // Let Slint paint the first image before removing the native
                        // preview. Subsequent zooms keep using the same image surface.
                        fVisible: visible.into(),
                        fSourceClientAreaOnly: false.into(),
                        ..Default::default()
                    },
                )?;
            }
            self.thumbnail_properties.set(Some(thumbnail_properties));
        }
        self.capture_rect.set(Some(capture::CaptureRect {
            x: position.x + rect.x,
            y: position.y + rect.y,
            ..rect
        }));
        let partly_visible = self
            .input
            .borrow()
            .as_ref()
            .is_some_and(|input| !input.source_covered());
        self.window.set_preview_notice(if partly_visible {
            PARTLY_VISIBLE_NOTICE.into()
        } else {
            "".into()
        });
        Ok(())
    }

    fn reset_quality(&self) {
        self.quality.borrow_mut().take();
        self.quality_frame_key.set(None);
        self.quality_pixels.borrow_mut().take();
        self.thumbnail_properties.set(None);
        self.quality_source_size.set(None);
        self.window.set_quality_active(false);
        self.window.set_quality_exact(false);
        self.window.set_quality_image(slint::Image::default());
    }

    fn frame_extent(&self) -> (i32, i32) {
        self.window
            .window()
            .with_winit_window(|native| {
                let outer = native.outer_size();
                let inner = native.inner_size();
                (
                    outer.width.saturating_sub(inner.width) as i32,
                    outer.height.saturating_sub(inner.height) as i32,
                )
            })
            .unwrap_or_default()
    }

    fn constrain_to_work_area(&self) -> Result<()> {
        let work = work_area(hwnd(&self.window)?)?;
        let (frame_w, frame_h) = self.frame_extent();
        let geometry = self
            .window
            .window()
            .with_winit_window(|native| {
                if native.is_minimized() == Some(true) {
                    return None;
                }
                let Ok(position) = native.outer_position() else {
                    return None;
                };
                let size = native.outer_size();
                let bounded = window_bounds::constrain_rect(
                    RECT {
                        left: position.x,
                        top: position.y,
                        right: position.x + size.width as i32,
                        bottom: position.y + size.height as i32,
                    },
                    RECT {
                        left: work.x,
                        top: work.y,
                        right: work.x + work.width,
                        bottom: work.y + work.height,
                    },
                );
                Some((position, size, bounded))
            })
            .flatten();
        let Some((position, size, bounded)) = geometry else {
            return Ok(());
        };
        // Change Slint geometry after releasing the native-window accessor.
        if bounded.right - bounded.left != size.width as i32
            || bounded.bottom - bounded.top != size.height as i32
        {
            self.window.window().set_size(slint::PhysicalSize::new(
                (bounded.right - bounded.left - frame_w).max(1) as u32,
                (bounded.bottom - bounded.top - frame_h).max(1) as u32,
            ));
        }
        if (position.x, position.y) != (bounded.left, bounded.top) {
            self.window.window().with_winit_window(|native| {
                native.set_outer_position(winit::dpi::PhysicalPosition::new(
                    bounded.left,
                    bounded.top,
                ));
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_limit_uses_integer_percent_and_respects_the_fifty_percent_minimum() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            for source in [(800, 600), (1920, 1080), (3840, 2160)] {
                let limit = maximum_zoom_percent(source, (1920, 1040), dpi, (0, 0));
                assert_eq!(limit.fract(), 0.0);
                assert!(limit >= 50.0);
                let (w, h) = fitted_window_size(source, limit as f64 / 100.0, dpi, (1920, 1040));
                let (_, actual) =
                    preview_rect(source, (w as i32, h as i32), dpi, limit as f64 / 100.0).unwrap();
                assert!(actual * 100.0 <= limit as f64 + 0.01);
                if limit > 50.0 {
                    assert!((actual * 100.0 - limit as f64).abs() < 0.01);
                }
                assert_eq!(bounded_zoom_percent(400.0, limit), limit);
            }
        }
        assert_eq!(bounded_zoom_percent(100.0, 25.0), 50.0);
        assert_eq!(bounded_zoom_percent(145.0, 165.0), 145.0);
        assert_eq!(bounded_zoom_percent(146.0, 165.0), 146.0);
        assert_eq!(bounded_zoom_percent(200.0, 167.8), 167.0);
    }

    #[test]
    fn pending_zoom_frames_keep_the_image_visible_instead_of_switching_to_dwm() {
        let rendered = quality::FrameKey {
            source: (800, 600),
            output: (800, 600),
        };
        assert_eq!(quality_presentation(None, rendered, false), (false, false));
        assert_eq!(
            quality_presentation(Some(rendered), rendered, false),
            (true, true)
        );
        for output in [(840, 630), (880, 660), (1200, 900)] {
            let requested = quality::FrameKey { output, ..rendered };
            assert_eq!(
                quality_presentation(Some(rendered), requested, false),
                (true, false)
            );
            assert_eq!(
                quality_presentation(Some(requested), requested, false),
                (true, true)
            );
        }
        assert_eq!(
            quality_presentation(Some(rendered), rendered, true),
            (false, false)
        );
    }

    #[test]
    fn zoom_rounds_to_one_percent_and_clamps_the_slider_limits() {
        for (input, expected) in [
            (-10.0, 50.0),
            (30.0, 50.0),
            (50.0, 50.0),
            (50.4, 50.0),
            (50.5, 51.0),
            (51.0, 51.0),
            (101.0, 101.0),
            (397.0, 397.0),
            (399.0, 399.0),
            (500.0, 400.0),
            (f32::NAN, 200.0),
        ] {
            assert_eq!(snap_zoom_percent(input), expected);
        }
    }

    #[test]
    fn zoom_slider_is_visible_in_the_header_in_light_dark_and_small_windows() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
        struct Headless(Rc<MinimalSoftwareWindow>);
        impl slint::platform::Platform for Headless {
            fn create_window_adapter(
                &self,
            ) -> std::result::Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError>
            {
                Ok(self.0.clone())
            }
        }
        let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(surface.clone()))).unwrap();
        let window = MagnifierWindow::new().unwrap();
        window.set_source_available(true);
        window.show().unwrap();
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/magnifier-preview");
        std::fs::create_dir_all(&directory).unwrap();
        for (name, width, height, dark) in [
            ("light", 800u32, 560u32, false),
            ("dark", 800, 560, true),
            ("small", 480, 190, true),
        ] {
            window.set_dark_theme(dark);
            surface.set_size(slint::PhysicalSize::new(width, height));
            let render = |percent| {
                window.set_zoom_percent(percent);
                window.window().request_redraw();
                let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                assert!(surface.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                }));
                pixels
            };
            let low = render(50.0);
            let high = render(400.0);
            let header_end = (width * 90.min(height)) as usize;
            // The track/thumb and percentage label must repaint in the upper controls.
            assert!(
                low[..header_end]
                    .iter()
                    .zip(&high[..header_end])
                    .filter(|(a, b)| a != b)
                    .count()
                    > 60
            );
            let bytes: Vec<u8> = high
                .iter()
                .flat_map(|pixel| [pixel.r, pixel.g, pixel.b])
                .collect();
            image::save_buffer(
                directory.join(format!("{name}.png")),
                &bytes,
                width,
                height,
                image::ColorType::Rgb8,
            )
            .unwrap();
            if name == "light" {
                let header = image::RgbaImage::from_fn(width, HEADER as u32, |x, y| {
                    let pixel = high[(y * width + x) as usize];
                    image::Rgba([pixel.r, pixel.g, pixel.b, 255])
                });
                let output = (width * 155 / 100, HEADER as u32 * 155 / 100);
                quality::resize_original(&mut fast_image_resize::Resizer::new(), &header, output)
                    .unwrap()
                    .save(directory.join("header-lanczos-155.png"))
                    .unwrap();
                image::imageops::resize(
                    &header,
                    output.0,
                    output.1,
                    image::imageops::FilterType::Triangle,
                )
                .save(directory.join("header-bilinear-155.png"))
                .unwrap();
            }
        }

        // Already-filtered pixels must survive Slint's fractional-DPI layout
        // without being filtered or shifted again on their way to the screen.
        let sample = image::RgbaImage::from_fn(63, 47, |x, y| {
            image::Rgba([
                (x * 3) as u8,
                (y * 5) as u8,
                if (x + y) % 2 == 0 { 240 } else { 15 },
                255,
            ])
        });
        window.set_quality_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
            slint::Rgba8Pixel,
        >::clone_from_slice(
            sample.as_raw(), 63, 47
        )));
        window.set_quality_active(true);
        window.set_quality_exact(true);
        for dpi in [1.0f32, 1.25, 1.5, 2.0] {
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: dpi,
                });
            surface.set_size(slint::PhysicalSize::new(1000, 700));
            window.set_quality_x(20.0 / dpi);
            window.set_quality_y(120.0 / dpi);
            window.set_quality_width(63.0 / dpi);
            window.set_quality_height(47.0 / dpi);
            window.window().request_redraw();
            let mut pixels = vec![slint::Rgb8Pixel::default(); 1000 * 700];
            assert!(surface.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, 1000);
            }));
            for y in 1..46 {
                for x in 1..62 {
                    let expected = sample.get_pixel(x, y);
                    let actual = pixels[((120 + y) * 1000 + 20 + x) as usize];
                    assert_eq!(
                        [actual.r, actual.g, actual.b],
                        [expected[0], expected[1], expected[2]],
                        "DPI {dpi} at {x},{y}"
                    );
                }
            }
        }

        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        window.set_quality_exact(false);
        window.set_quality_x(20.0);
        window.set_quality_y(120.0);
        // No replacement frame is supplied during these consecutive resizes.
        // The existing texture must remain painted at every intermediate size.
        for (width, height) in [(84u32, 63u32), (126, 94), (42, 31), (63, 47)] {
            surface.set_size(slint::PhysicalSize::new(800 + width, 700));
            window.set_quality_width(width as f32);
            window.set_quality_height(height as f32);
            window.window().request_redraw();
            let stride = (800 + width) as usize;
            let mut pixels = vec![slint::Rgb8Pixel::default(); stride * 700];
            assert!(surface.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, stride);
            }));
            let center = pixels[(120 + height / 2) as usize * stride + (20 + width / 2) as usize];
            assert!(
                center.r > 60 && center.g > 80,
                "Cached image disappeared at {width}x{height}"
            );
        }
    }

    #[test]
    fn zoom_window_stays_inside_work_area_including_header_at_each_dpi() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            for source in [(120, 80), (1600, 900), (700, 2200)] {
                for percent in 50..=400 {
                    let zoom = percent as f64 / 100.0;
                    let (w, h) = fitted_window_size(source, zoom, dpi, (1920, 1040));
                    assert!(w <= 1920 && h <= 1040);
                    let (rect, scale) =
                        preview_rect(source, (w as i32, h as i32), dpi, zoom).unwrap();
                    assert!(rect.x >= 0 && rect.y >= (HEADER * dpi) as i32);
                    assert!(rect.x + rect.width <= w as i32);
                    assert!(rect.y + rect.height <= h as i32 - (INSET * dpi).ceil() as i32);
                    assert!((rect.width as f64 - source.0 as f64 * scale).abs() < 1.01);
                    assert!((rect.height as f64 - source.1 as f64 * scale).abs() < 1.01);
                    assert!(scale <= zoom);
                }
            }
        }
    }

    #[test]
    fn small_source_is_enlarged_to_requested_physical_pixel_ratio() {
        for dpi in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let (w, h) = fitted_window_size((300, 200), 2.0, dpi, (1920, 1040));
            let (rect, scale) = preview_rect((300, 200), (w as i32, h as i32), dpi, 2.0).unwrap();
            assert_eq!((rect.width, rect.height), (600, 400));
            assert_eq!(scale, 2.0);
        }
    }

    #[test]
    fn bottom_frame_matches_both_side_frames_at_each_dpi() {
        for dpi in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let (w, h) = fitted_window_size((1200, 600), 1.0, dpi, (1920, 1040));
            let (rect, _) = preview_rect((1200, 600), (w as i32, h as i32), dpi, 1.0).unwrap();
            let right = w as i32 - rect.x - rect.width;
            let bottom = h as i32 - rect.y - rect.height;
            assert_eq!(bottom, rect.x);
            assert_eq!(bottom, right);
            assert_eq!(bottom, (INSET * dpi).ceil() as i32);
        }
    }

    #[test]
    fn manual_resize_keeps_whole_source_in_view_without_stretching() {
        let (rect, scale) = preview_rect((1920, 1080), (400, 600), 1.0, 4.0).unwrap();
        assert_eq!(rect.width, 388);
        assert_eq!(rect.height, 218);
        assert!(scale < 1.0);
        assert!(rect.y > HEADER as i32);
        assert!(preview_rect((0, 1080), (400, 600), 1.0, 2.0).is_none());
    }
}
