use crate::state::{AppState, SelectionPurpose};
use crate::text_layout::*;
use crate::{
    api, capture, ocr, win_utils, CaptureFrameWindow, CaptureToolbarWindow, MainWindow,
    RecordingBorderWindow, SelectionWindow,
};
use anyhow::{Context, Result};
use global_hotkey::{hotkey::HotKey, GlobalHotKeyManager};
use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) fn selection_pixel_at(state: &AppState, x: f32, y: f32) -> Option<image::Rgba<u8>> {
    if state.pending_selection != Some(SelectionPurpose::ColorPicker) {
        return None;
    }
    let screenshot = state.selection_screenshot.as_ref()?;
    let scale = state.selection_scale.max(1.0);
    let local_x = (x * scale).round() as i32;
    let local_y = (y * scale).round() as i32;
    if local_x < 0
        || local_y < 0
        || local_x >= screenshot.width() as i32
        || local_y >= screenshot.height() as i32
    {
        return None;
    }
    Some(*screenshot.get_pixel(local_x as u32, local_y as u32))
}

fn selection_magnifier_pixels(state: &AppState, x: f32, y: f32) -> Option<image::RgbaImage> {
    let screenshot = state.selection_screenshot.as_ref()?;
    let scale = state.selection_scale.max(1.0);
    let center_x = (x * scale).round() as i32;
    let center_y = (y * scale).round() as i32;

    if center_x < 0
        || center_y < 0
        || center_x >= screenshot.width() as i32
        || center_y >= screenshot.height() as i32
    {
        return None;
    }

    let mut pixels = image::RgbaImage::from_pixel(9, 9, image::Rgba([15, 23, 42, 255]));
    for preview_y in 0..9 {
        for preview_x in 0..9 {
            let source_x = center_x + preview_x as i32 - 4;
            let source_y = center_y + preview_y as i32 - 4;
            if source_x >= 0
                && source_y >= 0
                && source_x < screenshot.width() as i32
                && source_y < screenshot.height() as i32
            {
                pixels.put_pixel(
                    preview_x,
                    preview_y,
                    *screenshot.get_pixel(source_x as u32, source_y as u32),
                );
            }
        }
    }

    Some(pixels)
}

pub(crate) fn selection_magnifier_at(state: &AppState, x: f32, y: f32) -> Option<slint::Image> {
    selection_magnifier_pixels(state, x, y).map(rgba_to_slint_image)
}

pub(crate) fn sync_capture_state(state: &mut AppState, main: &MainWindow) {
    state.api_endpoint = main.get_api_endpoint().to_string();
    state.api_key = main.get_api_key().to_string();
    state.model_name = main.get_model_name().to_string();
    state.interval_sec = main.get_interval();
    state.system_prompt = main.get_system_prompt().to_string();
    state.temperature = main.get_temperature();
    state.thinking_level = main.get_thinking_level().to_string();
    state.provider = main.get_api_type().to_string();
    state.base_font_size = main.get_base_font_size();
    state.overlay_bg_color = main.get_overlay_bg_color();
    state.overlay_text_color = main.get_overlay_text_color();
    state.overlay_bg_opacity = main.get_overlay_bg_opacity();
    state.use_textbox = main.get_use_textbox();
    state.capture_folder = main.get_capture_folder().to_string();
}

pub(crate) fn make_api_client(http: &reqwest::Client, main: &MainWindow) -> api::ApiClient {
    api::ApiClient::new(
        http.clone(),
        main.get_api_endpoint().to_string(),
        main.get_api_key().to_string(),
        main.get_model_name().to_string(),
        main.get_system_prompt().to_string(),
        main.get_temperature(),
        main.get_thinking_level().to_string(),
        main.get_api_type().to_string(),
    )
}

pub(crate) fn physical_selection_rect(
    state: &AppState,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> capture::CaptureRect {
    let scale = if state.selection_scale > 0.0 {
        state.selection_scale
    } else {
        1.0
    };
    capture::CaptureRect {
        x: state.selection_origin_x + (x * scale).round() as i32,
        y: state.selection_origin_y + (y * scale).round() as i32,
        width: (width * scale).round() as i32,
        height: (height * scale).round() as i32,
    }
}

pub(crate) fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{:02}:{:02}", (seconds / 60) % 60, seconds % 60)
}

pub(crate) fn current_recording_elapsed(state: &AppState) -> Duration {
    let Some(started) = state.recording_started_at else {
        return Duration::ZERO;
    };
    let paused = state.recording_paused_total
        + state
            .recording_paused_at
            .map(|at| at.elapsed())
            .unwrap_or_default();
    started.elapsed().saturating_sub(paused)
}

pub(crate) fn sync_capture_toolbar_size(toolbar: &CaptureToolbarWindow) {
    let height = if toolbar.get_recording()
        || (toolbar.get_status_text().is_empty() && toolbar.get_active_tooltip().is_empty())
    {
        48.0
    } else {
        68.0
    };
    toolbar
        .window()
        .set_size(slint::LogicalSize::new(CAPTURE_TOOLBAR_WIDTH, height));
}

pub(crate) const CAPTURE_TOOLBAR_WIDTH: f32 = 603.0;
pub(crate) const CAPTURE_FRAME_HEADER: f32 = 42.0;
pub(crate) const CAPTURE_FRAME_BORDER: f32 = 3.0;
pub(crate) const OCR_WINDOW_WIDTH: f32 = 400.0;
pub(crate) const OCR_WINDOW_CLOSED_HEIGHT: f32 = 880.0;
pub(crate) const OCR_WINDOW_STYLE_HEIGHT: f32 = 1000.0;

pub(crate) fn sync_ocr_window_size(main: &MainWindow) {
    let height = if main.get_show_style_settings() {
        OCR_WINDOW_STYLE_HEIGHT
    } else {
        OCR_WINDOW_CLOSED_HEIGHT
    };
    main.window()
        .set_size(slint::LogicalSize::new(OCR_WINDOW_WIDTH, height));
}

#[cfg(target_os = "windows")]
pub(crate) fn configure_main_window_native_theme(main: &MainWindow, dark_theme: bool) -> bool {
    let mut configured = false;
    let _ = main.window().with_winit_window(|winit_window| {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
        win_utils::set_mica_backdrop(hwnd);
        win_utils::set_title_bar_theme(hwnd, dark_theme);
        configured = true;
    });
    configured
}

#[cfg(target_os = "windows")]
pub(crate) fn schedule_main_window_native_theme(
    main: slint::Weak<MainWindow>,
    dark_theme: bool,
    attempt: usize,
) {
    // Winit can create the native HWND just after Slint's show callback returns. Reapply the
    // DWM attributes over the short creation/activation window so startup and mode switches do
    // not leave a light non-client title bar above the dark OCR UI.
    const RETRY_DELAYS_MS: [u64; 6] = [0, 16, 50, 150, 350, 700];
    let Some(delay) = RETRY_DELAYS_MS.get(attempt).copied() else {
        return;
    };

    slint::Timer::single_shot(Duration::from_millis(delay), move || {
        let Some(main) = main.upgrade() else {
            return;
        };
        let _ = configure_main_window_native_theme(&main, dark_theme);
        schedule_main_window_native_theme(main.as_weak(), dark_theme, attempt + 1);
    });
}

#[cfg(target_os = "windows")]
pub(crate) fn configure_capture_toolbar_native_window(toolbar: &CaptureToolbarWindow) {
    let _ = toolbar.window().with_winit_window(|winit_window| {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
        win_utils::set_layered(hwnd);
        win_utils::set_tool_window(hwnd, true);
        win_utils::set_exclude_from_capture(hwnd);
        win_utils::disable_window_transitions(hwnd);
    });
}

#[cfg(target_os = "windows")]
pub(crate) fn configure_capture_frame_native_window(frame: &CaptureFrameWindow) {
    let _ = frame.window().with_winit_window(|winit_window| {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
        win_utils::set_layered(hwnd);
        win_utils::set_tool_window(hwnd, true);
        win_utils::set_exclude_from_capture(hwnd);
        win_utils::disable_window_transitions(hwnd);
    });
}

pub(crate) fn position_capture_frame(frame: &CaptureFrameWindow) {
    let (cursor_x, cursor_y) = capture::cursor_position();
    let monitor = capture::monitor_rect_at_point(cursor_x, cursor_y).ok();
    let (position, size) = if let Some(monitor) = monitor {
        let width = (monitor.width * 3 / 5).clamp(480, 960);
        let content_height = (width * 9 / 16).clamp(270, (monitor.height * 3 / 5).max(270));
        let height = content_height + CAPTURE_FRAME_HEADER as i32 + CAPTURE_FRAME_BORDER as i32;
        (
            slint::PhysicalPosition::new(
                monitor.x + (monitor.width - width) / 2,
                monitor.y + ((monitor.height - height) / 2).max(32),
            ),
            slint::PhysicalSize::new(width as u32, height as u32),
        )
    } else {
        (
            slint::PhysicalPosition::new(120, 120),
            slint::PhysicalSize::new(720, 450),
        )
    };
    frame
        .window()
        .set_position(slint::WindowPosition::Physical(position));
    frame.window().set_size(slint::WindowSize::Physical(size));
}

pub(crate) fn show_capture_frame(frame: &CaptureFrameWindow, use_default_geometry: bool) -> bool {
    if use_default_geometry {
        position_capture_frame(frame);
    }
    if frame.show().is_err() {
        return false;
    }
    #[cfg(target_os = "windows")]
    configure_capture_frame_native_window(frame);
    if use_default_geometry {
        // Secondary windows are created lazily; repeat the physical placement after show().
        position_capture_frame(frame);
    }
    true
}

pub(crate) fn capture_frame_rect(
    frame: &CaptureFrameWindow,
) -> anyhow::Result<capture::CaptureRect> {
    let position = frame.window().position();
    let size = frame.window().size();
    let scale = frame.window().scale_factor().max(1.0);
    capture_frame_rect_from_geometry(
        position.x,
        position.y,
        size.width as i32,
        size.height as i32,
        scale,
    )
}

fn capture_frame_rect_from_geometry(
    x: i32,
    y: i32,
    frame_width: i32,
    frame_height: i32,
    scale: f32,
) -> anyhow::Result<capture::CaptureRect> {
    let side = (CAPTURE_FRAME_BORDER * scale).round() as i32;
    let top = (CAPTURE_FRAME_HEADER * scale).round() as i32;
    let width = frame_width - side * 2;
    let height = frame_height - top - side;
    anyhow::ensure!(width >= 64 && height >= 64, "Capture frame is too small");
    Ok(capture::CaptureRect {
        x: x + side,
        y: y + top,
        width,
        height,
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn configure_recording_border_native_window(border: &RecordingBorderWindow) {
    let _ = border.window().with_winit_window(|winit_window| {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
        win_utils::set_layered(hwnd);
        win_utils::set_tool_window(hwnd, false);
        win_utils::set_click_through(hwnd, true);
        win_utils::set_exclude_from_capture(hwnd);
        win_utils::disable_window_transitions(hwnd);
    });
}

pub(crate) fn display_recording_border(border: &RecordingBorderWindow, rect: capture::CaptureRect) {
    let position = slint::WindowPosition::Physical(slint::PhysicalPosition::new(rect.x, rect.y));
    let size = slint::WindowSize::Physical(slint::PhysicalSize::new(
        rect.width.max(1) as u32,
        rect.height.max(1) as u32,
    ));
    // CaptureRect is expressed in xcap/Win32 physical desktop pixels. Use the physical Slint
    // APIs as well, otherwise a per-monitor DPI scale can move the border away from the pixels
    // that FFmpeg records.
    border.window().set_position(position.clone());
    border.window().set_size(size.clone());
    if border.show().is_ok() {
        // Winit may apply its default position/size while lazily creating this secondary window;
        // re-apply the physical geometry after show() so the first frame is aligned too.
        border.window().set_position(position);
        border.window().set_size(size);
        #[cfg(target_os = "windows")]
        configure_recording_border_native_window(border);
    }
}

pub(crate) fn shutdown_and_exit(
    recorder_slot: &Arc<Mutex<Option<capture::ScreenRecorder>>>,
    recording_border: &slint::Weak<RecordingBorderWindow>,
) -> ! {
    if let Some(border) = recording_border.upgrade() {
        let _ = border.hide();
    }
    let recorder = match recorder_slot.lock() {
        Ok(mut slot) => slot.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    if let Some(recorder) = recorder {
        if let Err(error) = recorder.stop() {
            log::warn!("Failed to stop recording during application exit: {error:?}");
        }
    }
    std::process::exit(0);
}

pub(crate) fn set_capture_toolbar_status(toolbar: &CaptureToolbarWindow, status: String) {
    toolbar.set_status_text(status.into());
    sync_capture_toolbar_size(toolbar);
}

pub(crate) fn position_capture_toolbar(toolbar: &CaptureToolbarWindow) {
    sync_capture_toolbar_size(toolbar);
    let (cursor_x, cursor_y) = capture::cursor_position();
    let monitor = {
        #[cfg(target_os = "windows")]
        {
            capture::native_monitor_rect_at_point(cursor_x, cursor_y)
        }
        #[cfg(not(target_os = "windows"))]
        {
            capture::monitor_rect_at_point(cursor_x, cursor_y).ok()
        }
    };
    let scale = toolbar.window().scale_factor().max(1.0);
    let (x, y) = if let Some(monitor) = monitor {
        let monitor_width = monitor.width as f32 / scale;
        let toolbar_width = CAPTURE_TOOLBAR_WIDTH;
        (
            monitor.x as f32 / scale + ((monitor_width - toolbar_width) / 2.0).max(12.0),
            monitor.y as f32 / scale + 18.0,
        )
    } else {
        // A safe fallback keeps the toolbar usable even while monitor enumeration is unavailable.
        (18.0, 18.0)
    };
    toolbar
        .window()
        .set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
            x, y,
        )));
}

pub(crate) fn show_capture_toolbar_at_top_center(toolbar: &CaptureToolbarWindow) -> bool {
    // Position before showing to avoid a visible jump, then repeat once the native window has
    // a monitor scale so the final location is exactly centered on the top edge.
    toolbar.set_active_tooltip(String::new().into());
    position_capture_toolbar(toolbar);
    if toolbar.show().is_err() {
        return false;
    }
    #[cfg(target_os = "windows")]
    configure_capture_toolbar_native_window(toolbar);
    position_capture_toolbar(toolbar);
    // The first native placement can be supplied by the platform (often top-right) even when
    // the logical position was set before show(). Re-apply the centered position after one
    // event-loop turn so the initial visible location is also top-center.
    let toolbar_weak = toolbar.as_weak();
    slint::Timer::single_shot(Duration::from_millis(32), move || {
        if let Some(toolbar) = toolbar_weak.upgrade() {
            position_capture_toolbar(&toolbar);
        }
    });
    true
}

pub(crate) fn prepare_selection_window(
    selection: &SelectionWindow,
    state: &Arc<Mutex<AppState>>,
    purpose: SelectionPurpose,
    window_mode: bool,
    color_picker_mode: bool,
    selection_initialized: &Arc<Mutex<bool>>,
    hotkey_manager: &Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
    owner: Option<isize>,
) -> bool {
    let _ = selection.hide();
    selection.set_window_mode(window_mode);
    selection.set_color_picker_mode(color_picker_mode);
    selection.set_ruler_mode(purpose == SelectionPurpose::Ruler);
    selection.invoke_reset();

    let (cursor_x, cursor_y) = capture::cursor_position();
    let (monitor_rect, screenshot) = match capture::capture_monitor_at_point(cursor_x, cursor_y) {
        Ok((rect, image)) => (rect, image),
        Err(_) => {
            let Ok(image) = capture::capture_full_screen() else {
                if let Ok(mut state) = state.lock() {
                    state.pending_selection = None;
                }
                return false;
            };
            (
                capture::CaptureRect {
                    x: 0,
                    y: 0,
                    width: image.width() as i32,
                    height: image.height() as i32,
                },
                image,
            )
        }
    };

    let (width, height) = screenshot.dimensions();
    let scale = selection.window().scale_factor().max(1.0);
    // Keep the pixels while the selector is open. Both the color picker and the region
    // magnifier sample from this immutable frame, so the preview exactly matches the capture.
    let selection_screenshot = (color_picker_mode
        || (!window_mode && purpose != SelectionPurpose::Ruler))
        .then(|| Arc::new(screenshot.clone()));
    {
        let mut state = state.lock().unwrap();
        state.pending_selection = Some(purpose);
        state.selection_origin_x = monitor_rect.x;
        state.selection_origin_y = monitor_rect.y;
        state.selection_scale = scale;
        state.selection_screenshot = selection_screenshot;
    }
    selection.set_screenshot(rgba_to_slint_image(screenshot));
    selection.window().set_size(slint::LogicalSize::new(
        width as f32 / scale,
        height as f32 / scale,
    ));
    selection
        .window()
        .set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
            monitor_rect.x as f32 / scale,
            monitor_rect.y as f32 / scale,
        )));

    let should_initialize = !*selection_initialized.lock().unwrap();
    if let Err(error) = selection.show() {
        log::error!("Failed to show selection window: {error:?}");
        if let Ok(mut state) = state.lock() {
            state.pending_selection = None;
        }
        return false;
    }
    // Configure the native selection window only after show(). Winit creates secondary windows
    // lazily, so doing this before show() can leave the first capture action unconfigured.
    #[cfg(target_os = "windows")]
    {
        selection.window().with_winit_window(move |winit_window| {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = winit_window.window_handle() {
                if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                    let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
                    // These operations are idempotent and must also run when switching between
                    // OCR and capture mode, because the selector's owner may have changed.
                    win_utils::set_tool_window(hwnd, true);
                    win_utils::set_exclude_from_capture(hwnd);
                    win_utils::disable_window_transitions(hwnd);
                    if let Some(owner) = owner {
                        win_utils::set_window_owner(
                            hwnd,
                            windows::Win32::Foundation::HWND(owner as _),
                        );
                    } else {
                        // The OCR window is hidden in capture mode. Clear a previous owner so
                        // an already-initialized selector is not hidden along with that window.
                        win_utils::set_window_owner(
                            hwnd,
                            windows::Win32::Foundation::HWND(std::ptr::null_mut()),
                        );
                    }
                }
            }
        });
    }
    if should_initialize {
        *selection_initialized.lock().unwrap() = true;
    }
    if let Some(manager) = hotkey_manager {
        let _ = manager.register(esc_hotkey);
    }
    true
}

pub(crate) fn begin_toolbar_selection(
    toolbar: &CaptureToolbarWindow,
    selection_weak: &slint::Weak<SelectionWindow>,
    state: &Arc<Mutex<AppState>>,
    purpose: SelectionPurpose,
    window_mode: bool,
    color_picker_mode: bool,
    selection_initialized: &Arc<Mutex<bool>>,
    hotkey_manager: &Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
) {
    if toolbar.get_recording() {
        return;
    }

    // Do not hide the toolbar or create/show another native window while Winit is dispatching
    // the toolbar's pointer event. On Windows that re-entrant window change can crash the app.
    let toolbar_weak = toolbar.as_weak();
    let selection_weak = selection_weak.clone();
    let state = state.clone();
    let selection_initialized = selection_initialized.clone();
    let hotkey_manager = hotkey_manager.clone();
    slint::Timer::single_shot(Duration::from_millis(1), move || {
        let Some(toolbar) = toolbar_weak.upgrade() else {
            return;
        };
        begin_toolbar_selection_now(
            &toolbar,
            &selection_weak,
            &state,
            purpose,
            window_mode,
            color_picker_mode,
            &selection_initialized,
            &hotkey_manager,
            esc_hotkey,
        );
    });
}

pub(crate) fn begin_toolbar_selection_now(
    toolbar: &CaptureToolbarWindow,
    selection_weak: &slint::Weak<SelectionWindow>,
    state: &Arc<Mutex<AppState>>,
    purpose: SelectionPurpose,
    window_mode: bool,
    color_picker_mode: bool,
    selection_initialized: &Arc<Mutex<bool>>,
    hotkey_manager: &Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
) {
    if toolbar.get_recording() {
        return;
    }
    let Some(selection) = selection_weak.upgrade() else {
        return;
    };
    toolbar.set_active_tooltip(String::new().into());
    sync_capture_toolbar_size(toolbar);

    // Keep the toolbar alive until the selection window has been shown. Slint ends the event
    // loop when the last visible window is hidden; in capture mode the toolbar is that last
    // window, so hiding it first makes the app terminate before selection can start. The toolbar
    // is excluded from desktop capture and can therefore remain visible while the screenshot is
    // prepared.
    let toolbar_weak = toolbar.as_weak();
    let selection_weak = selection.as_weak();
    let state = state.clone();
    let selection_initialized = selection_initialized.clone();
    let hotkey_manager = hotkey_manager.clone();
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        let (Some(toolbar), Some(selection)) = (toolbar_weak.upgrade(), selection_weak.upgrade())
        else {
            return;
        };
        if toolbar.get_recording() {
            return;
        }
        if !prepare_selection_window(
            &selection,
            &state,
            purpose,
            window_mode,
            color_picker_mode,
            &selection_initialized,
            &hotkey_manager,
            esc_hotkey,
            None,
        ) {
            return;
        }
        // Now that the selection window owns the visible-window slot, it is safe to hide the
        // toolbar without causing Slint to quit its event loop.
        if let Err(error) = toolbar.hide() {
            log::warn!("Failed to hide capture toolbar for selection: {error:?}");
        }
    });
}

pub(crate) fn begin_fullscreen_toolbar_action(
    toolbar: &CaptureToolbarWindow,
    main: &MainWindow,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http: reqwest::Client,
    recording_border: slint::Weak<RecordingBorderWindow>,
) {
    if toolbar.get_recording() {
        return;
    }

    // Match the selection actions above: native window changes must happen after the pointer
    // callback returns, not while the capture toolbar is still handling the click.
    let toolbar_weak = toolbar.as_weak();
    let main_weak = main.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1), move || {
        let (Some(toolbar), Some(main)) = (toolbar_weak.upgrade(), main_weak.upgrade()) else {
            return;
        };
        begin_fullscreen_toolbar_action_now(
            &toolbar,
            &main,
            state,
            recorder_slot,
            http,
            recording_border,
        );
    });
}

pub(crate) fn begin_fullscreen_toolbar_action_now(
    toolbar: &CaptureToolbarWindow,
    main: &MainWindow,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http: reqwest::Client,
    recording_border: slint::Weak<RecordingBorderWindow>,
) {
    if toolbar.get_recording() {
        return;
    }
    let action = if toolbar.get_record_mode() {
        SelectionPurpose::Record
    } else {
        SelectionPurpose::Capture
    };
    toolbar.set_active_tooltip(String::new().into());
    sync_capture_toolbar_size(toolbar);

    // xcap's Windows monitor capture uses BitBlt, so move the toolbar outside the virtual desktop
    // for a full-screen still image. Keeping the window visible avoids a no-window event-loop
    // shutdown while still guaranteeing that the toolbar is not present in the captured pixels.
    #[cfg(target_os = "windows")]
    configure_capture_toolbar_native_window(toolbar);
    let toolbar_weak = toolbar.as_weak();
    let main_weak = main.as_weak();
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        let (Some(toolbar), Some(main)) = (toolbar_weak.upgrade(), main_weak.upgrade()) else {
            return;
        };
        let (cursor_x, cursor_y) = capture::cursor_position();
        let rect = match capture::monitor_rect_at_point(cursor_x, cursor_y) {
            Ok(rect) => rect,
            Err(error) => {
                set_capture_toolbar_status(&toolbar, format!("Error: {error}"));
                let _ = toolbar.show();
                return;
            }
        };
        let prefetched_image = if action == SelectionPurpose::Capture {
            toolbar
                .window()
                .set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
                    -10000.0, -10000.0,
                )));
            // Give the native window manager a frame to apply the off-screen move before BitBlt.
            std::thread::sleep(Duration::from_millis(32));
            let image = capture::capture_area(&rect, &None);
            position_capture_toolbar(&toolbar);
            match image {
                Ok(image) => Some(image),
                Err(error) => {
                    set_capture_toolbar_status(&toolbar, format!("Error: {error}"));
                    return;
                }
            }
        } else {
            None
        };
        let spawn_result = slint::spawn_local(run_toolbar_action(
            action,
            rect,
            None,
            prefetched_image,
            false,
            recording_border,
            main.as_weak(),
            toolbar.as_weak(),
            state,
            recorder_slot,
            http,
        ));
        if let Err(error) = spawn_result {
            log::error!("Failed to start capture action: {error:?}");
            set_capture_toolbar_status(&toolbar, format!("Error: {error:?}"));
            let _ = toolbar.show();
        }
    });
}

pub(crate) fn rgba_to_bgra_bytes(image: &image::RgbaImage) -> Vec<u8> {
    let mut bytes = image.as_raw().clone();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    bytes
}

pub(crate) fn compose_ocr_clipboard(original: &str, translated: Option<&str>) -> String {
    let original = clean_text(original);
    match translated.map(clean_text).filter(|text| !text.is_empty()) {
        Some(translated) => format!("{original}\n\n{translated}"),
        None => original.to_string(),
    }
}

pub(crate) fn spawn_recording_clock(
    toolbar: slint::Weak<CaptureToolbarWindow>,
    state: Arc<Mutex<AppState>>,
) {
    std::thread::spawn(move || loop {
        let (recording, elapsed) = {
            let guard = state.lock().unwrap();
            (guard.recording, current_recording_elapsed(&guard))
        };
        if !recording {
            break;
        }
        let elapsed_text = format_elapsed(elapsed);
        let toolbar_weak = toolbar.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(toolbar) = toolbar_weak.upgrade() {
                toolbar.set_recording_elapsed(elapsed_text.into());
            }
        });
        std::thread::sleep(Duration::from_millis(250));
    });
}

pub(crate) async fn run_toolbar_action(
    action: SelectionPurpose,
    rect: capture::CaptureRect,
    target: Option<capture::WindowTarget>,
    prefetched_image: Option<image::RgbaImage>,
    show_recording_border: bool,
    recording_border: slint::Weak<RecordingBorderWindow>,
    main: slint::Weak<MainWindow>,
    toolbar: slint::Weak<CaptureToolbarWindow>,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http: reqwest::Client,
) {
    let Some(main) = main.upgrade() else {
        return;
    };
    let Some(toolbar) = toolbar.upgrade() else {
        return;
    };
    let show_progress_tooltip = matches!(
        action,
        SelectionPurpose::OcrTranslate | SelectionPurpose::Vlm
    );
    if show_progress_tooltip {
        toolbar.set_active_tooltip("thinking...".into());
        sync_capture_toolbar_size(&toolbar);
    }
    let recording_border_window = recording_border.upgrade();
    // Re-apply the native capture exclusion immediately before every toolbar action. This keeps
    // the toolbar out of full-screen captures and recordings even after Windows recreates or
    // changes the native window state.
    #[cfg(target_os = "windows")]
    configure_capture_toolbar_native_window(&toolbar);
    let configured_folder = {
        let mut state_guard = state.lock().unwrap();
        sync_capture_state(&mut state_guard, &main);
        state_guard.capture_folder.clone()
    };
    let result: Result<String> = async {
        match action {
            SelectionPurpose::Capture => {
                let image = if let Some(image) = prefetched_image {
                    image
                } else {
                    tokio::task::spawn_blocking(move || {
                        if let Some(target) = target {
                            capture::capture_window(target)
                        } else {
                            capture::capture_area(&rect, &None)
                        }
                    })
                    .await
                    .context("Capture worker stopped")??
                };
                let configured_folder = configured_folder.clone();
                let path = tokio::task::spawn_blocking(move || {
                    capture::save_png_and_copy_to(&image, Some(configured_folder.as_str()))
                })
                .await
                .context("Capture save worker stopped")??;
                Ok(format!("Saved and copied: {}", path.display()))
            }
            SelectionPurpose::ScrollCapture => {
                let target = target.context("A window target is required for scrolling capture")?;
                let image = tokio::task::spawn_blocking(move || capture::scrolling_capture(target))
                    .await
                    .context("Scrolling capture worker stopped")??;
                let configured_folder = configured_folder.clone();
                let path = tokio::task::spawn_blocking(move || {
                    capture::save_png_and_copy_to(&image, Some(configured_folder.as_str()))
                })
                .await
                .context("Scrolling capture save worker stopped")??;
                Ok(format!("Saved and copied: {}", path.display()))
            }
            SelectionPurpose::Record => {
                if recorder_slot.lock().unwrap().is_some() {
                    anyhow::bail!("A recording is already in progress");
                }
                let path = capture::unique_output_path_in("mp4", Some(configured_folder.as_str()))?;
                let recording_rect = capture::CaptureRect {
                    width: rect.width & !1,
                    height: rect.height & !1,
                    ..rect
                };
                if show_recording_border {
                    if let Some(border) = recording_border_window.as_ref() {
                        display_recording_border(border, recording_rect);
                    }
                }
                let recorder =
                    match capture::ScreenRecorder::start(recording_rect, path.clone(), 30) {
                        Ok(recorder) => recorder,
                        Err(error) => {
                            if let Some(border) = recording_border_window.as_ref() {
                                let _ = border.hide();
                            }
                            return Err(error);
                        }
                    };
                {
                    let mut slot = recorder_slot.lock().unwrap();
                    *slot = Some(recorder);
                }
                {
                    let mut guard = state.lock().unwrap();
                    guard.recording = true;
                    guard.recording_paused = false;
                    guard.recording_started_at = Some(Instant::now());
                    guard.recording_paused_at = None;
                    guard.recording_paused_total = Duration::ZERO;
                    guard.recording_path = Some(path);
                }
                toolbar.set_recording(true);
                toolbar.set_recording_paused(false);
                toolbar.set_recording_elapsed("00:00".into());
                set_capture_toolbar_status(&toolbar, String::new());
                let _ = toolbar.show();
                spawn_recording_clock(toolbar.as_weak(), state);
                Ok("Recording started".to_string())
            }
            SelectionPurpose::Ocr | SelectionPurpose::OcrTranslate | SelectionPurpose::Vlm => {
                let image =
                    tokio::task::spawn_blocking(move || capture::capture_area(&rect, &None))
                        .await
                        .context("Capture worker stopped")??;
                if action == SelectionPurpose::Vlm {
                    let client = make_api_client(&http, &main);
                    let text = client.translate_image(&image).await?;
                    if text.trim().is_empty() {
                        anyhow::bail!("VLM returned no text");
                    }
                    capture::copy_text_to_clipboard(&text)?;
                    main.set_last_translated_text(clean_text(&text).into());
                    return Ok("VLM result copied to clipboard".to_string());
                }

                let width = image.width();
                let height = image.height();
                let pixels = rgba_to_bgra_bytes(&image);
                let recognized_text = tokio::task::spawn_blocking(move || {
                    ocr::recognize_text(&pixels, width, height)
                })
                .await
                .context("OCR worker stopped")??;
                let text = clean_text(&recognized_text);
                if text.trim().is_empty() {
                    return Ok("No text was recognized".to_string());
                }
                let translation = if action == SelectionPurpose::OcrTranslate {
                    Some(make_api_client(&http, &main).translate_text(&text).await?)
                } else {
                    None
                };
                let composed = compose_ocr_clipboard(&text, translation.as_deref());
                capture::copy_text_to_clipboard(&composed)?;
                if translation.is_some() {
                    Ok("OCR text and translation copied".to_string())
                } else {
                    Ok(format!(
                        "Recognized text copied ({} characters)",
                        text.trim().chars().count()
                    ))
                }
            }
            SelectionPurpose::ContinuousOcr
            | SelectionPurpose::ColorPicker
            | SelectionPurpose::Ruler => {
                anyhow::bail!("This action requires a different selection flow")
            }
        }
    }
    .await;

    if action == SelectionPurpose::Record {
        if let Err(error) = result {
            if let Some(border) = recording_border_window.as_ref() {
                let _ = border.hide();
            }
            toolbar.set_recording(false);
            set_capture_toolbar_status(&toolbar, format!("Error: {error}"));
            let _ = toolbar.show();
        }
        return;
    }
    match result {
        Ok(message) => set_capture_toolbar_status(&toolbar, message),
        Err(error) => set_capture_toolbar_status(&toolbar, format!("Error: {error}")),
    }
    if show_progress_tooltip {
        toolbar.set_active_tooltip("done".into());
        sync_capture_toolbar_size(&toolbar);
    }
    toolbar.set_recording(false);
    let _ = toolbar.show();
}

#[cfg(test)]
mod tests {
    use super::{capture_frame_rect_from_geometry, selection_magnifier_pixels};
    use crate::state::AppState;
    use image::{Rgba, RgbaImage};
    use std::sync::Arc;

    #[test]
    fn magnifier_contains_nine_by_nine_pixels_centered_on_cursor() {
        let source = RgbaImage::from_fn(11, 11, |x, y| Rgba([x as u8, y as u8, 0, 255]));
        let state = AppState {
            selection_scale: 1.0,
            selection_screenshot: Some(Arc::new(source)),
            ..Default::default()
        };

        let preview = selection_magnifier_pixels(&state, 5.0, 5.0).unwrap();

        assert_eq!(preview.dimensions(), (9, 9));
        assert_eq!(*preview.get_pixel(0, 0), Rgba([1, 1, 0, 255]));
        assert_eq!(*preview.get_pixel(4, 4), Rgba([5, 5, 0, 255]));
        assert_eq!(*preview.get_pixel(8, 8), Rgba([9, 9, 0, 255]));
    }

    #[test]
    fn magnifier_keeps_cursor_at_center_near_screen_edge() {
        let source = RgbaImage::from_fn(5, 5, |x, y| Rgba([x as u8, y as u8, 0, 255]));
        let state = AppState {
            selection_scale: 1.0,
            selection_screenshot: Some(Arc::new(source)),
            ..Default::default()
        };

        let preview = selection_magnifier_pixels(&state, 0.0, 0.0).unwrap();

        assert_eq!(*preview.get_pixel(4, 4), Rgba([0, 0, 0, 255]));
        assert_eq!(*preview.get_pixel(0, 0), Rgba([15, 23, 42, 255]));
        assert_eq!(*preview.get_pixel(8, 8), Rgba([4, 4, 0, 255]));
    }

    #[test]
    fn capture_frame_rect_excludes_header_and_border_at_standard_scale() {
        let rect = capture_frame_rect_from_geometry(100, 200, 720, 450, 1.0).unwrap();

        assert_eq!(rect.x, 103);
        assert_eq!(rect.y, 242);
        assert_eq!(rect.width, 714);
        assert_eq!(rect.height, 405);
    }

    #[test]
    fn capture_frame_rect_uses_physical_offsets_at_high_dpi() {
        let rect = capture_frame_rect_from_geometry(200, 300, 1080, 675, 1.5).unwrap();

        assert_eq!(rect.x, 205);
        assert_eq!(rect.y, 363);
        assert_eq!(rect.width, 1070);
        assert_eq!(rect.height, 607);
    }
}
