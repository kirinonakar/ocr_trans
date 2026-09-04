use crate::capture_workflow::*;
use crate::settings::*;
use crate::state::{AppState, SelectionPurpose};
use crate::text_layout::calculate_font_size;
use crate::{win_utils, MainWindow, OverlayWindow, SelectionWindow, TextboxWindow};
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
    state: Arc<Mutex<AppState>>,
    hotkey_manager: Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
    selection_initialized: Arc<Mutex<bool>>,
    #[cfg(target_os = "windows")] main_hwnd: Option<windows::Win32::Foundation::HWND>,
    #[cfg(not(target_os = "windows"))] main_hwnd: Option<()>,
) {
    let main_weak = main_window.as_weak();
    let textbox_weak = textbox_window.as_weak();
    let state_clone = state.clone();
    // Overlay Toggle Callback
    let overlay_weak_toggle = overlay_window.as_weak();
    let state_for_toggle = state.clone();
    #[cfg(target_os = "windows")]
    let main_hwnd_overlay = main_hwnd;
    main_window.on_overlay_toggle_clicked(move |visible| {
        if let Some(overlay) = overlay_weak_toggle.upgrade() {
            overlay.set_show_text(
                visible
                    && !overlay.get_translated_text().is_empty()
                    && !overlay.get_translated_text().starts_with("COMMAND:"),
            );

            if visible {
                let has_rect = {
                    let s = state_for_toggle.lock().unwrap();
                    s.capture_rect.is_some()
                };

                if has_rect {
                    let _ = overlay.show();
                }

                let is_textbox = overlay.get_is_textbox_mode();
                #[cfg(target_os = "windows")]
                overlay.window().with_winit_window(move |winit_window| {
                    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = winit_window.window_handle() {
                        if let RawWindowHandle::Win32(h) = handle.as_raw() {
                            let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                            win_utils::set_layered(hwnd);
                            win_utils::set_tool_window(hwnd, false);
                            win_utils::set_exclude_from_capture(hwnd);
                            win_utils::disable_window_transitions(hwnd);
                            win_utils::set_click_through(hwnd, is_textbox);
                            if let Some(owner) = main_hwnd_overlay {
                                win_utils::set_window_owner(hwnd, owner);
                            }
                        }
                    }
                });
            } else {
                let _ = overlay.hide();
            }
        }
    });

    // Style Changed Callback
    let main_weak_style = main_window.as_weak();
    let overlay_weak_style = overlay_window.as_weak();
    let textbox_weak_style = textbox_window.as_weak();
    let state_style = state.clone();
    main_window.on_style_changed(move || {
        let use_textbox = main_weak_style
            .upgrade()
            .map(|m| m.get_use_textbox())
            .unwrap_or(false);
        {
            let mut s = state_style.lock().unwrap();
            s.use_textbox = use_textbox;
        }
        if let (Some(main), Some(overlay), Some(textbox)) = (
            main_weak_style.upgrade(),
            overlay_weak_style.upgrade(),
            textbox_weak_style.upgrade(),
        ) {
            overlay.set_bg_color(main.get_overlay_bg_color());
            overlay.set_text_color(main.get_overlay_text_color());

            // Handle Textbox mode toggle and opacity sync
            let use_textbox = main.get_use_textbox();
            let base_opacity = main.get_overlay_bg_opacity();

            if use_textbox {
                overlay.set_bg_opacity(0.1);
                overlay.set_hide_text(true);
                overlay.set_is_textbox_mode(true);
                let _ = textbox.show();
                #[cfg(target_os = "windows")]
                schedule_textbox_native_theme(textbox.as_weak(), 0);
                textbox.set_text_color(main.get_overlay_text_color());
                textbox.set_font_size(main.get_base_font_size());
            } else {
                overlay.set_bg_opacity(base_opacity);
                overlay.set_hide_text(false);
                overlay.set_is_textbox_mode(false);
                let _ = textbox.hide();
            }

            #[cfg(target_os = "windows")]
            overlay.window().with_winit_window(move |winit_window| {
                use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = winit_window.window_handle() {
                    if let RawWindowHandle::Win32(h) = handle.as_raw() {
                        let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                        win_utils::set_click_through(hwnd, use_textbox);
                    }
                }
            });

            overlay.set_show_text(main.get_overlay_visible());

            // Recalculate and sync font size immediately
            let last_text = {
                let s = state_style.lock().unwrap();
                s.last_text.clone()
            };

            let base_fs = main.get_base_font_size();
            if !last_text.is_empty() {
                let font_size = calculate_font_size(
                    &last_text,
                    overlay.get_window_w(),
                    overlay.get_window_h(),
                    base_fs,
                );
                // println!("Style change: text len={}, base_fs={}, calculated_fs={}", last_text.len(), base_fs, font_size);
                overlay.set_font_size(font_size);
            } else {
                // If text is empty (searching or startup), still update for "Searching..." or future text
                overlay.set_font_size(base_fs);
            }
        }
    });

    // Textbox Closed Callback (Switch back to overlay)
    let main_weak_tb_close = main_window.as_weak();
    let overlay_weak_tb_close = overlay_window.as_weak();
    let state_tb_close = state.clone();
    textbox_window.window().on_close_requested(move || {
        if let (Some(main), Some(overlay)) = (
            main_weak_tb_close.upgrade(),
            overlay_weak_tb_close.upgrade(),
        ) {
            let mut s = state_tb_close.lock().unwrap();
            s.use_textbox = false;

            main.set_use_textbox(false);
            overlay.set_is_textbox_mode(false);
            overlay.set_bg_opacity(main.get_overlay_bg_opacity());
            overlay.set_hide_text(false);
            overlay.set_show_text(main.get_overlay_visible());

            #[cfg(target_os = "windows")]
            overlay.window().with_winit_window(|winit_window| {
                use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = winit_window.window_handle() {
                    if let RawWindowHandle::Win32(h) = handle.as_raw() {
                        let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                        win_utils::set_click_through(hwnd, false);
                    }
                }
            });
        }
        slint::CloseRequestResponse::HideWindow
    });

    // Close Clicked Callback
    let main_weak_close = main_window.as_weak();
    let overlay_weak_close = overlay_window.as_weak();
    let state_close = state.clone();
    overlay_window.on_close_clicked(move || {
        let main = main_weak_close.unwrap();
        let overlay = overlay_weak_close.unwrap();
        let mut s = state_close.lock().unwrap();

        s.is_running = false;
        main.set_is_running(false);
        main.set_overlay_visible(false);
        overlay.hide().unwrap();
    });

    // Start/Stop Callback
    let overlay_weak_for_stop = overlay_window.as_weak();
    #[cfg(target_os = "windows")]
    let main_hwnd_stop = main_hwnd;
    let textbox_weak_for_stop = textbox_weak.clone();
    main_window.on_start_stop_clicked(move || {
        let main = main_weak.unwrap();
        let state_clone = state_clone.clone();
        let overlay_weak = overlay_weak_for_stop.clone();
        let textbox_weak = textbox_weak_for_stop.clone();

        if !main.get_is_running() {
            if main.get_is_running() {
                return;
            } // Should not happen

            let main_weak_async = main_weak.clone();
            slint::spawn_local(async move {
                let main = main_weak_async.unwrap();

                // Sync with LM Studio if applicable
                // (Removed automatic sync on start to prevent model reverting bug)
                if main.get_api_type().as_str() == PROVIDER_GEMINI {
                    persist_google_api_key(&main.get_api_key().to_string());
                } else if main.get_api_type().as_str() == PROVIDER_CEREBRAS {
                    persist_cerebras_api_key(&main.get_api_key().to_string());
                } else if main.get_api_type().as_str() == PROVIDER_OLLAMA_CLOUD {
                    persist_ollama_cloud_api_key(&main.get_api_key().to_string());
                } else if main.get_api_type().as_str() == PROVIDER_UNSLOTH {
                    persist_unsloth_api_key(&main.get_api_key().to_string());
                }

                let mut s = state_clone.lock().unwrap();
                if s.capture_rect.is_none() {
                    return;
                }

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
                s.use_textbox = main.get_use_textbox();
                main.set_is_running(true);
                main.set_overlay_visible(true);

                if let Some(overlay) = overlay_weak.upgrade() {
                    overlay.set_translated_text("Searching...".into());
                    overlay.set_is_searching(true);
                    overlay.set_font_size(calculate_font_size(
                        "Searching...",
                        overlay.get_window_w(),
                        overlay.get_window_h(),
                        main.get_base_font_size(),
                    ));
                    overlay.set_bg_color(s.overlay_bg_color.clone());
                    overlay.set_text_color(s.overlay_text_color.clone());
                    overlay.set_bg_opacity(if s.use_textbox {
                        0.1
                    } else {
                        s.overlay_bg_opacity
                    });
                    overlay.set_hide_text(s.use_textbox);
                    overlay.set_is_textbox_mode(s.use_textbox);
                    overlay.set_show_text(main.get_overlay_visible());
                    if s.use_textbox {
                        if let Some(textbox) = textbox_weak.upgrade() {
                            textbox.set_text("Searching...".into());
                            textbox.set_text_color(main.get_overlay_text_color());
                            textbox.set_font_size(main.get_base_font_size());
                            let _ = textbox.show();
                            #[cfg(target_os = "windows")]
                            schedule_textbox_native_theme(textbox.as_weak(), 0);
                        }
                    }

                    overlay.show().unwrap();

                    #[cfg(target_os = "windows")]
                    overlay.window().with_winit_window(|winit_window| {
                        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                        if let Ok(handle) = winit_window.window_handle() {
                            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                                let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                                win_utils::set_layered(hwnd);
                                win_utils::set_tool_window(hwnd, false);
                                win_utils::set_exclude_from_capture(hwnd);
                                win_utils::disable_window_transitions(hwnd);
                                win_utils::set_click_through(hwnd, s.use_textbox);
                                if let Some(owner) = main_hwnd_stop {
                                    win_utils::set_window_owner(hwnd, owner);
                                }
                            }
                        }
                    });
                }
            })
            .unwrap();
        } else {
            let mut s = state_clone.lock().unwrap();
            s.is_running = false;
            main.set_is_running(false);
            main.set_overlay_visible(false);
            if let Some(overlay) = overlay_weak_for_stop.upgrade() {
                overlay.hide().unwrap();
            }
        }
    });

    let s_weak = selection_window.as_weak();
    let state_for_selection_trigger = state.clone();
    let main_weak_for_selection_trigger = main_window.as_weak();
    let overlay_weak_for_select = overlay_window.as_weak();
    let selection_initialized_clone = selection_initialized.clone();
    #[cfg(target_os = "windows")]
    let selection_owner = main_hwnd.map(|hwnd| hwnd.0 as isize);
    #[cfg(not(target_os = "windows"))]
    let selection_owner: Option<isize> = None;
    let hotkey_manager_trigger = hotkey_manager.clone();
    let esc_hotkey_trigger = esc_hotkey.clone();
    main_window.on_select_area_clicked(move || {
        // Defer creation/showing of the secondary native window until the main button's pointer
        // event has returned. Winit can otherwise tear down the event loop during re-entrant
        // window changes, which looked like a crash when SELECT AREA was pressed.
        let selection_weak = s_weak.clone();
        let state = state_for_selection_trigger.clone();
        let main_weak = main_weak_for_selection_trigger.clone();
        let overlay_weak = overlay_weak_for_select.clone();
        let selection_initialized = selection_initialized_clone.clone();
        let hotkey_manager = hotkey_manager_trigger.clone();
        let owner = selection_owner;
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            let Some(selection) = selection_weak.upgrade() else {
                return;
            };

            // Stop active capture before opening a new selection surface.
            if let Ok(mut state) = state.lock() {
                state.is_running = false;
                state.pending_selection = None;
            }
            if let Some(main) = main_weak.upgrade() {
                main.set_is_running(false);
            }
            if let Some(overlay) = overlay_weak.upgrade() {
                let _ = overlay.hide();
                overlay.set_translated_text("".into());
                overlay.set_show_text(false);
            }

            if !prepare_selection_window(
                &selection,
                &state,
                SelectionPurpose::ContinuousOcr,
                false,
                false,
                &selection_initialized,
                &hotkey_manager,
                esc_hotkey_trigger,
                owner,
            ) {
                log::warn!("Unable to start area selection from the OCR window");
            }
        });
    });
}
