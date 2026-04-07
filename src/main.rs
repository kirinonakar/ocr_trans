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

#[derive(Default)]
struct AppState {
    is_running: bool,
    capture_rect: Option<capture::CaptureRect>,
    api_endpoint: String,
    api_key: String,
    model_name: String,
    interval_sec: u64,
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
    main_window.set_model_name("qwen3.5-4b".into());
    main_window.set_interval(3.0);

    let state = Arc::new(Mutex::new(AppState {
        api_endpoint: main_window.get_api_endpoint().to_string(),
        model_name: main_window.get_model_name().to_string(),
        interval_sec: 3,
        ..Default::default()
    }));

    // Setup Transparency and Windows Specifics
    #[cfg(target_os = "windows")]
    {
        let mut main_hwnd = None;
        // Main window Mica
        main_window.window().with_winit_window(|winit_window| {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = winit_window.window_handle() {
                if let RawWindowHandle::Win32(h) = handle.as_raw() {
                    let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                    win_utils::set_mica_backdrop(hwnd);
                    main_hwnd = Some(hwnd);
                }
            }
        });

        if let Some(owner) = main_hwnd {
            // Ensure SelectionWindow supports alpha transparency and hide from taskbar
            selection_window.window().with_winit_window(move |winit_window| {
                use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = winit_window.window_handle() {
                    if let RawWindowHandle::Win32(h) = handle.as_raw() {
                        let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                        win_utils::set_layered(hwnd);
                        win_utils::set_tool_window(hwnd);
                        win_utils::set_window_owner(hwnd, owner);
                    }
                }
            });

            // Ensure OverlayWindow supports alpha transparency and hide from taskbar
            overlay_window.window().with_winit_window(move |winit_window| {
                use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = winit_window.window_handle() {
                    if let RawWindowHandle::Win32(h) = handle.as_raw() {
                        let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                        win_utils::set_layered(hwnd);
                        win_utils::set_tool_window(hwnd);
                        win_utils::set_window_owner(hwnd, owner);
                    }
                }
            });
        }
    }

    let main_weak = main_window.as_weak();
    let main_weak_api = main_window.as_weak();
    let overlay_weak = overlay_window.as_weak();
    let selection_weak = selection_window.as_weak();
    let state_clone = state.clone();

    // API Type Changed Callback
    main_window.on_api_type_changed(move |api_type| {
        let main = main_weak_api.unwrap();
        if api_type == "Google Gemini" {
            main.set_api_endpoint("https://generativelanguage.googleapis.com".into());
            main.set_model_name("Gemini 3.1 Flash Lite Preview".into());
        } else {
            main.set_api_endpoint("http://localhost:1234/v1".into());
            main.set_model_name("qwen3.5-4b".into());
        }
    });

    // Start/Stop Callback
    let overlay_weak_for_stop = overlay_window.as_weak();
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
                overlay.show().unwrap();
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
    let state_for_selection = state.clone();
    let overlay_weak_for_select = overlay_window.as_weak();
    main_window.on_select_area_clicked(move || {
        let selection = s_weak.unwrap();
        // Hide existing overlay if any
        if let Some(overlay) = overlay_weak_for_select.upgrade() {
            let _ = overlay.hide();
            overlay.set_translated_text("".into());
        }
        // Capture screenshot for background
        if let Ok(img) = capture::capture_full_screen() {
            let slint_img = rgba_to_slint_image(img);
            selection.set_screenshot(slint_img);
        }
        selection.show().unwrap();
        selection.window().set_fullscreen(true);
    });

    // Close Requested - Hard Exit
    main_window.window().on_close_requested(move || {
        std::process::exit(0);
    });

    let main_weak_for_selection = main_window.as_weak();
    selection_window.on_area_selected(move |x, y, w, h| {
        let selection = selection_weak.unwrap();
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
        
        selection.hide().unwrap();
        
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
            overlay.set_show_text(true);
            overlay.show().unwrap();
            
            // Set overlay to click-through
            #[cfg(target_os = "windows")]
            {
                overlay.window().with_winit_window(|winit_window| {
                    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = winit_window.window_handle() {
                        if let RawWindowHandle::Win32(h) = handle.as_raw() {
                            let hwnd = windows::Win32::Foundation::HWND(h.hwnd.get() as _);
                            win_utils::set_click_through(hwnd, true);
                        }
                    }
                });
            }
        }
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
                    if capture::is_changed(&prev_img, &curr_img, 0.05) { // Increased to 5%
                        prev_img = Some(curr_img.clone());
                        let _ = tx.send("Searching...".into()).await;
                        
                        let client = api::ApiClient::new(api_config.0, api_config.1, api_config.2);
                        match client.translate_image(&curr_img).await {
                            Ok(text) => {
                                let _ = tx.send(text).await;
                            }
                            Err(e) => {
                                log::error!("API Error: {:?}", e);
                                let _ = tx.send(format!("Error: {}", e)).await;
                            }
                        }
                    }
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
    slint::spawn_local(async move {
        while let Some(text) = rx.recv().await {
            if let Some(overlay) = overlay_weak_ui.upgrade() {
                overlay.set_translated_text(text.into());
                overlay.set_show_text(true);
            }
        }
    }).unwrap();

    // Hotkey Event Loop
    let main_weak_hk = main_window.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == hotkey.id() && event.state == global_hotkey::HotKeyState::Pressed {
                if let Some(main) = main_weak_hk.upgrade() {
                    main.invoke_start_stop_clicked();
                }
            }
        }
    });

    main_window.run().unwrap();
    Ok(())
}
