use crate::capture_workflow::*;
use crate::settings::*;
use crate::state::{AppState, SelectionPurpose};
use crate::text_layout::*;
use crate::{
    capture, win_utils, CaptureToolbarWindow, MainWindow, OverlayWindow, RecordingBorderWindow,
    SelectionWindow, TextboxWindow,
};
use anyhow::Context;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyManager};
use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn register_callbacks(
    main_window: &MainWindow,
    overlay_window: &OverlayWindow,
    selection_window: &SelectionWindow,
    textbox_window: &TextboxWindow,
    capture_toolbar: &CaptureToolbarWindow,
    recording_border_window: &RecordingBorderWindow,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http_client: reqwest::Client,
    hotkey_manager: Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
    #[cfg(target_os = "windows")] main_hwnd: Option<windows::Win32::Foundation::HWND>,
    #[cfg(not(target_os = "windows"))] main_hwnd: Option<()>,
) {
    let selection_weak = selection_window.as_weak();
    let overlay_weak = overlay_window.as_weak();
    let textbox_weak = textbox_window.as_weak();
    // Close Requested - Hard Exit
    let recorder_slot_main_close = recorder_slot.clone();
    let recording_border_main_close = recording_border_window.as_weak();
    main_window.window().on_close_requested(move || {
        shutdown_and_exit(&recorder_slot_main_close, &recording_border_main_close);
    });

    let main_weak_for_selection = main_window.as_weak();
    let selection_weak_for_close = selection_window.as_weak();
    let hotkey_manager_for_close = hotkey_manager.clone();
    let esc_hotkey_for_close = esc_hotkey.clone();
    let state_for_selection_close = state.clone();
    let main_weak_for_selection_close = main_weak_for_selection.clone();
    let toolbar_weak_for_selection_close = capture_toolbar.as_weak();
    selection_window.on_closed(move || {
        // Keep native show/hide calls out of the key event that requested Escape. In capture mode
        // the toolbar is hidden while selecting, so the deferred callback also restores it before
        // hiding the selector; otherwise the last visible window would disappear and stop Slint's
        // event loop.
        let selection_weak = selection_weak_for_close.clone();
        let main_weak = main_weak_for_selection_close.clone();
        let toolbar_weak = toolbar_weak_for_selection_close.clone();
        let state = state_for_selection_close.clone();
        let hotkey_manager = hotkey_manager_for_close.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            let capture_mode = main_weak
                .upgrade()
                .map(|main| main.get_app_mode() == "capture")
                .unwrap_or(false);
            let toolbar_ready = if capture_mode {
                toolbar_weak
                    .upgrade()
                    .map(|toolbar| {
                        if toolbar.get_recording() {
                            true
                        } else {
                            toolbar.show().is_ok()
                        }
                    })
                    .unwrap_or(false)
            } else {
                true
            };

            if toolbar_ready {
                if let Some(selection) = selection_weak.upgrade() {
                    let _ = selection.hide();
                }
            } else {
                log::warn!("Unable to restore capture toolbar after closing selection window");
            }
            if let Ok(mut state) = state.lock() {
                state.pending_selection = None;
            }
            if let Some(ref manager) = hotkey_manager {
                let _ = manager.unregister(esc_hotkey_for_close);
            }
        });
    });

    let selection_weak_area_updated = selection_window.as_weak();
    let state_area_updated = state.clone();
    selection_window.on_area_updated(move |x, y, w, h| {
        let rect = {
            let state = state_area_updated.lock().unwrap();
            if state.pending_selection != Some(SelectionPurpose::Ruler) {
                return;
            }
            physical_selection_rect(&state, x, y, w, h)
        };
        if let Some(selection) = selection_weak_area_updated.upgrade() {
            let (line_1, line_2) = ruler_selection_lines(rect);
            selection.set_ruler_info_line_1(line_1.into());
            selection.set_ruler_info_line_2(line_2.into());
        }
    });

    let selection_weak_magnifier = selection_window.as_weak();
    let state_magnifier = state.clone();
    selection_window.on_selection_hovered(move |x, y| {
        let magnifier = {
            let state = state_magnifier.lock().unwrap();
            selection_magnifier_at(&state, x, y)
        };
        if let Some(selection) = selection_weak_magnifier.upgrade() {
            if let Some(image) = magnifier {
                selection.set_selection_magnifier(image);
                selection.set_magnifier_visible(true);
            } else {
                selection.set_magnifier_visible(false);
            }
        }
    });

    let state_for_selection = state.clone();
    let hotkey_manager_area = hotkey_manager.clone();
    let esc_hotkey_area = esc_hotkey.clone();
    let textbox_weak_for_area = textbox_weak.clone();
    let toolbar_weak_for_selection_actions = capture_toolbar.as_weak();
    let recorder_slot_for_selection_actions = recorder_slot.clone();
    let http_for_selection_actions = http_client.clone();
    let recording_border_for_selection_actions = recording_border_window.as_weak();
    selection_window.on_area_selected(move |x, y, w, h| {
        let textbox_weak = textbox_weak_for_area.clone();
        let Some(selection) = selection_weak.upgrade() else {
            if let Some(toolbar) = toolbar_weak_for_selection_actions.upgrade() {
                let _ = toolbar.show();
            }
            return;
        };

        let pending = state_for_selection.lock().unwrap().pending_selection;
        if let Some(purpose) = pending.filter(|purpose| *purpose != SelectionPurpose::ContinuousOcr)
        {
            let minimum_size = if purpose == SelectionPurpose::Ruler {
                1.0
            } else {
                5.0
            };
            if w < minimum_size || h < minimum_size {
                let selection_weak = selection.as_weak();
                let toolbar_weak = toolbar_weak_for_selection_actions.clone();
                let state = state_for_selection.clone();
                let hotkey_manager = hotkey_manager_area.clone();
                slint::Timer::single_shot(Duration::from_millis(1), move || {
                    // Restore the toolbar before hiding the selector so capture mode keeps one
                    // visible window and its event loop alive.
                    if let Some(toolbar) = toolbar_weak.upgrade() {
                        let _ = toolbar.show();
                    }
                    if let Some(selection) = selection_weak.upgrade() {
                        let _ = selection.hide();
                    }
                    if let Some(ref manager) = hotkey_manager {
                        let _ = manager.unregister(esc_hotkey_area);
                    }
                    if let Ok(mut state) = state.lock() {
                        state.pending_selection = None;
                    }
                });
                return;
            }

            let rect = {
                let mut state = state_for_selection.lock().unwrap();
                state.pending_selection = None;
                physical_selection_rect(&state, x, y, w, h)
            };
            let selection_weak = selection.as_weak();
            let toolbar_weak = toolbar_weak_for_selection_actions.clone();
            let main_weak = main_weak_for_selection.clone();
            let state = state_for_selection.clone();
            let recorder_slot = recorder_slot_for_selection_actions.clone();
            let http = http_for_selection_actions.clone();
            let recording_border = recording_border_for_selection_actions.clone();
            let hotkey_manager = hotkey_manager_area.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                // Showing the toolbar first prevents hiding the selector from shutting down the
                // app when capture mode has no other visible window.
                let toolbar = toolbar_weak.upgrade();
                if let Some(toolbar) = &toolbar {
                    let _ = toolbar.show();
                }
                if let Some(selection) = selection_weak.upgrade() {
                    let _ = selection.hide();
                }
                if let Some(ref manager) = hotkey_manager {
                    let _ = manager.unregister(esc_hotkey_area);
                }

                if purpose == SelectionPurpose::ColorPicker {
                    if let Some(toolbar) = toolbar {
                        set_capture_toolbar_status(
                            &toolbar,
                            "Click a pixel to choose a color".to_string(),
                        );
                    }
                    return;
                }

                if purpose == SelectionPurpose::Ruler {
                    let Some(toolbar) = toolbar else {
                        return;
                    };
                    let tooltip = ruler_toolbar_tooltip(rect);
                    let clipboard_text = ruler_clipboard_text(rect);
                    let _ = slint::spawn_local(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            capture::copy_text_to_clipboard(&clipboard_text)?;
                            Ok::<String, anyhow::Error>(tooltip)
                        })
                        .await
                        .context("Ruler clipboard worker stopped")
                        .and_then(|result| result);
                        match result {
                            Ok(measurement) => {
                                toolbar.set_ruler_tooltip(measurement.clone().into());
                                toolbar.set_active_tooltip(measurement.into());
                                set_capture_toolbar_status(
                                    &toolbar,
                                    "Copied region coordinates and dimensions".to_string(),
                                );
                            }
                            Err(error) => {
                                set_capture_toolbar_status(&toolbar, format!("Error: {error}"));
                            }
                        }
                    });
                    return;
                }

                if let (Some(main), Some(toolbar)) = (main_weak.upgrade(), toolbar) {
                    if let Err(error) = slint::spawn_local(run_toolbar_action(
                        purpose,
                        rect,
                        None,
                        None,
                        purpose == SelectionPurpose::Record,
                        recording_border,
                        main.as_weak(),
                        toolbar.as_weak(),
                        state,
                        recorder_slot,
                        http,
                    )) {
                        log::error!("Failed to start capture action: {error:?}");
                        set_capture_toolbar_status(&toolbar, format!("Error: {error:?}"));
                    }
                }
            });
            return;
        }

        if w < 5.0 || h < 5.0 {
            let selection_weak = selection.as_weak();
            let hotkey_manager = hotkey_manager_area.clone();
            let state = state_for_selection.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(selection) = selection_weak.upgrade() {
                    let _ = selection.hide();
                }
                if let Some(ref manager) = hotkey_manager {
                    let _ = manager.unregister(esc_hotkey_area);
                }
                if let Ok(mut state) = state.lock() {
                    state.pending_selection = None;
                }
            });
            return;
        }

        // The continuous OCR path consumes the pending selection here, before the asynchronous
        // capture/translation work starts. This prevents a later close or click from being
        // mistaken for the old selection request.
        if let Ok(mut state) = state_for_selection.lock() {
            state.pending_selection = None;
        }

        // Do not mutate the native selection window from its pointer-up callback. Deferring the
        // hide avoids a re-entrant Winit window change and lets the OCR flow continue with the
        // selector already out of the way.
        let selection_weak_for_hide = selection.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            if let Some(selection) = selection_weak_for_hide.upgrade() {
                let _ = selection.hide();
            }
        });
        let main_weak_for_sync = main_weak_for_selection.clone();
        let state_for_selection = state_for_selection.clone();
        let overlay_weak = overlay_weak.clone();

        let hotkey_manager_async = hotkey_manager_area.clone();
        let esc_hotkey_async = esc_hotkey_area.clone();
        let textbox_weak_async = textbox_weak.clone();
        let selection_weak_async = selection.as_weak();
        let spawn_result = slint::spawn_local(async move {
            let textbox_weak = textbox_weak_async;
            let Some(selection) = selection_weak_async.upgrade() else {
                if let Some(ref mgr) = hotkey_manager_async {
                    let _ = mgr.unregister(esc_hotkey_async);
                }
                return;
            };
            let Some(main) = main_weak_for_sync.upgrade() else {
                let _ = selection.hide();
                if let Some(ref mgr) = hotkey_manager_async {
                    let _ = mgr.unregister(esc_hotkey_async);
                }
                return;
            };

            // Sync with LM Studio if applicable
            // (Removed automatic sync on area selected to prevent model reverting bug)
            if main.get_api_type().as_str() == PROVIDER_GEMINI {
                persist_google_api_key(&main.get_api_key().to_string());
            } else if main.get_api_type().as_str() == PROVIDER_CEREBRAS {
                persist_cerebras_api_key(&main.get_api_key().to_string());
            } else if main.get_api_type().as_str() == PROVIDER_OLLAMA_CLOUD {
                persist_ollama_cloud_api_key(&main.get_api_key().to_string());
            } else if main.get_api_type().as_str() == PROVIDER_UNSLOTH {
                persist_unsloth_api_key(&main.get_api_key().to_string());
            }

            let mut s = state_for_selection.lock().unwrap();
            // Convert logical to physical coordinates using scale factor
            let sf = selection.window().scale_factor();
            s.selection_scale = sf;
            s.capture_rect = Some(physical_selection_rect(&s, x, y, w, h));

            // Auto-start
            s.is_running = true;
            s.api_endpoint = main.get_api_endpoint().to_string();
            s.api_key = main.get_api_key().to_string();
            s.model_name = main.get_model_name().to_string();
            s.interval_sec = main.get_interval();
            s.system_prompt = main.get_system_prompt().to_string();
            s.temperature = main.get_temperature();
            s.thinking_level = main.get_thinking_level().to_string();
            s.provider = main.get_api_type().to_string();
            s.base_font_size = main.get_base_font_size();
            s.overlay_bg_color = main.get_overlay_bg_color();
            s.overlay_text_color = main.get_overlay_text_color();
            s.overlay_bg_opacity = main.get_overlay_bg_opacity();
            main.set_is_running(true);

            if let Some(overlay) = overlay_weak.upgrade() {
                // Set properties
                overlay.set_window_w(w);
                overlay.set_window_h(h);
                overlay.set_window_x(0.0); // Internal offset should be 0 since window itself is moved
                overlay.set_window_y(0.0);

                overlay.set_bg_color(s.overlay_bg_color.clone());
                overlay.set_text_color(s.overlay_text_color.clone());
                overlay.set_bg_opacity(if main.get_use_textbox() {
                    0.1
                } else {
                    s.overlay_bg_opacity
                });
                overlay.set_hide_text(main.get_use_textbox());
                overlay.set_is_textbox_mode(main.get_use_textbox());

                // Move and resize native window
                let window = overlay.window();
                window.set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
                    x, y,
                )));
                window.set_size(slint::LogicalSize::new(w, h));

                overlay.set_translated_text("Searching...".into());
                overlay.set_is_searching(true);
                overlay.set_font_size(calculate_font_size(
                    "Searching...",
                    w,
                    h,
                    main.get_base_font_size(),
                ));
                main.set_overlay_visible(true);

                if main.get_use_textbox() {
                    if let Some(tw) = textbox_weak.upgrade() {
                        tw.set_text("Searching...".into());
                        let _ = tw.show();
                    }
                }

                overlay.set_show_text(true);
                if let Err(error) = overlay.show() {
                    log::warn!("Failed to show OCR overlay: {error:?}");
                }

                // Set overlay to click-through and hide from taskbar
                #[cfg(target_os = "windows")]
                {
                    let owner = main_hwnd;
                    overlay.window().with_winit_window(move |winit_window| {
                        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                        if let Ok(handle) = winit_window.window_handle() {
                            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                                let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                                win_utils::set_layered(hwnd);
                                win_utils::set_tool_window(hwnd, false);
                                win_utils::set_exclude_from_capture(hwnd);
                                win_utils::disable_window_transitions(hwnd);
                                win_utils::set_click_through(hwnd, main.get_use_textbox());
                                if let Some(owner) = owner {
                                    win_utils::set_window_owner(hwnd, owner);
                                }
                            }
                        }
                    });
                }
            }

            if let Some(ref mgr) = hotkey_manager_async {
                let _ = mgr.unregister(esc_hotkey_async);
            }
        });
        if let Err(error) = spawn_result {
            log::error!("Failed to start OCR selection action: {error:?}");
            let selection_weak = selection.as_weak();
            let hotkey_manager = hotkey_manager_area.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(selection) = selection_weak.upgrade() {
                    let _ = selection.hide();
                }
                if let Some(ref manager) = hotkey_manager {
                    let _ = manager.unregister(esc_hotkey_area);
                }
            });
        }
    });

    let selection_weak_window_hover = selection_window.as_weak();
    let state_window_hover = state.clone();
    selection_window.on_window_hovered(move |x, y| {
        let Some(selection) = selection_weak_window_hover.upgrade() else {
            return;
        };
        let (origin_x, origin_y, scale) = {
            let state = state_window_hover.lock().unwrap();
            (
                state.selection_origin_x,
                state.selection_origin_y,
                state.selection_scale.max(1.0),
            )
        };
        let screen_x = origin_x + (x * scale).round() as i32;
        let screen_y = origin_y + (y * scale).round() as i32;
        if let Some(target) = capture::window_target_at_point(screen_x, screen_y) {
            selection.set_hover_x(((target.bounds.x - origin_x) as f32 / scale).max(0.0));
            selection.set_hover_y(((target.bounds.y - origin_y) as f32 / scale).max(0.0));
            selection.set_hover_w((target.bounds.width as f32 / scale).max(0.0));
            selection.set_hover_h((target.bounds.height as f32 / scale).max(0.0));
        } else {
            selection.set_hover_x(0.0);
            selection.set_hover_y(0.0);
            selection.set_hover_w(0.0);
            selection.set_hover_h(0.0);
        }
    });

    let selection_weak_color_hover = selection_window.as_weak();
    let toolbar_weak_color_hover = capture_toolbar.as_weak();
    let state_color_hover = state.clone();
    selection_window.on_color_hovered(move |x, y| {
        let color = {
            let state = state_color_hover.lock().unwrap();
            selection_pixel_at(&state, x, y)
        };
        let Some(color) = color else {
            return;
        };

        if let Some(selection) = selection_weak_color_hover.upgrade() {
            let (hex, decimal) = format_color_values(&color);
            selection.set_color_preview(color_preview(&color));
            selection.set_color_hex(hex.into());
            selection.set_color_decimal(decimal.into());
        }
        if let Some(toolbar) = toolbar_weak_color_hover.upgrade() {
            toolbar.set_color_picker_tooltip(color_toolbar_tooltip(&color).into());
        }
    });

    let selection_weak_window_selected = selection_window.as_weak();
    let state_window_selected = state.clone();
    let hotkey_manager_window_selected = hotkey_manager.clone();
    let esc_hotkey_window_selected = esc_hotkey.clone();
    let main_weak_window_selected = main_window.as_weak();
    let toolbar_weak_window_selected = capture_toolbar.as_weak();
    let recorder_slot_window_selected = recorder_slot.clone();
    let http_window_selected = http_client.clone();
    let recording_border_window_selected = recording_border_window.as_weak();
    selection_window.on_window_selected(move |click_x, click_y| {
        let Some(selection) = selection_weak_window_selected.upgrade() else {
            return;
        };
        let (purpose, origin_x, origin_y, scale) = {
            let mut state = state_window_selected.lock().unwrap();
            let Some(purpose) = state.pending_selection.take() else {
                return;
            };
            (
                purpose,
                state.selection_origin_x,
                state.selection_origin_y,
                state.selection_scale.max(1.0),
            )
        };

        let screen_x = origin_x + (click_x * scale).round() as i32;
        let screen_y = origin_y + (click_y * scale).round() as i32;
        let target = capture::window_target_at_point(screen_x, screen_y);

        if purpose == SelectionPurpose::ColorPicker || target.is_none() {
            let selection_weak = selection.as_weak();
            let toolbar_weak = toolbar_weak_window_selected.clone();
            let hotkey_manager = hotkey_manager_window_selected.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(toolbar) = toolbar_weak.upgrade() {
                    set_capture_toolbar_status(&toolbar, "No window was selected".to_string());
                    let _ = toolbar.show();
                }
                if let Some(selection) = selection_weak.upgrade() {
                    let _ = selection.hide();
                }
                if let Some(ref manager) = hotkey_manager {
                    let _ = manager.unregister(esc_hotkey_window_selected);
                }
            });
            return;
        }

        let target = target.expect("window target checked above");
        let Some(main) = main_weak_window_selected.upgrade() else {
            return;
        };
        let Some(toolbar) = toolbar_weak_window_selected.upgrade() else {
            return;
        };
        let selection_weak = selection.as_weak();
        let hotkey_manager = hotkey_manager_window_selected.clone();
        let main_weak = main.as_weak();
        let toolbar_weak = toolbar.as_weak();
        let state = state_window_selected.clone();
        let recorder_slot = recorder_slot_window_selected.clone();
        let http = http_window_selected.clone();
        let recording_border = recording_border_window_selected.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            // Keep a visible window while closing the selector; otherwise capture mode's
            // selection window would be the last registered window and stop the event loop.
            let toolbar = toolbar_weak.upgrade();
            if let Some(toolbar) = &toolbar {
                let _ = toolbar.show();
            }
            if let Some(selection) = selection_weak.upgrade() {
                let _ = selection.hide();
            }
            if let Some(ref manager) = hotkey_manager {
                let _ = manager.unregister(esc_hotkey_window_selected);
            }

            if let (Some(main), Some(toolbar)) = (main_weak.upgrade(), toolbar) {
                if let Err(error) = slint::spawn_local(run_toolbar_action(
                    purpose,
                    target.bounds,
                    Some(target),
                    None,
                    purpose == SelectionPurpose::Record,
                    recording_border,
                    main.as_weak(),
                    toolbar.as_weak(),
                    state,
                    recorder_slot,
                    http,
                )) {
                    log::error!("Failed to start window capture action: {error:?}");
                    set_capture_toolbar_status(&toolbar, format!("Error: {error:?}"));
                }
            }
        });
    });

    let selection_weak_color_picked = selection_window.as_weak();
    let state_color_picked = state.clone();
    let hotkey_manager_color_picked = hotkey_manager.clone();
    let esc_hotkey_color_picked = esc_hotkey.clone();
    let toolbar_weak_color_picked = capture_toolbar.as_weak();
    selection_window.on_color_picked(move |x, y| {
        let Some(selection) = selection_weak_color_picked.upgrade() else {
            return;
        };
        let (purpose, origin_x, origin_y, scale) = {
            let mut state = state_color_picked.lock().unwrap();
            let Some(purpose) = state.pending_selection.take() else {
                return;
            };
            (
                purpose,
                state.selection_origin_x,
                state.selection_origin_y,
                state.selection_scale.max(1.0),
            )
        };
        let screen_x = origin_x + (x * scale).round() as i32;
        let screen_y = origin_y + (y * scale).round() as i32;
        let selection_weak = selection.as_weak();
        let toolbar_weak = toolbar_weak_color_picked.clone();
        let hotkey_manager = hotkey_manager_color_picked.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            // Show the toolbar before hiding the selector so the capture-mode event loop keeps
            // running while the selected pixel is sampled.
            let toolbar = toolbar_weak.upgrade();
            if let Some(toolbar) = &toolbar {
                let _ = toolbar.show();
            }
            if let Some(selection) = selection_weak.upgrade() {
                let _ = selection.hide();
            }
            if let Some(ref manager) = hotkey_manager {
                let _ = manager.unregister(esc_hotkey_color_picked);
            }
            if purpose != SelectionPurpose::ColorPicker {
                return;
            }
            let Some(toolbar) = toolbar else {
                return;
            };
            let _ = slint::spawn_local(async move {
                let result = tokio::task::spawn_blocking(move || {
                    let color = capture::sample_pixel_at_point(screen_x, screen_y)?;
                    let tooltip = color_toolbar_tooltip(&color);
                    let clipboard_text = color_selection_tooltip(&color);
                    capture::copy_text_to_clipboard(&clipboard_text)?;
                    Ok::<String, anyhow::Error>(tooltip)
                })
                .await
                .context("Color picker worker stopped")
                .and_then(|result| result);
                match result {
                    Ok(color_info) => {
                        toolbar.set_color_picker_tooltip(color_info.clone().into());
                        toolbar.set_active_tooltip(color_info.clone().into());
                        set_capture_toolbar_status(&toolbar, format!("Copied {color_info}"));
                    }
                    Err(error) => set_capture_toolbar_status(&toolbar, format!("Error: {error}")),
                }
            });
        });
    });
}
