use crate::capture_workflow::*;
use crate::state::{AppState, SelectionPurpose};
use crate::{
    capture, win_utils, CaptureToolbarWindow, MainWindow, RecordingBorderWindow, SelectionWindow,
};
use global_hotkey::{hotkey::HotKey, GlobalHotKeyManager};
use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) fn register_callbacks(
    capture_toolbar: &CaptureToolbarWindow,
    main_window: &MainWindow,
    selection_window: &SelectionWindow,
    recording_border_window: &RecordingBorderWindow,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http_client: reqwest::Client,
    hotkey_manager: Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
    selection_initialized: Arc<Mutex<bool>>,
) {
    // Tooltips occupy the compact status row. Resize only after the hover event has returned;
    // changing a native window while Winit is dispatching pointer input can crash on Windows.
    let toolbar_weak_tooltip = capture_toolbar.as_weak();
    capture_toolbar.on_tooltip_visibility_changed(move |_tooltip| {
        let toolbar_weak = toolbar_weak_tooltip.clone();
        slint::Timer::single_shot(Duration::from_millis(16), move || {
            if let Some(toolbar) = toolbar_weak.upgrade() {
                sync_capture_toolbar_size(&toolbar);
            }
        });
    });

    let toolbar_weak_capture_mode = capture_toolbar.as_weak();
    capture_toolbar.on_capture_mode_clicked(move || {
        let toolbar_weak = toolbar_weak_capture_mode.clone();
        slint::Timer::single_shot(Duration::from_millis(16), move || {
            if let Some(toolbar) = toolbar_weak.upgrade() {
                toolbar.set_record_mode(false);
                toolbar.set_status_text(String::new().into());
                sync_capture_toolbar_size(&toolbar);
            }
        });
    });

    let toolbar_weak_record_mode = capture_toolbar.as_weak();
    capture_toolbar.on_record_mode_clicked(move || {
        let toolbar_weak = toolbar_weak_record_mode.clone();
        slint::Timer::single_shot(Duration::from_millis(16), move || {
            if let Some(toolbar) = toolbar_weak.upgrade() {
                toolbar.set_record_mode(true);
                toolbar.set_status_text(String::new().into());
                sync_capture_toolbar_size(&toolbar);
            }
        });
    });

    let toolbar_weak_fullscreen = capture_toolbar.as_weak();
    let main_weak_fullscreen = main_window.as_weak();
    let state_fullscreen = state.clone();
    let recorder_fullscreen = recorder_slot.clone();
    let http_fullscreen = http_client.clone();
    let recording_border_fullscreen = recording_border_window.as_weak();
    capture_toolbar.on_fullscreen_clicked(move || {
        if let (Some(toolbar), Some(main)) = (
            toolbar_weak_fullscreen.upgrade(),
            main_weak_fullscreen.upgrade(),
        ) {
            begin_fullscreen_toolbar_action(
                &toolbar,
                &main,
                state_fullscreen.clone(),
                recorder_fullscreen.clone(),
                http_fullscreen.clone(),
                recording_border_fullscreen.clone(),
            );
        }
    });

    let toolbar_weak_window = capture_toolbar.as_weak();
    let selection_weak_window = selection_window.as_weak();
    let state_window = state.clone();
    let selection_initialized_window = selection_initialized.clone();
    let hotkey_manager_window = hotkey_manager.clone();
    let esc_hotkey_window = esc_hotkey.clone();
    capture_toolbar.on_window_clicked(move || {
        if let Some(toolbar) = toolbar_weak_window.upgrade() {
            let purpose = if toolbar.get_record_mode() {
                SelectionPurpose::Record
            } else {
                SelectionPurpose::Capture
            };
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_window,
                &state_window,
                purpose,
                true,
                false,
                &selection_initialized_window,
                &hotkey_manager_window,
                esc_hotkey_window,
            );
        }
    });

    let toolbar_weak_scroll = capture_toolbar.as_weak();
    let selection_weak_scroll = selection_window.as_weak();
    let state_scroll = state.clone();
    let selection_initialized_scroll = selection_initialized.clone();
    let hotkey_manager_scroll = hotkey_manager.clone();
    let esc_hotkey_scroll = esc_hotkey.clone();
    capture_toolbar.on_scroll_clicked(move || {
        if let Some(toolbar) = toolbar_weak_scroll.upgrade() {
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_scroll,
                &state_scroll,
                SelectionPurpose::ScrollCapture,
                true,
                false,
                &selection_initialized_scroll,
                &hotkey_manager_scroll,
                esc_hotkey_scroll,
            );
        }
    });

    let toolbar_weak_region = capture_toolbar.as_weak();
    let selection_weak_region = selection_window.as_weak();
    let state_region = state.clone();
    let selection_initialized_region = selection_initialized.clone();
    let hotkey_manager_region = hotkey_manager.clone();
    let esc_hotkey_region = esc_hotkey.clone();
    capture_toolbar.on_region_clicked(move || {
        if let Some(toolbar) = toolbar_weak_region.upgrade() {
            let purpose = if toolbar.get_record_mode() {
                SelectionPurpose::Record
            } else {
                SelectionPurpose::Capture
            };
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_region,
                &state_region,
                purpose,
                false,
                false,
                &selection_initialized_region,
                &hotkey_manager_region,
                esc_hotkey_region,
            );
        }
    });

    let toolbar_weak_ocr = capture_toolbar.as_weak();
    let selection_weak_ocr = selection_window.as_weak();
    let state_ocr = state.clone();
    let selection_initialized_ocr = selection_initialized.clone();
    let hotkey_manager_ocr = hotkey_manager.clone();
    let esc_hotkey_ocr = esc_hotkey.clone();
    capture_toolbar.on_ocr_clicked(move || {
        if let Some(toolbar) = toolbar_weak_ocr.upgrade() {
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_ocr,
                &state_ocr,
                SelectionPurpose::Ocr,
                false,
                false,
                &selection_initialized_ocr,
                &hotkey_manager_ocr,
                esc_hotkey_ocr,
            );
        }
    });

    let toolbar_weak_translate = capture_toolbar.as_weak();
    let selection_weak_translate = selection_window.as_weak();
    let state_translate = state.clone();
    let selection_initialized_translate = selection_initialized.clone();
    let hotkey_manager_translate = hotkey_manager.clone();
    let esc_hotkey_translate = esc_hotkey.clone();
    capture_toolbar.on_translate_clicked(move || {
        if let Some(toolbar) = toolbar_weak_translate.upgrade() {
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_translate,
                &state_translate,
                SelectionPurpose::OcrTranslate,
                false,
                false,
                &selection_initialized_translate,
                &hotkey_manager_translate,
                esc_hotkey_translate,
            );
        }
    });

    let toolbar_weak_vlm = capture_toolbar.as_weak();
    let selection_weak_vlm = selection_window.as_weak();
    let state_vlm = state.clone();
    let selection_initialized_vlm = selection_initialized.clone();
    let hotkey_manager_vlm = hotkey_manager.clone();
    let esc_hotkey_vlm = esc_hotkey.clone();
    capture_toolbar.on_vlm_clicked(move || {
        if let Some(toolbar) = toolbar_weak_vlm.upgrade() {
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_vlm,
                &state_vlm,
                SelectionPurpose::Vlm,
                false,
                false,
                &selection_initialized_vlm,
                &hotkey_manager_vlm,
                esc_hotkey_vlm,
            );
        }
    });

    let toolbar_weak_color = capture_toolbar.as_weak();
    let selection_weak_color = selection_window.as_weak();
    let state_color = state.clone();
    let selection_initialized_color = selection_initialized.clone();
    let hotkey_manager_color = hotkey_manager.clone();
    let esc_hotkey_color = esc_hotkey.clone();
    capture_toolbar.on_color_picker_clicked(move || {
        if let Some(toolbar) = toolbar_weak_color.upgrade() {
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_color,
                &state_color,
                SelectionPurpose::ColorPicker,
                false,
                true,
                &selection_initialized_color,
                &hotkey_manager_color,
                esc_hotkey_color,
            );
        }
    });

    let toolbar_weak_ruler = capture_toolbar.as_weak();
    let selection_weak_ruler = selection_window.as_weak();
    let state_ruler = state.clone();
    let selection_initialized_ruler = selection_initialized.clone();
    let hotkey_manager_ruler = hotkey_manager.clone();
    let esc_hotkey_ruler = esc_hotkey.clone();
    capture_toolbar.on_ruler_clicked(move || {
        if let Some(toolbar) = toolbar_weak_ruler.upgrade() {
            begin_toolbar_selection(
                &toolbar,
                &selection_weak_ruler,
                &state_ruler,
                SelectionPurpose::Ruler,
                false,
                false,
                &selection_initialized_ruler,
                &hotkey_manager_ruler,
                esc_hotkey_ruler,
            );
        }
    });

    let toolbar_weak_drag = capture_toolbar.as_weak();
    capture_toolbar.on_drag_requested(move || {
        let Some(toolbar) = toolbar_weak_drag.upgrade() else {
            return;
        };

        // The move button is press-on-down, so its TouchArea is still handling the pointer when
        // this callback runs. Clear the tooltip now, then wait for the input turn to finish before
        // entering Win32's modal drag loop. The ToolbarButton binding cancels the TouchArea grab;
        // without that cancellation every later button would be treated as part of the move.
        toolbar.set_active_tooltip(String::new().into());
        let toolbar_weak = toolbar.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            let Some(toolbar) = toolbar_weak.upgrade() else {
                return;
            };
            sync_capture_toolbar_size(&toolbar);

            #[cfg(target_os = "windows")]
            toolbar.window().with_winit_window(|winit_window| {
                use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = winit_window.window_handle() {
                    if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                        win_utils::begin_window_drag(windows::Win32::Foundation::HWND(
                            handle.hwnd.get() as _,
                        ));
                    }
                }
            });

            // WM_NCLBUTTONDOWN consumes the release event, so keep the status row/height in sync
            // explicitly once the native drag loop has returned.
            toolbar.set_active_tooltip(String::new().into());
            sync_capture_toolbar_size(&toolbar);
        });
    });

    let toolbar_weak_pause = capture_toolbar.as_weak();
    let state_pause = state.clone();
    let recorder_pause = recorder_slot.clone();
    capture_toolbar.on_pause_recording_clicked(move || {
        let Some(toolbar) = toolbar_weak_pause.upgrade() else {
            return;
        };
        let paused = {
            let mut state = state_pause.lock().unwrap();
            if !state.recording {
                return;
            }
            if state.recording_paused {
                if let Some(paused_at) = state.recording_paused_at.take() {
                    state.recording_paused_total += paused_at.elapsed();
                }
                state.recording_paused = false;
                false
            } else {
                state.recording_paused_at = Some(Instant::now());
                state.recording_paused = true;
                true
            }
        };
        if let Some(recorder) = recorder_pause.lock().unwrap().as_ref() {
            recorder.set_paused(paused);
        }
        toolbar.set_recording_paused(paused);
    });

    let toolbar_weak_stop_recording = capture_toolbar.as_weak();
    let state_stop_recording = state.clone();
    let recorder_stop_recording = recorder_slot.clone();
    let recording_border_stop_recording = recording_border_window.as_weak();
    capture_toolbar.on_stop_recording_clicked(move || {
        let Some(toolbar) = toolbar_weak_stop_recording.upgrade() else {
            return;
        };
        let Some(recorder) = recorder_stop_recording.lock().unwrap().take() else {
            return;
        };
        let (path, was_recording) = {
            let mut state = state_stop_recording.lock().unwrap();
            let path = state.recording_path.clone();
            let was_recording = state.recording;
            state.recording = false;
            state.recording_paused = false;
            state.recording_started_at = None;
            state.recording_paused_at = None;
            state.recording_paused_total = Duration::ZERO;
            (path, was_recording)
        };
        if !was_recording {
            let _ = recorder.stop();
            return;
        }
        if let Some(border) = recording_border_stop_recording.upgrade() {
            let _ = border.hide();
        }
        toolbar.set_recording(false);
        toolbar.set_recording_paused(false);
        let result = recorder.stop();
        match result {
            Ok(()) => {
                let message = path
                    .map(|path| format!("Recording saved: {}", path.display()))
                    .unwrap_or_else(|| "Recording saved".to_string());
                set_capture_toolbar_status(&toolbar, message);
            }
            Err(error) => set_capture_toolbar_status(&toolbar, format!("Error: {error}")),
        }
        let _ = toolbar.show();
    });

    let recorder_slot_toolbar_close = recorder_slot.clone();
    let recording_border_toolbar_close = recording_border_window.as_weak();
    capture_toolbar.on_close_clicked(move || {
        shutdown_and_exit(
            &recorder_slot_toolbar_close,
            &recording_border_toolbar_close,
        );
    });
    let recorder_slot_toolbar_window_close = recorder_slot.clone();
    let recording_border_toolbar_window_close = recording_border_window.as_weak();
    capture_toolbar.window().on_close_requested(move || {
        shutdown_and_exit(
            &recorder_slot_toolbar_window_close,
            &recording_border_toolbar_window_close,
        );
    });
}
