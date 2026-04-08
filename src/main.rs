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

const DEFAULT_SYSTEM_PROMPT: &str = "naturally translate into korean. only show translated texts.";

fn get_system_prompt() -> String {
    // 1. Check current directory
    if let Ok(prompt) = std::fs::read_to_string("system_prompt.txt") {
        return prompt.trim().to_string();
    }
    // 2. Check executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join("system_prompt.txt");
            if let Ok(prompt) = std::fs::read_to_string(path) {
                return prompt.trim().to_string();
            }
        }
    }
    DEFAULT_SYSTEM_PROMPT.to_string()
}

#[derive(Default)]
struct AppState {
    is_running: bool,
    capture_rect: Option<capture::CaptureRect>,
    api_endpoint: String,
    api_key: String,
    model_name: String,
    interval_sec: u64,
    system_prompt: String,
    temperature: f32,
    last_text: String,
    base_font_size: f32,
    overlay_bg_color: slint::Color,
    overlay_text_color: slint::Color,
    overlay_bg_opacity: f32,
}


fn clean_text(text: &str) -> String {
    let mut cleaned = String::new();
    let mut prev_empty = false;
    
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            prev_empty = true;
        } else {
            if !cleaned.is_empty() {
                if prev_empty {
                    cleaned.push_str("\n\n");
                } else {
                    cleaned.push('\n');
                }
            }
            cleaned.push_str(trimmed);
            prev_empty = false;
        }
    }
    cleaned
}

fn calculate_font_size(text: &str, width: f32, height: f32, max_size: f32) -> f32 {
    if text.is_empty() { return max_size; }
    
    // Safety padding for UI elements like the close button
    // top (24) + bottom (16) + extra bottom buffer (8) = 48
    let padding_v = 48.0; 
    let padding_h = 32.0; // 16 + 16
    
    let available_w = (width - padding_h).max(40.0);
    let available_h = (height - padding_v).max(30.0);

    // Responsive font size for Searching...
    if text.starts_with("Searching...") {
        return (max_size * 1.2).min(available_h).max(10.0);
    }
    
    // Calculate how many CJK characters are in the text to adjust width heuristic
    let cjk_count = text.chars().filter(|&c| {
        // Simple CJK range check
        (c >= '\u{3000}' && c <= '\u{9FFF}') || (c >= '\u{AC00}' && c <= '\u{D7AF}')
    }).count();
    let total_chars = text.chars().count();
    let cjk_ratio = if total_chars > 0 { cjk_count as f32 / total_chars as f32 } else { 0.0 };
    
    // Iterative approach to find a fitting font size
    let mut best_size = 8.0;
    let start_size = (max_size as i32).max(8);
    
    for size in (8..=start_size).rev() {
        let f_size = size as f32;
        // CJK characters are roughly square (1.0 width ratio), 
        // while Latin/Numerical are roughly 0.5-0.6.
        let char_width_est = f_size * (0.55 + (0.45 * cjk_ratio)); 
        let line_height_est = f_size * 1.4; // Slightly more generous line height
        
        let mut total_height = 0.0;
        for line in text.lines() {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() {
                total_height += line_height_est;
            } else {
                let line_len = line_trimmed.chars().count() as f32;
                let num_wrapped_lines = (line_len * char_width_est / available_w).ceil().max(1.0);
                total_height += num_wrapped_lines * line_height_est;
            }
        }
        
        if total_height <= available_h {
            best_size = f_size;
            break;
        }
        best_size = f_size; 
    }
    
    best_size
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

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to build HTTP client");

    // Setup initial window states
    main_window.set_api_endpoint("http://localhost:1234/v1".into());
    let lm_models: Vec<slint::SharedString> = vec!["google/gemma-4-26b-a4b".into(), "qwen/qwen3.5-9b".into(), "translate-gemma-12b-it".into(), "gemma-4-e4b-it".into(), "gemma-4-31b-it".into(), "qwen3.5-4b".into()];
    main_window.set_model_options(slint::ModelRc::from(lm_models.as_slice()));
    main_window.set_model_name("google/gemma-4-26b-a4b".into());
    main_window.set_model_index(0);
    main_window.set_api_key("lm-studio".into());
    main_window.set_system_prompt(get_system_prompt().into());
    main_window.set_interval(0.0);
    main_window.set_base_font_size(18.0);


    // Load API key from gemini.txt if exists
    if let Some(key) = get_gemini_key() {
        main_window.set_api_key(key.into());
    }

    // Initial Model Sync (Localhost/LM Studio)
    let main_weak_startup = main_window.as_weak();
    let http_startup = http_client.clone();
    slint::spawn_local(async move {
        if let Some(main) = main_weak_startup.upgrade() {
            let endpoint = main.get_api_endpoint().to_string();
            let api_key = main.get_api_key().to_string();
            
            if endpoint.contains("localhost") || endpoint.contains("127.0.0.1") {
                let client = api::ApiClient::new(http_startup, endpoint, api_key, String::new(), String::new(), 0.0);
                if let Ok(models) = client.get_models().await {
                    let slint_models: Vec<slint::SharedString> = models.into_iter().map(|s| s.into()).collect();
                    let current_model_str = main.get_model_name().as_str().to_string();
                    let default_model_str = "google/gemma-4-26b-a4b";
                    
                    // Debug: print what models we got from LM Studio
                    println!("[Startup Sync] Models from API: {:?}", slint_models.iter().map(|m| m.as_str().to_string()).collect::<Vec<_>>());
                    println!("[Startup Sync] Looking for current: {:?}, default: {:?}", current_model_str, default_model_str);
                    
                    main.set_model_options(slint::ModelRc::from(slint_models.as_slice()));
                    
                    let mut found_index = None;
                    
                    // Priority 1: Current model (might be saved from previous session)
                    if let Some(idx) = slint_models.iter().position(|m| m.as_str() == current_model_str) {
                        found_index = Some(idx);
                        println!("[Startup Sync] Found current at index: {:?}", found_index);
                    } 
                    // Priority 2: Default model
                    else if let Some(idx) = slint_models.iter().position(|m| m.as_str() == default_model_str) {
                         found_index = Some(idx);
                         println!("[Startup Sync] Found default at index: {:?}", found_index);
                    }
                    
                    if let Some(idx) = found_index {
                        main.set_model_name(slint_models[idx].clone());
                        main.set_model_index(idx as i32);
                    } else if let Some(first) = slint_models.first() {
                        println!("[Startup Sync] Fallback to first model");
                        main.set_model_name(first.clone());
                        main.set_model_index(0);
                    }
                }
            }
        }
    }).unwrap();

    let state = Arc::new(Mutex::new(AppState {
        api_endpoint: main_window.get_api_endpoint().to_string(),
        model_name: main_window.get_model_name().to_string(),
        interval_sec: 0,
        system_prompt: main_window.get_system_prompt().to_string(),
        last_text: String::new(),
        base_font_size: main_window.get_base_font_size(),
        overlay_bg_color: main_window.get_overlay_bg_color(),
        overlay_text_color: main_window.get_overlay_text_color(),
        overlay_bg_opacity: main_window.get_overlay_bg_opacity(),
        temperature: main_window.get_temperature(),
        ..Default::default()
    }));

    // Global Hotkey Setup
    let hotkey_manager = Arc::new(GlobalHotKeyManager::new().unwrap());
    let hotkey_capture = HotKey::new(Some(Modifiers::META), Code::Backquote);
    hotkey_manager.register(hotkey_capture).unwrap();
    let esc_hotkey = HotKey::new(None, Code::Escape);


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
            main.set_model_index(0);
            main.set_api_key(get_gemini_key().unwrap_or_default().into());
            main.set_system_prompt(main.get_system_prompt()); // Preserve current prompt if user edited it, or we could reset to default

        } else {
            main.set_api_endpoint("http://localhost:1234/v1".into());
            let lm_models: Vec<slint::SharedString> = vec!["google/gemma-4-26b-a4b".into(), "qwen/qwen3.5-9b".into(), "translate-gemma-12b-it".into(), "gemma-4-e4b-it".into(), "gemma-4-31b-it".into(), "qwen3.5-4b".into()];
            main.set_model_options(slint::ModelRc::from(lm_models.as_slice()));
            main.set_model_name("google/gemma-4-26b-a4b".into());
            main.set_model_index(0);
            main.set_api_key("lm-studio".into());
            main.set_system_prompt(main.get_system_prompt()); // Preserve current prompt

        }
    });
    
    // Refresh Models Callback
    let main_weak_refresh = main_window.as_weak();
    let http_refresh = http_client.clone();
    main_window.on_refresh_models_clicked(move || {
        let main = main_weak_refresh.unwrap();
        let endpoint = main.get_api_endpoint().to_string();
        let api_key = main.get_api_key().to_string();
        let http = http_refresh.clone();
        
        slint::spawn_local(async move {
            let client = api::ApiClient::new(http, endpoint, api_key, String::new(), String::new(), 0.0);
            match client.get_models().await {
                Ok(models) => {
                    let slint_models: Vec<slint::SharedString> = models.into_iter().map(|s| s.into()).collect();
                    let current_model_str = main.get_model_name().as_str().to_string();
                    let default_model_str = "google/gemma-4-26b-a4b";

                    main.set_model_options(slint::ModelRc::from(slint_models.as_slice()));
                    
                    let mut found_index = None;
                    
                    // Priority 1: Current model
                    if let Some(idx) = slint_models.iter().position(|m| m.as_str() == current_model_str) {
                        found_index = Some(idx);
                    } 
                    // Priority 2: Default model
                    else if let Some(idx) = slint_models.iter().position(|m| m.as_str() == default_model_str) {
                        found_index = Some(idx);
                    }

                    if let Some(idx) = found_index {
                        main.set_model_name(slint_models[idx].clone());
                        main.set_model_index(idx as i32);
                    } else if let Some(first) = slint_models.first() {
                        main.set_model_name(first.clone());
                        main.set_model_index(0);
                    }
                }
                Err(e) => {
                    log::error!("Failed to fetch models: {:?}", e);
                }
            }
        }).unwrap();
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
                            win_utils::set_tool_window(hwnd, false);
                            win_utils::set_exclude_from_capture(hwnd);
                            win_utils::disable_window_transitions(hwnd);
                            if let Some(owner) = main_hwnd_overlay {
                                win_utils::set_window_owner(hwnd, owner);
                            }
                        }
                    }
                });
            }
        }
    });

    // Style Changed Callback
    let main_weak_style = main_window.as_weak();
    let overlay_weak_style = overlay_window.as_weak();
    let state_style = state.clone();
    main_window.on_style_changed(move || {
        if let (Some(main), Some(overlay)) = (main_weak_style.upgrade(), overlay_weak_style.upgrade()) {
            overlay.set_bg_color(main.get_overlay_bg_color());
            overlay.set_text_color(main.get_overlay_text_color());
            overlay.set_bg_opacity(main.get_overlay_bg_opacity());
            
            // Recalculate and sync font size immediately
            let last_text = {
                let s = state_style.lock().unwrap();
                s.last_text.clone()
            };
            
            let base_fs = main.get_base_font_size();
            if !last_text.is_empty() {
                let font_size = calculate_font_size(&last_text, overlay.get_window_w(), overlay.get_window_h(), base_fs);
                // println!("Style change: text len={}, base_fs={}, calculated_fs={}", last_text.len(), base_fs, font_size);
                overlay.set_font_size(font_size);
            } else {
                // If text is empty (searching or startup), still update for "Searching..." or future text
                overlay.set_font_size(base_fs);
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
        let state_clone = state_clone.clone();
        let overlay_weak = overlay_weak_for_stop.clone();
        
        if !main.get_is_running() {
            if main.get_is_running() { return; } // Should not happen
            
            let main_weak_async = main_weak.clone();
            slint::spawn_local(async move {
                let main = main_weak_async.unwrap();
                
                // Sync with LM Studio if applicable
                // (Removed automatic sync on start to prevent model reverting bug)
                
                let mut s = state_clone.lock().unwrap();
                if s.capture_rect.is_none() {
                    return;
                }
                
                s.is_running = true;
                s.api_endpoint = main.get_api_endpoint().to_string();
                s.api_key = main.get_api_key().to_string();
                s.model_name = main.get_model_name().to_string();
                s.interval_sec = main.get_interval() as u64;
                s.system_prompt = main.get_system_prompt().to_string();
                s.temperature = main.get_temperature();
                s.base_font_size = main.get_base_font_size();
                s.overlay_bg_color = main.get_overlay_bg_color();
                s.overlay_text_color = main.get_overlay_text_color();
                s.overlay_bg_opacity = main.get_overlay_bg_opacity();
                main.set_is_running(true);

                
                if let Some(overlay) = overlay_weak.upgrade() {
                    overlay.set_translated_text("Searching...".into());
                    overlay.set_is_searching(true);
                    overlay.set_font_size(calculate_font_size("Searching...", overlay.get_window_w(), overlay.get_window_h(), main.get_base_font_size()));
                    overlay.set_show_text(main.get_overlay_visible());
                    overlay.set_bg_color(s.overlay_bg_color.clone());
                    overlay.set_text_color(s.overlay_text_color.clone());
                    overlay.set_bg_opacity(s.overlay_bg_opacity);

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
                                if let Some(owner) = main_hwnd_stop {
                                    win_utils::set_window_owner(hwnd, owner);
                                }
                            }
                        }
                    });
                }
            }).unwrap();
        } else {
            let mut s = state_clone.lock().unwrap();
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
    let hotkey_manager_trigger = hotkey_manager.clone();
    let esc_hotkey_trigger = esc_hotkey.clone();
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
                            // Removed set_layered for SelectionWindow to avoid DWM flickering
                            win_utils::set_tool_window(hwnd, true);
                            // Removed set_exclude_from_capture for SelectionWindow as it's not needed after screenshot
                            win_utils::disable_window_transitions(hwnd);
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
        let _ = hotkey_manager_trigger.register(esc_hotkey_trigger);
    });

    // Close Requested - Hard Exit
    main_window.window().on_close_requested(move || {
        std::process::exit(0);
    });

    let main_weak_for_selection = main_window.as_weak();
    let selection_weak_for_close = selection_window.as_weak();
    let hotkey_manager_for_close = hotkey_manager.clone();
    let esc_hotkey_for_close = esc_hotkey.clone();
    selection_window.on_closed(move || {
        let selection = selection_weak_for_close.unwrap();
        let _ = selection.hide();
        let _ = hotkey_manager_for_close.unregister(esc_hotkey_for_close);
    });

    let state_for_selection = state.clone();
    let hotkey_manager_area = hotkey_manager.clone();
    let esc_hotkey_area = esc_hotkey.clone();
    selection_window.on_area_selected(move |x, y, w, h| {
        let selection = selection_weak.unwrap();
        if w < 5.0 || h < 5.0 {
            let _ = selection.hide();
            return;
        }
        let main_weak_for_sync = main_weak_for_selection.clone();
        let state_for_selection = state_for_selection.clone();
        let overlay_weak = overlay_weak.clone();

        let hotkey_manager_async = hotkey_manager_area.clone();
        let esc_hotkey_async = esc_hotkey_area.clone();
        slint::spawn_local(async move {
            let main = main_weak_for_sync.unwrap();
            
            // Sync with LM Studio if applicable
            // (Removed automatic sync on area selected to prevent model reverting bug)

            let mut s = state_for_selection.lock().unwrap();
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
            s.system_prompt = main.get_system_prompt().to_string();
            s.temperature = main.get_temperature();
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
                overlay.set_bg_opacity(s.overlay_bg_opacity);
                
                // Move and resize native window
                let window = overlay.window();
                window.set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(x, y)));
                window.set_size(slint::LogicalSize::new(w, h));
                
                overlay.set_translated_text("Searching...".into());
                overlay.set_is_searching(true);
                overlay.set_font_size(calculate_font_size("Searching...", w, h, main.get_base_font_size()));
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
                                win_utils::set_tool_window(hwnd, false);
                                win_utils::set_exclude_from_capture(hwnd);
                                win_utils::disable_window_transitions(hwnd);
                                if let Some(owner) = owner {
                                    win_utils::set_window_owner(hwnd, owner);
                                }
                            }
                        }
                    });
                }
            }

            selection.hide().unwrap();
            let _ = hotkey_manager_async.unregister(esc_hotkey_async);
        }).unwrap();
    });

    let state_for_worker = state.clone();

    // Background Worker - Dedicated thread to handle non-Send Monitor objects and CPU-intensive capture
    // Uses slint::invoke_from_event_loop() instead of tokio channels to guarantee
    // the slint event loop wakes up for every UI update (tokio wakers don't reliably
    // wake the slint event loop, causing freezes after prolonged use).
    let http_worker = http_client.clone();
    let runtime_handle = tokio::runtime::Handle::current();
    let overlay_weak_worker = overlay_window.as_weak();
    let main_weak_worker = main_window.as_weak();
    std::thread::spawn(move || {
        let mut prev_img = None;
        let mut prev_rect = None;
        let mut cached_monitors = None;
        let mut last_monitor_refresh = std::time::Instant::now();
        
        loop {
            let (is_running, rect, api_config, step_interval, _base_fs) = {
                let s = state_for_worker.lock().unwrap();
                (s.is_running, s.capture_rect, (s.api_endpoint.clone(), s.api_key.clone(), s.model_name.clone(), s.system_prompt.clone(), s.temperature), s.interval_sec, s.base_font_size)
            };

            if is_running && rect.is_some() {
                let current_rect = rect.unwrap();
                if Some(current_rect) != prev_rect {
                    prev_img = None;
                    prev_rect = Some(current_rect);
                }
                
                // Refresh monitors every 60 seconds or if never fetched
                if cached_monitors.is_none() || last_monitor_refresh.elapsed() > Duration::from_secs(60) {
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
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(overlay) = ow.upgrade() {
                                    overlay.set_translated_text("Searching...".into());
                                    overlay.set_is_searching(true);
                                    let base_fs = mw.upgrade().map(|m| m.get_base_font_size()).unwrap_or(18.0);
                                    let font_size = calculate_font_size("Searching...", overlay.get_window_w(), overlay.get_window_h(), base_fs);
                                    overlay.set_font_size(font_size);
                                    let is_visible = mw.upgrade().map(|m| m.get_overlay_visible()).unwrap_or(true);
                                    overlay.set_show_text(is_visible);
                                }
                            });
                        }
                        
                        let client = api::ApiClient::new(http_worker.clone(), api_config.0, api_config.1, api_config.2, api_config.3, api_config.4);
                        
                        // Use runtime handle to call async translation from sync thread
                        let api_result = runtime_handle.block_on(async {
                            client.translate_image(&curr_img).await
                        });

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
                                let final_text = text;
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(overlay) = ow.upgrade() {
                                        let display_text = clean_text(&final_text);
                                        overlay.set_translated_text(display_text.clone().into());
                                        overlay.set_is_searching(false);
                                        
                                        let base_fs = mw.upgrade().map(|m| m.get_base_font_size()).unwrap_or(18.0);
                                        let font_size = calculate_font_size(&display_text, overlay.get_window_w(), overlay.get_window_h(), base_fs);
                                        overlay.set_font_size(font_size);
                                        
                                        let is_visible = mw.upgrade().map(|m| m.get_overlay_visible()).unwrap_or(true);
                                        overlay.set_show_text(is_visible);
                                        
                                        // Sync colors/opacity
                                        if let Some(main) = mw.upgrade() {
                                            overlay.set_bg_color(main.get_overlay_bg_color());
                                            overlay.set_text_color(main.get_overlay_text_color());
                                            overlay.set_bg_opacity(main.get_overlay_bg_opacity());
                                        }
                                        
                                        // Copy to clipboard — create/drop immediately
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
                                        let base_fs = mw.upgrade().map(|m| m.get_base_font_size()).unwrap_or(18.0);
                                        let font_size = calculate_font_size(&err_msg, overlay.get_window_w(), overlay.get_window_h(), base_fs);
                                        overlay.set_font_size(font_size);
                                    }
                                });
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
            let sleep_secs = if step_interval == 0 { 1 } else { step_interval };
            std::thread::sleep(Duration::from_secs(sleep_secs));
        }
    });

    // Hotkey Event Loop - Dedicated Thread for Responsiveness
    let main_weak_hk = main_window.as_weak();
    let selection_weak_hk = selection_window.as_weak();
    let hk_id = hotkey_capture.id();
    let esc_id = esc_hotkey.id();
    std::thread::spawn(move || {
        loop {
            if let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
                if event.state == global_hotkey::HotKeyState::Pressed {
                    if event.id == hk_id {
                        let main_weak = main_weak_hk.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(main) = main_weak.upgrade() {
                                main.invoke_select_area_clicked();
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
        }
    });

    main_window.run().unwrap();
    Ok(())
}
