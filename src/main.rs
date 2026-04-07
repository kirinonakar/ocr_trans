#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
slint::include_modules!();

mod win_utils;
mod capture;
mod api;

use std::sync::{Arc, Mutex};
use std::time::Duration;
use slint::ComponentHandle;
use anyhow::Result;
use global_hotkey::{GlobalHotKeyManager, hotkey::{HotKey, Modifiers, Code}, GlobalHotKeyEvent};
use tokio::sync::mpsc;
use i_slint_backend_winit::WinitWindowAccessor; // To access HWND on Windows

fn get_gemini_key() -> Option<String> {
    // 1. Check current directory
    if let Ok(key) = std::fs::read_to_string("gemini.txt") {
        return Some(key.trim().to_string());
    }
    // 2. Check executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join("gemini.txt");
            if let Ok(key) = std::fs::read_to_string(path) {
                return Some(key.trim().to_string());
            }
        }
    }
    None
}

#[derive(Default)]
struct AppState {
    is_running: bool,
    capture_rect: Option<capture::CaptureRect>,
    api_endpoint: String,
    api_key: String,
    model_name: String,
    interval_sec: u64,
    last_text: String,
}

fn calculate_font_size(text: &str, width: f32, height: f32) -> f32 {
    let len = text.chars().count();
    if len == 0 { return 16.0; }
    
    // Further increased padding for HiDPI safety
    let padding = 48.0;
    let available_w = (width - padding).max(40.0);
    let available_h = (height - padding).max(30.0);
    let area = available_w * available_h;
    
    // Even more conservative heuristic for font size
    let char_area_unit = 1.6; 
    
    let mut size = (area / (len as f32 * char_area_unit)).sqrt();
    
    // Clamp between 8 and 20 (further reduced max font size)
    size = size.clamp(8.0, 20.0);
    
    // One more check: if width is very small, we might need even smaller font
    // but word-wrap will handle it by growing vertically.
    // If it grows beyond available_h, it will be cut.
    
    size
}

fn rgba_to_slint_image(rgba: image::RgbaImage) -> slint::Image {
    let (width, height) = rgba.dimensions();
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(), width, height
    );
    slint::Image::from_rgba8(buffer)
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let main_window = MainWindow::new()?;
    let overlay_window = OverlayWindow::new()?;
    let selection_window = SelectionWindow::new()?;

    // Setup initial window states
    main_window.set_api_endpoint("http://localhost:1234/v1".into());
    let lm_models: Vec<slint::SharedString> = vec!["gemma-4-e4b-it".into(), "gemma-4-31b-it".into(), "qwen3.5-4b".into(), "qwen/qwen3.5-9b".into()];
    main_window.set_model_options(slint::ModelRc::from(lm_models.as_slice()));
    main_window.set_model_name("gemma-4-e4b-it".into());
    main_window.set_api_key("lm-studio".into());
    main_window.set_interval(0.0);

    // Load API key from gemini.txt if exists
    if let Some(key) = get_gemini_key() {
        main_window.set_api_key(key.into());
    }

    let state = Arc::new(Mutex::new(AppState {
        api_endpoint: main_window.get_api_endpoint().to_string(),
        model_name: main_window.get_model_name().to_string(),
        interval_sec: 0,
        last_text: String::new(),
        ..Default::default()
    }));

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
                    hwnd_out = Some(hwnd);
                }
            }
        });
        hwnd_out
    };

    let main_weak = main_window.as_weak();
    let main_weak_api = main_window.as_weak();
    let overlay_weak = overlay_window.as_weak();
    let selection_weak = selection_window.as_weak();
    let state_clone = state.clone();

    // Initial Selection Window Styles Setup
    let selection_initialized = Arc::new(Mutex::new(false));

    // API Type Changed Callback
    main_window.on_api_type_changed(move |api_type| {
        let main = main_weak_api.unwrap();
        if api_type == "Google Gemini" {
            main.set_api_endpoint("https://generativelanguage.googleapis.com".into());
            let gemini_models: Vec<slint::SharedString> = vec!["gemini-3.1-flash-lite-preview".into()];
            main.set_model_options(slint::ModelRc::from(gemini_models.as_slice()));
            main.set_model_name("gemini-3.1-flash-lite-preview".into());
            main.set_api_key(get_gemini_key().unwrap_or_default().into());
        } else {
            main.set_api_endpoint("http://localhost:1234/v1".into());
            let lm_models: Vec<slint::SharedString> = vec!["gemma-4-e4b-it".into(), "gemma-4-31b-it".into(), "qwen3.5-4b".into(), "qwen/qwen3.5-9b".into()];
            main.set_model_options(slint::ModelRc::from(lm_models.as_slice()));
            main.set_model_name("gemma-4-e4b-it".into());
            main.set_api_key("lm-studio".into());
        }
    });

    // Overlay Toggle Callback
    let overlay_weak_toggle = overlay_window.as_weak();
    #[cfg(target_os = "windows")]
    let main_hwnd_overlay = main_hwnd;
    main_window.on_overlay_toggle_clicked(move |visible| {
        if let Some(overlay) = overlay_weak_toggle.upgrade() {
            overlay.set_show_text(visible && !overlay.get_translated_text().is_empty() && !overlay.get_translated_text().starts_with("COMMAND:"));
            
            #[cfg(target_os = "windows")]
            if visible {
                overlay.window().with_winit_window(|winit_window| {
                    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = winit_window.window_handle() {
                        if let RawWindowHandle::Win32(h) = handle.as_raw() {
                            let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                            win_utils::set_layered(hwnd);
                            win_utils::set_tool_window(hwnd);
                            if let Some(owner) = main_hwnd_overlay {
                                win_utils::set_window_owner(hwnd, owner);
                            }
                        }
                    }
                });
            }
        }
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
        overlay.hide().unwrap();
    });

    // Start/Stop Callback
    let overlay_weak_for_stop = overlay_window.as_weak();
    #[cfg(target_os = "windows")]
    let main_hwnd_stop = main_hwnd;
    main_window.on_start_stop_clicked(move || {
        let main = main_weak.unwrap();
        let mut s = state_clone.lock().unwrap();
        
        if !s.is_running {
            if s.capture_rect.is_none() {
                return;
            }
            s.is_running = true;
            s.api_endpoint = main.get_api_endpoint().to_string();
            s.api_key = main.get_api_key().to_string();
            s.model_name = main.get_model_name().to_string();
            s.interval_sec = main.get_interval() as u64;
            main.set_is_running(true);
            if let Some(overlay) = overlay_weak_for_stop.upgrade() {
                overlay.set_translated_text("Searching...".into());
                overlay.set_font_size(calculate_font_size("Searching...", overlay.get_window_w(), overlay.get_window_h()));
                overlay.set_show_text(main.get_overlay_visible());
                overlay.show().unwrap();

                #[cfg(target_os = "windows")]
                overlay.window().with_winit_window(|winit_window| {
                    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = winit_window.window_handle() {
                        if let RawWindowHandle::Win32(h) = handle.as_raw() {
                            let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                            win_utils::set_layered(hwnd);
                            win_utils::set_tool_window(hwnd);
                            if let Some(owner) = main_hwnd_stop {
                                win_utils::set_window_owner(hwnd, owner);
                            }
                        }
                    }
                });
            }
        } else {
            s.is_running = false;
            main.set_is_running(false);
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
    let main_hwnd_selection = main_hwnd;
    main_window.on_select_area_clicked(move || {
        // Stop active capture
        {
            let mut s = state_for_selection_trigger.lock().unwrap();
            s.is_running = false;
        }
        if let Some(main) = main_weak_for_selection_trigger.upgrade() {
            main.set_is_running(false);
        }

        let selection = s_weak.unwrap();
        selection.invoke_reset();
        
        // Hide existing overlay if any
        if let Some(overlay) = overlay_weak_for_select.upgrade() {
            let _ = overlay.hide();
            overlay.set_translated_text("".into());
            overlay.set_show_text(false);
        }
        // Capture screenshot for background
        if let Ok(img) = capture::capture_full_screen() {
            let (w, h) = img.dimensions();
            let slint_img = rgba_to_slint_image(img);
            selection.set_screenshot(slint_img);
            
            // Set window size to match physical screenshot dimensions
            let sf = selection.window().scale_factor();
            selection.window().set_size(slint::LogicalSize::new(w as f32 / sf, h as f32 / sf));
        }
        selection.window().set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(0.0, 0.0)));
        
        #[cfg(target_os = "windows")]
        {
            let mut init = selection_initialized_clone.lock().unwrap();
            if !*init {
                let main_hwnd_cap = main_hwnd_selection;
                selection.window().with_winit_window(move |winit_window| {
                    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = winit_window.window_handle() {
                        if let RawWindowHandle::Win32(h) = handle.as_raw() {
                            let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                            win_utils::set_layered(hwnd);
                            win_utils::set_tool_window(hwnd);
                            if let Some(owner) = main_hwnd_cap {
                                win_utils::set_window_owner(hwnd, owner);
                            }
                        }
                    }
                });
                *init = true;
            }
        }

        selection.show().unwrap();
    });

    // Close Requested - Hard Exit
    main_window.window().on_close_requested(move || {
        std::process::exit(0);
    });

    let main_weak_for_selection = main_window.as_weak();
    let selection_weak_for_close = selection_window.as_weak();
    selection_window.on_closed(move || {
        let selection = selection_weak_for_close.unwrap();
        let _ = selection.hide();
    });

    let state_for_selection = state.clone();
    selection_window.on_area_selected(move |x, y, w, h| {
        let selection = selection_weak.unwrap();
        if w < 5.0 || h < 5.0 {
            let _ = selection.hide();
            return;
        }
        let mut s = state_for_selection.lock().unwrap();
        let main = main_weak_for_selection.unwrap();
        
        // Convert logical to physical coordinates using scale factor
        let sf = selection.window().scale_factor();
        
        s.capture_rect = Some(capture::CaptureRect {
            x: (x * sf) as i32,
            y: (y * sf) as i32,
            width: (w * sf) as i32,
            height: (h * sf) as i32,
        });

        // Auto-start
        s.is_running = true;
        s.api_endpoint = main.get_api_endpoint().to_string();
        s.api_key = main.get_api_key().to_string();
        s.model_name = main.get_model_name().to_string();
        s.interval_sec = main.get_interval() as u64;
        main.set_is_running(true);
        
        if let Some(overlay) = overlay_weak.upgrade() {
            // Set properties
            overlay.set_window_w(w);
            overlay.set_window_h(h);
            overlay.set_window_x(0.0); // Internal offset should be 0 since window itself is moved
            overlay.set_window_y(0.0);
            
            // Move and resize native window
            let window = overlay.window();
            window.set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(x, y)));
            window.set_size(slint::LogicalSize::new(w, h));
            
            overlay.set_translated_text("Searching...".into());
            overlay.set_font_size(calculate_font_size("Searching...", w, h));
            main.set_overlay_visible(true);
            overlay.set_show_text(true);
            overlay.show().unwrap();
            
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
                            win_utils::set_tool_window(hwnd);
                            if let Some(owner) = owner {
                                win_utils::set_window_owner(hwnd, owner);
                            }
                        }
                    }
                });
            }
        }

        selection.hide().unwrap();
    });

    // Global Hotkey Setup
    let hotkey_manager = GlobalHotKeyManager::new().unwrap();
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyA);
    hotkey_manager.register(hotkey).unwrap();

    let (tx, mut rx) = mpsc::channel(10);
    let state_for_worker = state.clone();
    
    // Background Worker
    tokio::spawn(async move {
        let mut prev_img = None;
        let mut prev_rect = None;
        
        loop {
            let (is_running, rect, api_config, step_interval) = {
                let s = state_for_worker.lock().unwrap();
                (s.is_running, s.capture_rect, (s.api_endpoint.clone(), s.api_key.clone(), s.model_name.clone()), s.interval_sec)
            };

            if is_running && rect.is_some() {
                let current_rect = rect.unwrap();
                if Some(current_rect) != prev_rect {
                    prev_img = None;
                    prev_rect = Some(current_rect);
                }
                
                if let Ok(curr_img) = capture::capture_area(&current_rect) {
                    if capture::is_changed(&prev_img, &curr_img, 0.05) { // The threshold in capture.rs is now 0.02, but we pass 0.05 here? 
                        // Actually, I'll update the capture.rs call to reflect the new logic.
                        // I'll keep the param but capture.rs now ignores it after my previous edit.
                        // I'll fix capture.rs to use the param properly later if needed, but for now 0.05 is fine.
                        
                        prev_img = Some(curr_img.clone());
                        let _ = tx.send("Searching...".into()).await;
                        
                        let client = api::ApiClient::new(api_config.0, api_config.1, api_config.2);
                        match client.translate_image(&curr_img).await {
                            Ok(text) => {
                                let is_new = {
                                    let mut s = state_for_worker.lock().unwrap();
                                    if s.last_text != text {
                                        s.last_text = text.clone();
                                        true
                                    } else {
                                        false
                                    }
                                };
                                
                                if is_new {
                                    let _ = tx.send(text).await;
                                } else if step_interval == 0 {
                                    // In Once mode, even if text is same, keep showing it
                                    let _ = tx.send(text).await;
                                } else {
                                    let _ = tx.send("".into()).await; // Clear "Searching..." if no change in text in loop mode
                                }
                            }
                            Err(e) => {
                                log::error!("API Error: {:?}", e);
                                let _ = tx.send(format!("Error: {}", e)).await;
                            }
                        }
                    }
                }
                
                // Handle Interval 0 (One-shot)
                if step_interval == 0 {
                    {
                        let mut s = state_for_worker.lock().unwrap();
                        s.is_running = false;
                    }
                    let _ = tx.send("COMMAND:STOPPED".into()).await;
                }
            } else {
                prev_img = None;
                prev_rect = None;
            }
            tokio::time::sleep(Duration::from_secs(step_interval.max(1))).await;
        }
    });

    // Listener for workers
    let overlay_weak_ui = overlay_window.as_weak();
    let main_weak_ui = main_window.as_weak();
    slint::spawn_local(async move {
        let mut clipboard = arboard::Clipboard::new().ok();
        while let Some(text) = rx.recv().await {
            if text == "COMMAND:STOPPED" {
                if let Some(main) = main_weak_ui.upgrade() {
                    main.set_is_running(false);
                }
                continue;
            }
            
            if let Some(overlay) = overlay_weak_ui.upgrade() {
                if text == "" {
                    overlay.set_show_text(false);
                    continue;
                }
                overlay.set_translated_text(text.clone().into());
                
                // Calculate and set font size
                let font_size = calculate_font_size(&text, overlay.get_window_w(), overlay.get_window_h());
                overlay.set_font_size(font_size);
                
                let is_overlay_visible = main_weak_ui.upgrade().map(|m| m.get_overlay_visible()).unwrap_or(true);
                overlay.set_show_text(is_overlay_visible);
                
                // Copy to clipboard
                if !text.starts_with("Searching...") && !text.starts_with("Error:") {
                    if let Some(ref mut cb) = clipboard {
                        let _ = cb.set_text(text);
                    }
                }
            }
        }
    }).unwrap();

    // Hotkey Event Loop - Dedicated Thread for Responsiveness
    let main_weak_hk = main_window.as_weak();
    std::thread::spawn(move || {
        loop {
            if let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
                if event.id == hotkey.id() && event.state == global_hotkey::HotKeyState::Pressed {
                    let main_weak = main_weak_hk.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(main) = main_weak.upgrade() {
                            main.invoke_select_area_clicked();
                        }
                    });
                }
            }
        }
    });

    main_window.run().unwrap();
    Ok(())
}
