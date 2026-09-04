use crate::{
    capture, win_utils, CaptureFrameWindow, CaptureToolbarWindow, MainWindow, OverlayWindow,
    RecordingBorderWindow, SelectionWindow, TextboxWindow,
};

use anyhow::Result;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyManager,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i_slint_backend_winit::WinitWindowAccessor;

use crate::state::AppState;

use crate::capture_workflow::*;

pub(crate) async fn run() -> Result<()> {
    env_logger::init();

    let main_window = MainWindow::new()?;
    let overlay_window = OverlayWindow::new()?;
    let selection_window = SelectionWindow::new()?;
    let textbox_window = TextboxWindow::new()?;
    let capture_toolbar = CaptureToolbarWindow::new()?;
    let capture_frame_window = CaptureFrameWindow::new()?;
    let recording_border_window = RecordingBorderWindow::new()?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to build HTTP client");

    let startup = crate::startup::initialize_ui(
        &main_window,
        &textbox_window,
        &capture_toolbar,
        &http_client,
    );
    let initial_capture_folder = startup.capture_folder;
    let initial_dark_theme = startup.dark_theme;
    let initial_app_mode = startup.app_mode;
    capture_frame_window.set_dark_theme(initial_dark_theme);

    let state = Arc::new(Mutex::new(AppState {
        api_endpoint: main_window.get_api_endpoint().to_string(),
        api_key: main_window.get_api_key().to_string(),
        model_name: main_window.get_model_name().to_string(),
        interval_sec: 0.0,
        system_prompt: main_window.get_system_prompt().to_string(),
        last_text: String::new(),
        base_font_size: main_window.get_base_font_size(),
        overlay_bg_color: main_window.get_overlay_bg_color(),
        overlay_text_color: main_window.get_overlay_text_color(),
        overlay_bg_opacity: main_window.get_overlay_bg_opacity(),
        temperature: main_window.get_temperature(),
        thinking_level: main_window.get_thinking_level().to_string(),
        provider: main_window.get_api_type().to_string(),
        use_textbox: main_window.get_use_textbox(),
        capture_folder: initial_capture_folder,
        selection_scale: 1.0,
        ..Default::default()
    }));

    // Global Hotkey Setup - Initialize safely without panicking on failure
    let hotkey_manager = GlobalHotKeyManager::new().ok().map(Arc::new);

    let hotkey_capture = HotKey::new(Some(Modifiers::META | Modifiers::ALT), Code::KeyA);
    let hotkey_start_stop = HotKey::new(Some(Modifiers::META | Modifiers::ALT), Code::KeyP);
    let esc_hotkey = HotKey::new(None, Code::Escape);

    if let Some(ref mgr) = hotkey_manager {
        if let Err(e) = mgr.register(hotkey_capture) {
            log::error!("Failed to register capture hotkey: {:?}", e);
        }
        if let Err(e) = mgr.register(hotkey_start_stop) {
            log::error!("Failed to register start/stop hotkey: {:?}", e);
        }
    }

    // Setup Transparency and Windows Specifics
    // Setup Transparency and Windows Specifics
    #[cfg(target_os = "windows")]
    let main_hwnd = {
        let mut hwnd_out = None;
        main_window.window().with_winit_window(|winit_window| {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = winit_window.window_handle() {
                if let RawWindowHandle::Win32(h) = handle.as_raw() {
                    let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                    win_utils::set_mica_backdrop(hwnd);
                    win_utils::set_title_bar_theme(hwnd, initial_dark_theme);
                    hwnd_out = Some(hwnd);
                }
            }
        });
        hwnd_out
    };
    #[cfg(not(target_os = "windows"))]
    let main_hwnd: Option<()> = None;

    // The capture toolbar's native handle is configured after its first show. Winit creates
    // native windows lazily, so an accessor call here (before the event loop starts) is a no-op.

    #[cfg(target_os = "windows")]
    let folder_owner = main_hwnd.map(|hwnd| hwnd.0 as isize);
    #[cfg(not(target_os = "windows"))]
    let folder_owner: Option<isize> = None;

    let selection_initialized = crate::window_mode::register_callbacks(
        &main_window,
        &capture_toolbar,
        &capture_frame_window,
        &overlay_window,
        &textbox_window,
        state.clone(),
        folder_owner,
        initial_dark_theme,
        &initial_app_mode,
    );

    let recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>> = Arc::new(Mutex::new(None));

    crate::toolbar_ui::register_callbacks(
        &capture_toolbar,
        &main_window,
        &selection_window,
        &capture_frame_window,
        &recording_border_window,
        state.clone(),
        recorder_slot.clone(),
        http_client.clone(),
        hotkey_manager.clone(),
        esc_hotkey,
        selection_initialized.clone(),
    );

    crate::provider_ui::register_callbacks(&main_window, state.clone(), http_client.clone());

    crate::ocr_ui::register_callbacks(
        &main_window,
        &overlay_window,
        &selection_window,
        &textbox_window,
        state.clone(),
        hotkey_manager.clone(),
        esc_hotkey,
        selection_initialized.clone(),
        main_hwnd,
    );

    crate::selection_ui::register_callbacks(
        &main_window,
        &overlay_window,
        &selection_window,
        &textbox_window,
        &capture_toolbar,
        &recording_border_window,
        state.clone(),
        recorder_slot.clone(),
        http_client.clone(),
        hotkey_manager.clone(),
        esc_hotkey,
        main_hwnd,
    );

    crate::runtime_workers::start(
        &main_window,
        &overlay_window,
        &selection_window,
        &textbox_window,
        state,
        http_client,
        hotkey_capture,
        hotkey_start_stop,
        esc_hotkey,
    );

    if let Err(error) = main_window.run() {
        log::error!("OCR Translator event loop stopped: {error:?}");
    }
    let recording_border_exit = recording_border_window.as_weak();
    shutdown_and_exit(&recorder_slot, &recording_border_exit);
}
