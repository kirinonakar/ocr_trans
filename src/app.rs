use crate::{
    capture, CaptureFrameWindow, CaptureToolbarWindow, MainWindow, OverlayWindow,
    RecordingBorderWindow, SelectionWindow, ShortcutSettingsWindow, TextboxWindow,
};

use anyhow::Result;
use global_hotkey::{
    hotkey::{Code, HotKey},
    GlobalHotKeyManager,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::state::AppState;

use crate::capture_workflow::*;

pub(crate) async fn run(
    instance: crate::lifecycle::NativeHost,
    start_in_tray: bool,
    hotkey_manager: Option<Arc<GlobalHotKeyManager>>,
    hotkey_state: Arc<Mutex<crate::shortcuts::HotkeyState>>,
) -> Result<()> {
    let main_window = MainWindow::new()?;
    let about_window = crate::AboutWindow::new()?;
    crate::about_ui::register_callbacks(&main_window, &about_window);
    let overlay_window = OverlayWindow::new()?;
    let selection_window = SelectionWindow::new()?;
    let textbox_window = TextboxWindow::new()?;
    crate::markdown::initialize(&textbox_window);
    let capture_toolbar = CaptureToolbarWindow::new()?;
    let capture_frame_window = CaptureFrameWindow::new()?;
    let recording_border_window = RecordingBorderWindow::new()?;
    let shortcut_window = ShortcutSettingsWindow::new()?;

    let http_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to build HTTP client");

    let startup = crate::startup::initialize_ui(
        &main_window,
        &textbox_window,
        &capture_toolbar,
        &http_client,
    );
    let initial_capture_folder = startup.capture_folder;
    overlay_window.set_result_font_family(main_window.get_result_font_family());
    let initial_dark_theme = startup.dark_theme;
    let initial_app_mode = startup.app_mode;
    capture_frame_window.set_dark_theme(initial_dark_theme);
    shortcut_window.set_dark_theme(initial_dark_theme);

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

    let esc_hotkey = HotKey::new(None, Code::Escape);

    let selection_initialized = crate::window_mode::register_callbacks(
        &main_window,
        &capture_toolbar,
        &capture_frame_window,
        &textbox_window,
        &shortcut_window,
        state.clone(),
        initial_dark_theme,
        &initial_app_mode,
        start_in_tray,
    );

    let recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>> = Arc::new(Mutex::new(None));
    let magnifier = crate::magnifier::WindowMagnifier::new(&capture_toolbar, state.clone())?;
    magnifier.register_recording(
        &main_window, &recording_border_window, &textbox_window,
        recorder_slot.clone(), http_client.clone(),
    );

    crate::toolbar_ui::register_callbacks(
        &capture_toolbar,
        &main_window,
        &selection_window,
        &capture_frame_window,
        &recording_border_window,
        &textbox_window,
        state.clone(),
        recorder_slot.clone(),
        http_client.clone(),
        hotkey_manager.clone(),
        esc_hotkey,
        selection_initialized.clone(),
        &magnifier,
    );

    crate::provider_ui::register_callbacks(&main_window, state.clone(), http_client.clone());

    crate::ocr_ui::register_callbacks(
        &main_window,
        &overlay_window,
        &selection_window,
        &textbox_window,
        &capture_toolbar,
        state.clone(),
        hotkey_manager.clone(),
        esc_hotkey,
        selection_initialized.clone(),
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
        magnifier,
    );

    crate::runtime_workers::start(
        &main_window,
        &overlay_window,
        &selection_window,
        &textbox_window,
        &capture_toolbar,
        state,
        http_client,
        hotkey_state.clone(),
        esc_hotkey,
    );

    crate::shortcuts_ui::register_callbacks(
        &main_window,
        &shortcut_window,
        &capture_toolbar,
        hotkey_manager.clone(),
        hotkey_state,
    );

    let _lifecycle = crate::lifecycle::register_callbacks(
        instance,
        &main_window,
        &capture_toolbar,
        &capture_frame_window,
        &shortcut_window,
        recorder_slot.clone(),
        &recording_border_window,
    );

    // Keep the translation overlay above applications that run in fullscreen mode. The guard
    // re-applies the topmost window band while another process owns a fullscreen foreground
    // window, and the timer stays alive until the event loop exits.
    #[cfg(target_os = "windows")]
    let _overlay_topmost_guard =
        start_overlay_topmost_guard(overlay_window.as_weak(), textbox_window.as_weak());

    if !start_in_tray {
        main_window.show()?;
    }
    // Closing to tray leaves no visible Slint windows. Only an explicit exit should end the loop.
    if let Err(error) = slint::run_event_loop_until_quit() {
        log::error!("OCR Translator event loop stopped: {error:?}");
    }
    let recording_border_exit = recording_border_window.as_weak();
    shutdown_and_exit(&recorder_slot, &recording_border_exit);
}
