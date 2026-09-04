use crate::state::AppState;
use crate::text_layout::{calculate_font_size, clean_text};
use crate::{api, capture, MainWindow, OverlayWindow, SelectionWindow, TextboxWindow};
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn start(
    main_window: &MainWindow,
    overlay_window: &OverlayWindow,
    selection_window: &SelectionWindow,
    textbox_window: &TextboxWindow,
    state: Arc<Mutex<AppState>>,
    http_client: reqwest::Client,
    hotkey_capture: HotKey,
    hotkey_start_stop: HotKey,
    esc_hotkey: HotKey,
) {
    let state_for_worker = state.clone();
    // Background Worker - Dedicated thread to handle non-Send Monitor objects and CPU-intensive capture
    // Uses slint::invoke_from_event_loop() instead of tokio channels to guarantee
    // the slint event loop wakes up for every UI update (tokio wakers don't reliably
    // wake the slint event loop, causing freezes after prolonged use).
    let http_worker = http_client.clone();
    let runtime_handle = tokio::runtime::Handle::current();
    let overlay_weak_worker = overlay_window.as_weak();
    let main_weak_worker = main_window.as_weak();
    let textbox_weak_worker = textbox_window.as_weak();
    std::thread::spawn(move || {
        let mut prev_img = None;
        let mut prev_rect = None;
        let mut cached_monitors = None;
        let mut was_running = false;
        let mut last_monitor_refresh = std::time::Instant::now();

        loop {
            let (is_running, rect, api_config, step_interval, _base_fs, _use_textbox) = {
                let s = state_for_worker.lock().unwrap();
                (
                    s.is_running,
                    s.capture_rect,
                    (
                        s.api_endpoint.clone(),
                        s.api_key.clone(),
                        s.model_name.clone(),
                        s.system_prompt.clone(),
                        s.temperature,
                        s.thinking_level.clone(),
                        s.provider.clone(),
                    ),
                    s.interval_sec,
                    s.base_font_size,
                    s.use_textbox,
                )
            };

            if is_running && !was_running {
                prev_img = None;
                prev_rect = None;
            }
            was_running = is_running;

            if is_running && rect.is_some() {
                let current_rect = rect.unwrap();
                if Some(current_rect) != prev_rect {
                    prev_img = None;
                    prev_rect = Some(current_rect);
                }

                // Refresh monitors every 60 seconds or if never fetched
                if cached_monitors.is_none()
                    || last_monitor_refresh.elapsed() > Duration::from_secs(60)
                {
                    if let Ok(m) = xcap::Monitor::all() {
                        cached_monitors = Some(m);
                        last_monitor_refresh = std::time::Instant::now();
                    }
                }

                if let Ok(curr_img) = capture::capture_area(&current_rect, &cached_monitors) {
                    if capture::is_changed(&prev_img, &curr_img, 0.05) {
                        prev_img = Some(curr_img.clone());

                        // Notify UI: "Searching..."
                        {
                            let ow = overlay_weak_worker.clone();
                            let mw = main_weak_worker.clone();
                            let tww = textbox_weak_worker.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(overlay) = ow.upgrade() {
                                    overlay.set_translated_text("Searching...".into());
                                    overlay.set_is_searching(true);
                                    let base_fs = mw
                                        .upgrade()
                                        .map(|m| m.get_base_font_size())
                                        .unwrap_or(16.0);
                                    let font_size = calculate_font_size(
                                        "Searching...",
                                        overlay.get_window_w(),
                                        overlay.get_window_h(),
                                        base_fs,
                                    );
                                    overlay.set_font_size(font_size);
                                    let is_visible = mw
                                        .upgrade()
                                        .map(|m| m.get_overlay_visible())
                                        .unwrap_or(true);
                                    let use_textbox =
                                        mw.upgrade().map(|m| m.get_use_textbox()).unwrap_or(false);

                                    if use_textbox {
                                        overlay.set_show_text(is_visible);
                                        overlay.set_hide_text(true);
                                        overlay.set_bg_opacity(0.1);
                                    } else {
                                        overlay.set_show_text(is_visible);
                                        overlay.set_hide_text(false);
                                        overlay.set_bg_opacity(
                                            mw.upgrade()
                                                .map(|m| m.get_overlay_bg_opacity())
                                                .unwrap_or(0.9),
                                        );
                                    }

                                    if let Some(main) = mw.upgrade() {
                                        main.set_last_translated_text("Searching...".into());
                                        if let Some(tw) = tww.upgrade() {
                                            tw.set_text("Searching...".into());
                                            tw.set_text_color(main.get_overlay_text_color());
                                            tw.set_font_size(main.get_base_font_size());
                                        }
                                    }
                                }
                            });
                        }

                        let client = api::ApiClient::new(
                            http_worker.clone(),
                            api_config.0,
                            api_config.1,
                            api_config.2,
                            api_config.3,
                            api_config.4,
                            api_config.5,
                            api_config.6,
                        );

                        // Use runtime handle to call async translation from sync thread
                        let api_result = runtime_handle
                            .block_on(async { client.translate_image(&curr_img).await });

                        match api_result {
                            Ok(text) => {
                                {
                                    let mut s = state_for_worker.lock().unwrap();
                                    if s.last_text != text {
                                        s.last_text = text.clone();
                                    }
                                }

                                let ow = overlay_weak_worker.clone();
                                let mw = main_weak_worker.clone();
                                let tww = textbox_weak_worker.clone();
                                let final_text = text;
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(overlay) = ow.upgrade() {
                                        let display_text = clean_text(&final_text);
                                        overlay.set_translated_text(display_text.clone().into());
                                        overlay.set_is_searching(false);

                                        let base_fs = mw
                                            .upgrade()
                                            .map(|m| m.get_base_font_size())
                                            .unwrap_or(16.0);
                                        let font_size = calculate_font_size(
                                            &display_text,
                                            overlay.get_window_w(),
                                            overlay.get_window_h(),
                                            base_fs,
                                        );
                                        overlay.set_font_size(font_size);

                                        // Sync colors/opacity
                                        if let Some(main) = mw.upgrade() {
                                            let use_textbox = main.get_use_textbox();
                                            let is_visible = main.get_overlay_visible();

                                            overlay.set_bg_color(main.get_overlay_bg_color());
                                            overlay.set_text_color(main.get_overlay_text_color());

                                            if use_textbox {
                                                overlay.set_bg_opacity(0.1);
                                                overlay.set_hide_text(true);
                                            } else {
                                                overlay
                                                    .set_bg_opacity(main.get_overlay_bg_opacity());
                                                overlay.set_hide_text(false);
                                            }

                                            overlay.set_show_text(is_visible);

                                            main.set_last_translated_text(
                                                display_text.clone().into(),
                                            );
                                            if let Some(tw) = tww.upgrade() {
                                                tw.set_text(display_text.clone().into());
                                                tw.set_text_color(main.get_overlay_text_color());
                                                tw.set_font_size(main.get_base_font_size());
                                            }
                                        }

                                        // Copy to clipboard ??create/drop immediately
                                        if let Ok(mut cb) = arboard::Clipboard::new() {
                                            let _ = cb.set_text(&final_text);
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                log::error!("API Error: {:?}", e);
                                let err_msg = format!("Error: {}", e);
                                let ow = overlay_weak_worker.clone();
                                let mw = main_weak_worker.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(overlay) = ow.upgrade() {
                                        overlay.set_translated_text(err_msg.clone().into());
                                        overlay.set_is_searching(false);
                                        let base_fs = mw
                                            .upgrade()
                                            .map(|m| m.get_base_font_size())
                                            .unwrap_or(16.0);
                                        let font_size = calculate_font_size(
                                            &err_msg,
                                            overlay.get_window_w(),
                                            overlay.get_window_h(),
                                            base_fs,
                                        );
                                        overlay.set_font_size(font_size);
                                    }
                                });
                            }
                        }
                    }
                }

                // Handle Interval 0 (One-shot)
                if step_interval <= 0.01 {
                    {
                        let mut s = state_for_worker.lock().unwrap();
                        s.is_running = false;
                    }
                    let mw = main_weak_worker.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(main) = mw.upgrade() {
                            main.set_is_running(false);
                        }
                    });
                }
            } else {
                prev_img = None;
                prev_rect = None;
            }
            let sleep_duration = if step_interval <= 0.01 {
                Duration::from_secs(1)
            } else {
                Duration::from_secs_f32(step_interval)
            };
            std::thread::sleep(sleep_duration);
        }
    });

    // Hotkey Event Loop - Dedicated Thread for Responsiveness
    let main_weak_hk = main_window.as_weak();
    let selection_weak_hk = selection_window.as_weak();
    let hk_id = hotkey_capture.id();
    let ss_id = hotkey_start_stop.id();
    let esc_id = esc_hotkey.id();
    std::thread::spawn(move || loop {
        if let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            if event.state == global_hotkey::HotKeyState::Pressed {
                if event.id == hk_id {
                    let main_weak = main_weak_hk.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(main) = main_weak.upgrade() {
                            main.invoke_select_area_clicked();
                        }
                    });
                } else if event.id == ss_id {
                    let main_weak = main_weak_hk.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(main) = main_weak.upgrade() {
                            main.invoke_start_stop_clicked();
                        }
                    });
                } else if event.id == esc_id {
                    let selection_weak = selection_weak_hk.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(selection) = selection_weak.upgrade() {
                            selection.invoke_closed();
                        }
                    });
                }
            }
        }
    });
}
