use crate::capture_workflow::{
    configure_main_window_native_theme, schedule_main_window_native_theme,
    show_capture_toolbar_at_top_center, sync_ocr_window_size,
};
use crate::settings::{save_app_mode, save_capture_folder, save_dark_theme, save_system_prompt};
use crate::state::AppState;
use crate::{
    win_utils, CaptureFrameWindow, CaptureToolbarWindow, MainWindow, OverlayWindow, TextboxWindow,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn register_callbacks(
    main_window: &MainWindow,
    capture_toolbar: &CaptureToolbarWindow,
    capture_frame_window: &CaptureFrameWindow,
    overlay_window: &OverlayWindow,
    textbox_window: &TextboxWindow,
    state: Arc<Mutex<AppState>>,
    folder_owner: Option<isize>,
    initial_dark_theme: bool,
    initial_app_mode: &str,
) -> Arc<Mutex<bool>> {
    let state_folder_changed = state.clone();
    main_window.on_capture_folder_changed(move |folder| {
        let folder = folder.to_string();
        save_capture_folder(&folder);
        if let Ok(mut state) = state_folder_changed.lock() {
            state.capture_folder = folder;
        }
    });

    main_window.on_system_prompt_changed(move |prompt| {
        save_system_prompt(prompt.as_str());
    });

    // Resize only when the style panel state changes. Both states remain fixed-height so the
    // native window cannot be freely resized, while the expanded state has room for its controls.
    let main_weak_style_panel = main_window.as_weak();
    main_window.on_style_panel_toggled(move |_is_open| {
        if let Some(main) = main_weak_style_panel.upgrade() {
            sync_ocr_window_size(&main);
        }
    });

    let main_weak_folder = main_window.as_weak();
    let state_folder = state.clone();
    main_window.on_folder_select_clicked(move || {
        let Some(folder) = win_utils::pick_folder(folder_owner) else {
            return;
        };
        let folder = folder.to_string_lossy().to_string();
        save_capture_folder(&folder);
        if let Ok(mut state) = state_folder.lock() {
            state.capture_folder = folder.clone();
        }
        if let Some(main) = main_weak_folder.upgrade() {
            main.set_capture_folder(folder.into());
        }
    });

    // Initial Selection Window Styles Setup
    let selection_initialized = Arc::new(Mutex::new(false));

    // Keep the OCR window's theme switch in sync with the compact capture toolbar. The native
    // title bar is updated here as well because changing Slint's client-area colors alone does
    // not update Windows' non-client area.
    let main_weak_main_theme = main_window.as_weak();
    let toolbar_weak_main_theme = capture_toolbar.as_weak();
    let frame_weak_main_theme = capture_frame_window.as_weak();
    main_window.on_theme_toggle_clicked(move || {
        let Some(main) = main_weak_main_theme.upgrade() else {
            return;
        };
        let dark_theme = !main.get_dark_theme();
        main.set_dark_theme(dark_theme);
        if let Some(toolbar) = toolbar_weak_main_theme.upgrade() {
            toolbar.set_dark_theme(dark_theme);
        }
        if let Some(frame) = frame_weak_main_theme.upgrade() {
            frame.set_dark_theme(dark_theme);
        }
        #[cfg(target_os = "windows")]
        {
            configure_main_window_native_theme(&main, dark_theme);
            schedule_main_window_native_theme(main.as_weak(), dark_theme, 0);
        }
        save_dark_theme(dark_theme);
    });

    // Apply the persisted title-bar theme repeatedly while Winit creates and activates the native
    // OCR window. The HWND may not exist during the initial setup phase.
    #[cfg(target_os = "windows")]
    schedule_main_window_native_theme(main_window.as_weak(), initial_dark_theme, 0);

    // Main window <-> compact capture toolbar mode switch. OCR mode keeps the original
    // settings UI; Capture mode hides it and reveals the AIMediaWorker-style toolbar.
    let main_weak_mode = main_window.as_weak();
    let toolbar_weak_mode = capture_toolbar.as_weak();
    let overlay_weak_mode = overlay_window.as_weak();
    let textbox_weak_mode = textbox_window.as_weak();
    let state_mode = state.clone();
    main_window.on_mode_toggle_clicked(move |mode| {
        let Some(main) = main_weak_mode.upgrade() else {
            return;
        };
        save_app_mode(mode.as_str());
        if mode == "capture" {
            {
                let mut state = state_mode.lock().unwrap();
                state.is_running = false;
            }
            main.set_is_running(false);
            main.set_overlay_visible(false);
            if let Some(overlay) = overlay_weak_mode.upgrade() {
                let _ = overlay.hide();
            }
            if let Some(textbox) = textbox_weak_mode.upgrade() {
                let _ = textbox.hide();
            }
            main.set_app_mode("capture".into());
            let main_weak_switch = main.as_weak();
            let toolbar_weak_switch = toolbar_weak_mode.clone();
            // Defer the native show/hide pair until this UI callback has returned. On some
            // Windows/Winit combinations, changing the window set re-entrantly from a click
            // handler can tear down the event loop and look like a crash.
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                let Some(toolbar) = toolbar_weak_switch.upgrade() else {
                    return;
                };
                if show_capture_toolbar_at_top_center(&toolbar) {
                    // Keep at least one application window visible during the switch.
                    if let Some(main) = main_weak_switch.upgrade() {
                        let _ = main.hide();
                    }
                }
            });
        } else {
            main.set_app_mode("ocr".into());
            let main_weak_switch = main.as_weak();
            let toolbar_weak_switch = toolbar_weak_mode.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(main) = main_weak_switch.upgrade() {
                    let _ = main.show();
                    #[cfg(target_os = "windows")]
                    {
                        let dark_theme = main.get_dark_theme();
                        configure_main_window_native_theme(&main, dark_theme);
                        schedule_main_window_native_theme(main.as_weak(), dark_theme, 0);
                    }
                }
                if let Some(toolbar) = toolbar_weak_switch.upgrade() {
                    let _ = toolbar.hide();
                }
            });
        }
    });

    // The capture toolbar owns the theme switch so the compact capture UI can change both
    // windows while the OCR settings window is hidden.
    let main_weak_toolbar_theme = main_window.as_weak();
    let toolbar_weak_toolbar_theme = capture_toolbar.as_weak();
    let frame_weak_toolbar_theme = capture_frame_window.as_weak();
    capture_toolbar.on_theme_toggle_clicked(move || {
        let Some(toolbar) = toolbar_weak_toolbar_theme.upgrade() else {
            return;
        };
        let dark_theme = !toolbar.get_dark_theme();
        toolbar.set_dark_theme(dark_theme);
        if let Some(frame) = frame_weak_toolbar_theme.upgrade() {
            frame.set_dark_theme(dark_theme);
        }
        if let Some(main) = main_weak_toolbar_theme.upgrade() {
            main.set_dark_theme(dark_theme);
            #[cfg(target_os = "windows")]
            {
                configure_main_window_native_theme(&main, dark_theme);
                schedule_main_window_native_theme(main.as_weak(), dark_theme, 0);
            }
        }
        save_dark_theme(dark_theme);
    });

    // The toolbar's small UI button is the reverse path back to the full OCR settings UI.
    let main_weak_toolbar_ui = main_window.as_weak();
    let toolbar_weak_toolbar_ui = capture_toolbar.as_weak();
    let frame_weak_toolbar_ui = capture_frame_window.as_weak();
    main_window.set_app_mode(initial_app_mode.into());

    capture_toolbar.on_ui_toggle_clicked(move || {
        // A toolbar button is part of the window currently dispatching the pointer event.
        // Deferring the native show/hide pair avoids changing that window set re-entrantly,
        // which can terminate the Winit event loop on Windows.
        let main_weak = main_weak_toolbar_ui.clone();
        let toolbar_weak = toolbar_weak_toolbar_ui.clone();
        let frame_weak = frame_weak_toolbar_ui.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            if let Some(toolbar) = toolbar_weak.upgrade() {
                if toolbar.get_recording() {
                    return;
                }
            }
            let Some(main) = main_weak.upgrade() else {
                return;
            };
            main.set_app_mode("ocr".into());
            if main.show().is_err() {
                return;
            }
            #[cfg(target_os = "windows")]
            {
                let dark_theme = main.get_dark_theme();
                configure_main_window_native_theme(&main, dark_theme);
                schedule_main_window_native_theme(main.as_weak(), dark_theme, 0);
            }
            save_app_mode("ocr");
            if let Some(toolbar) = toolbar_weak.upgrade() {
                let _ = toolbar.hide();
                toolbar.set_frame_mode(false);
            }
            if let Some(frame) = frame_weak.upgrade() {
                let _ = frame.hide();
            }
        });
    });

    // Restore the last selected mode after the event loop has created the native windows.
    // Showing the toolbar before hiding the main window also avoids a transient no-window state.
    if initial_app_mode == "capture" {
        let main_weak_restore = main_window.as_weak();
        let toolbar_weak_restore = capture_toolbar.as_weak();
        slint::Timer::single_shot(Duration::from_millis(0), move || {
            if let (Some(main), Some(toolbar)) =
                (main_weak_restore.upgrade(), toolbar_weak_restore.upgrade())
            {
                if show_capture_toolbar_at_top_center(&toolbar) {
                    let _ = main.hide();
                }
            }
        });
    } else {
        // Explicitly materialize the OCR window before entering the event loop. This gives the
        // native title-bar attributes a real HWND on the very first launch as well as on mode
        // switches from the compact toolbar.
        let _ = main_window.show();
        #[cfg(target_os = "windows")]
        schedule_main_window_native_theme(main_window.as_weak(), initial_dark_theme, 0);
    }

    selection_initialized
}
