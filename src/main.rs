#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
slint::include_modules!();

mod win_utils;
mod capture;
mod api;
mod credentials;

use std::sync::{Arc, Mutex};
use std::time::Duration;
use slint::ComponentHandle;
use anyhow::Result;
use global_hotkey::{GlobalHotKeyManager, hotkey::{HotKey, Modifiers, Code}, GlobalHotKeyEvent};

use i_slint_backend_winit::WinitWindowAccessor; // To access HWND on Windows

fn read_gemini_txt_key() -> Option<String> {
    // 1. Check current directory
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join("gemini.txt");
        if let Ok(key) = std::fs::read_to_string(path) {
            let key = key.trim().to_string();
            if !key.is_empty() {
                return Some(key);
            }
        }
    }
    // 2. Check executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join("gemini.txt");
            if let Ok(key) = std::fs::read_to_string(path) {
                let key = key.trim().to_string();
                if !key.is_empty() {
                    return Some(key);
                }
            }
        }
    }
    None
}

fn get_gemini_key() -> Option<String> {
    if let Some(key) = read_gemini_txt_key() {
        if let Err(err) = credentials::store_google_api_key(&key) {
            log::warn!("Failed to save gemini.txt key to Credential Manager: {err:?}");
        }
        return Some(key);
    }

    credentials::read_google_api_key()
}

fn persist_google_api_key(api_key: &str) {
    if let Err(err) = credentials::store_google_api_key(api_key) {
        log::warn!("Failed to update Google API key in Credential Manager: {err:?}");
    }
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

fn get_model_name() -> String {
    let default = "unsloth/gemma-4-26b-a4b-it";
    // 1. Check current directory
    if let Ok(model) = std::fs::read_to_string("model.txt") {
        return model.trim().to_string();
    }
    // 2. Check executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join("model.txt");
            if let Ok(model) = std::fs::read_to_string(path) {
                return model.trim().to_string();
            }
        }
    }
    default.to_string()
}

#[derive(Default)]
struct AppState {
    is_running: bool,
    capture_rect: Option<capture::CaptureRect>,
    api_endpoint: String,
    api_key: String,
    model_name: String,
    interval_sec: f32,
    system_prompt: String,
    temperature: f32,
    last_text: String,
    base_font_size: f32,
    overlay_bg_color: slint::Color,
    overlay_text_color: slint::Color,
    overlay_bg_opacity: f32,
    use_textbox: bool,
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
    
    // 1. Dynamic padding based on window size to maximize space in small overlays
    let padding_v = if height < 120.0 { (height * 0.2).max(20.0) } else { 48.0 };
    let padding_h = if width < 120.0 { (width * 0.1).max(12.0) } else { 32.0 };
    
    let available_w = (width - padding_h).max(20.0);
    let available_h = (height - padding_v).max(20.0);

    // Responsive font size for Searching...
    if text.starts_with("Searching...") {
        return (max_size * 1.1).min(available_h).max(10.0);
    }
    
    // Helper closure to check if text fits at a given font size
    let fits = |size: f32| -> bool {
        let line_height_est = size * 1.35; // Slightly tighter line height for better fitting
        let mut total_height = 0.0;
        
        for line in text.lines() {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() {
                total_height += line_height_est;
            } else {
                let mut line_width = 0.0;
                for c in line_trimmed.chars() {
                    // CJK characters are essentially square (1.0 ratio)
                    // Latin/Numbers are roughly 0.55-0.6 ratio
                    // Spaces are narrower (0.3 ratio)
                    let char_w = if (c >= '\u{3000}' && c <= '\u{9FFF}') || (c >= '\u{AC00}' && c <= '\u{D7AF}') {
                        size
                    } else if c.is_whitespace() {
                        size * 0.3
                    } else {
                        size * 0.58
                    };
                    line_width += char_w;
                }
                let num_wrapped_lines = (line_width / available_w).ceil().max(1.0);
                total_height += num_wrapped_lines * line_height_est;
            }
            if total_height > available_h { return false; }
        }
        total_height <= available_h
    };

    // 2. Binary search for the best font size (8.0 to max_size)
    // This provides much better precision and performance than linear search.
    let mut low = 8.0;
    let mut high = max_size;
    let mut best_size = low;

    // Fast-path: check if max_size already fits
    if fits(max_size) {
        return max_size;
    }

    // Binary search for precision (8 iterations = ~0.25px precision for range 8-72)
    for _ in 0..8 {
        let mid = (low + high) / 2.0;
        if fits(mid) {
            best_size = mid;
            low = mid;
        } else {
            high = mid;
        }
    }
    
    // Round to 0.5 for stability and clean appearance
    (best_size * 2.0).round() / 2.0
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
    let textbox_window = TextboxWindow::new()?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to build HTTP client");

    // Setup initial window states
    main_window.window().set_size(slint::LogicalSize::new(400.0, 780.0));
    main_window.set_api_endpoint("http://localhost:1234/v1".into());
    let default_model = get_model_name();
    let lm_models: Vec<slint::SharedString> = vec![
        default_model.clone().into(),
        "unsloth/gemma-4-26b-a4b-it".into(),
        "qwen/qwen3.5-9b".into(),
        "translate-gemma-12b-it".into(),
        "gemma-4-e4b-it".into(),
        "gemma-4-31b-it".into(),
        "qwen3.5-4b".into()
    ];
    main_window.set_model_options(slint::ModelRc::from(lm_models.as_slice()));
    main_window.set_model_name(default_model.into());
    main_window.set_model_index(0);
    main_window.set_api_key("lm-studio".into());
    main_window.set_system_prompt(get_system_prompt().into());
    main_window.set_interval(0.0);
    main_window.set_base_font_size(16.0);

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
                    let default_model_str = get_model_name();
                    
                    // Debug: print what models we got from LM Studio
                    println!("[Startup Sync] Models from API: {:?}", slint_models.iter().map(|m| m.as_str().to_string()).collect::<Vec<_>>());
                    println!("[Startup Sync] Looking for current: {:?}, default: {:?}", current_model_str, default_model_str);
                    
                    main.set_model_options(slint::ModelRc::from(slint_models.as_slice()));
                    
                    let mut found_index = None;
                    if let Some(idx) = slint_models.iter().position(|m| m.as_str() == current_model_str) {
                        found_index = Some(idx);
                    } else if let Some(idx) = slint_models.iter().position(|m| m.as_str() == default_model_str) {
                         found_index = Some(idx);
                    }
                    
                    let main_weak = main.as_weak();
                    slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                        if let Some(main) = main_weak.upgrade() {
                            if let Some(idx) = found_index {
                                main.set_model_name(slint_models[idx].clone());
                                main.set_model_index(idx as i32);
                            } else if let Some(first) = slint_models.first() {
                                main.set_model_name(first.clone());
                                main.set_model_index(0);
                            }
                        }
                    });
                }
            }
        }
    }).unwrap();

    let state = Arc::new(Mutex::new(AppState {
        api_endpoint: main_window.get_api_endpoint().to_string(),
        model_name: main_window.get_model_name().to_string(),
        interval_sec: 0.0,
        system_prompt: main_window.get_system_prompt().to_string(),
        last_text: String::new(),
        base_font_size: main_window.get_base_font_size(),
        overlay_bg_color: main_window.get_overlay_bg_color(),
        overlay_text_color: main_window.get_overlay_text_color(),
        overlay_bg_opacity: main_window.get_overlay_bg_opacity(),
        temperature: main_window.get_temperature(),
        use_textbox: main_window.get_use_textbox(),
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
                    hwnd_out = Some(hwnd);
                }
            }
        });
        hwnd_out
    };

    let main_weak = main_window.as_weak();
    let main_weak_api = main_window.as_weak();
    let main_weak_api_key = main_window.as_weak();
    let overlay_weak = overlay_window.as_weak();
    let selection_weak = selection_window.as_weak();
    let textbox_weak = textbox_window.as_weak();
    let state_clone = state.clone();

    // Initial Selection Window Styles Setup
    let selection_initialized = Arc::new(Mutex::new(false));

    // API Type Changed Callback
    main_window.on_api_type_changed(move |api_type| {
        let main = main_weak_api.unwrap();
        if api_type == "Google Gemini" {
            main.set_api_endpoint("https://generativelanguage.googleapis.com".into());
            let gemini_models: Vec<slint::SharedString> = vec![
                "gemini-3.1-flash-lite-preview".into(), 
                "gemini-3-flash-preview".into(), 
                "gemini-3.1-pro-preview".into(),
                "gemini-2.5-flash".into(),
                "gemini-2.5-flash-lite".into(),
                "gemma-4-26b-a4b-it".into(),
                "gemma-4-31b-it".into()
            ];
            main.set_model_options(slint::ModelRc::from(gemini_models.as_slice()));
            main.set_model_name("gemini-3.1-flash-lite-preview".into());
            main.set_model_index(0);
            main.set_api_key(get_gemini_key().unwrap_or_default().into());
            main.set_system_prompt(main.get_system_prompt()); // Preserve current prompt if user edited it, or we could reset to default

        } else {
            main.set_api_endpoint("http://localhost:1234/v1".into());
            main.set_api_key("lm-studio".into());
            // Model sync will be triggered separately by sync_lmstudio_models() from UI
        }
    });

    let state_api_key = state.clone();
    main_window.on_api_key_changed(move |api_key| {
        if let Some(main) = main_weak_api_key.upgrade() {
            let api_key = api_key.to_string();
            if main.get_api_type().as_str() == "Google Gemini" {
                persist_google_api_key(&api_key);
            }

            let mut s = state_api_key.lock().unwrap();
            s.api_key = api_key;
        }
    });
    
    // Sync LMStudio Models helper (shared logic)
    fn make_sync_lm_future(
        http: reqwest::Client,
        main: MainWindow,
    ) -> impl std::future::Future<Output = ()> {
        async move {
            let endpoint = main.get_api_endpoint().to_string();
            let api_key = main.get_api_key().to_string();
            let client = api::ApiClient::new(http, endpoint, api_key, String::new(), String::new(), 0.0);
            match client.get_models().await {
                Ok(models) => {
                    let slint_models: Vec<slint::SharedString> = models.into_iter().map(|s| s.into()).collect();
                    let current_model_str = main.get_model_name().as_str().to_string();
                    let default_model_str = get_model_name();

                    main.set_model_options(slint::ModelRc::from(slint_models.as_slice()));

                    let mut found_index = None;
                    if let Some(idx) = slint_models.iter().position(|m| m.as_str() == current_model_str) {
                        found_index = Some(idx);
                    } else if let Some(idx) = slint_models.iter().position(|m| m.as_str() == default_model_str) {
                        found_index = Some(idx);
                    }

                    let main_weak = main.as_weak();
                    slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                        if let Some(main) = main_weak.upgrade() {
                            if let Some(idx) = found_index {
                                main.set_model_name(slint_models[idx].clone());
                                main.set_model_index(idx as i32);
                            } else if let Some(first) = slint_models.first() {
                                main.set_model_name(first.clone());
                                main.set_model_index(0);
                            }
                        }
                    });
                }
                Err(e) => {
                    log::error!("Failed to fetch models: {:?}", e);
                }
            }
        }
    }

    // Sync LMStudio Models Callback (triggered on provider switch to LMStudio)
    let main_weak_sync = main_window.as_weak();
    let http_sync = http_client.clone();
    main_window.on_sync_lmstudio_models(move || {
        let main = main_weak_sync.unwrap();
        let http = http_sync.clone();
        slint::spawn_local(make_sync_lm_future(http, main)).unwrap();
    });

    // Refresh Models Callback
    let main_weak_refresh = main_window.as_weak();
    let http_refresh = http_client.clone();
    main_window.on_refresh_models_clicked(move || {
        let main = main_weak_refresh.unwrap();
        let api_key = main.get_api_key().to_string();
        if main.get_api_type().as_str() == "Google Gemini" {
            persist_google_api_key(&api_key);
        }
        let http = http_refresh.clone();
        slint::spawn_local(make_sync_lm_future(http, main)).unwrap();
    });

    // Overlay Toggle Callback
    let overlay_weak_toggle = overlay_window.as_weak();
    let state_for_toggle = state.clone();
    #[cfg(target_os = "windows")]
    let main_hwnd_overlay = main_hwnd;
    main_window.on_overlay_toggle_clicked(move |visible| {
        if let Some(overlay) = overlay_weak_toggle.upgrade() {
            overlay.set_show_text(visible && !overlay.get_translated_text().is_empty() && !overlay.get_translated_text().starts_with("COMMAND:"));
            
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

    // Style Panel Toggle Callback - force window resize
    let main_weak_panel = main_window.as_weak();
    main_window.on_style_panel_toggled(move |is_open| {
        if let Some(main) = main_weak_panel.upgrade() {
            let new_height = if is_open { 895.0_f32 } else { 780.0_f32 };
            main.window().set_size(slint::LogicalSize::new(400.0, new_height));
        }
    });

    // Style Changed Callback
    let main_weak_style = main_window.as_weak();
    let overlay_weak_style = overlay_window.as_weak();
    let textbox_weak_style = textbox_window.as_weak();
    let state_style = state.clone();
    main_window.on_style_changed(move || {
        let use_textbox = main_weak_style.upgrade().map(|m| m.get_use_textbox()).unwrap_or(false);
        {
            let mut s = state_style.lock().unwrap();
            s.use_textbox = use_textbox;
        }
        if let (Some(main), Some(overlay), Some(textbox)) = (main_weak_style.upgrade(), overlay_weak_style.upgrade(), textbox_weak_style.upgrade()) {
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
                let font_size = calculate_font_size(&last_text, overlay.get_window_w(), overlay.get_window_h(), base_fs);
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
        if let (Some(main), Some(overlay)) = (main_weak_tb_close.upgrade(), overlay_weak_tb_close.upgrade()) {
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
            if main.get_is_running() { return; } // Should not happen
            
            let main_weak_async = main_weak.clone();
            slint::spawn_local(async move {
                let main = main_weak_async.unwrap();
                
                // Sync with LM Studio if applicable
                // (Removed automatic sync on start to prevent model reverting bug)
                if main.get_api_type().as_str() == "Google Gemini" {
                    persist_google_api_key(&main.get_api_key().to_string());
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
                    overlay.set_font_size(calculate_font_size("Searching...", overlay.get_window_w(), overlay.get_window_h(), main.get_base_font_size()));
                    overlay.set_bg_color(s.overlay_bg_color.clone());
                    overlay.set_text_color(s.overlay_text_color.clone());
                    overlay.set_bg_opacity(if s.use_textbox { 0.1 } else { s.overlay_bg_opacity });
                    overlay.set_hide_text(s.use_textbox);
                    overlay.set_is_textbox_mode(s.use_textbox);
                    overlay.set_show_text(main.get_overlay_visible());
                    if s.use_textbox {
                        if let Some(textbox) = textbox_weak.upgrade() {
                            textbox.set_text("Searching...".into());
                            textbox.set_text_color(main.get_overlay_text_color());
                            textbox.set_font_size(main.get_base_font_size());
                            let _ = textbox.show();
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
            }).unwrap();
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
    let main_hwnd_selection = main_hwnd;
    let hotkey_manager_trigger = hotkey_manager.clone();
    let esc_hotkey_trigger = esc_hotkey.clone();
    main_window.on_select_area_clicked(move || {
        let selection = s_weak.unwrap();
        let _ = selection.hide(); // Hide if already showing to avoid double dimming

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
                            // Set exclude from capture to prevent double-dimming if hotkey is pressed again
                            win_utils::set_tool_window(hwnd, true);
                            win_utils::set_exclude_from_capture(hwnd);
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
        if let Some(ref mgr) = hotkey_manager_trigger {
            let _ = mgr.register(esc_hotkey_trigger);
        }
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
        if let Some(ref mgr) = hotkey_manager_for_close {
            let _ = mgr.unregister(esc_hotkey_for_close);
        }
    });

    let state_for_selection = state.clone();
    let hotkey_manager_area = hotkey_manager.clone();
    let esc_hotkey_area = esc_hotkey.clone();
    let textbox_weak_for_area = textbox_weak.clone();
    selection_window.on_area_selected(move |x, y, w, h| {
        let textbox_weak = textbox_weak_for_area.clone();
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
        let textbox_weak_async = textbox_weak.clone();
        slint::spawn_local(async move {
            let textbox_weak = textbox_weak_async;
            let main = main_weak_for_sync.unwrap();
            
            // Sync with LM Studio if applicable
            // (Removed automatic sync on area selected to prevent model reverting bug)
            if main.get_api_type().as_str() == "Google Gemini" {
                persist_google_api_key(&main.get_api_key().to_string());
            }

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
            s.interval_sec = main.get_interval();
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
                overlay.set_bg_opacity(if main.get_use_textbox() { 0.1 } else { s.overlay_bg_opacity });
                overlay.set_hide_text(main.get_use_textbox());
                overlay.set_is_textbox_mode(main.get_use_textbox());
                
                // Move and resize native window
                let window = overlay.window();
                window.set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(x, y)));
                window.set_size(slint::LogicalSize::new(w, h));
                
                overlay.set_translated_text("Searching...".into());
                overlay.set_is_searching(true);
                overlay.set_font_size(calculate_font_size("Searching...", w, h, main.get_base_font_size()));
                main.set_overlay_visible(true);

                if main.get_use_textbox() {
                    if let Some(tw) = textbox_weak.upgrade() {
                        tw.set_text("Searching...".into());
                        let _ = tw.show();
                    }
                }

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
                                win_utils::set_click_through(hwnd, main.get_use_textbox());
                                if let Some(owner) = owner {
                                    win_utils::set_window_owner(hwnd, owner);
                                }
                            }
                        }
                    });
                }
            }

            selection.hide().unwrap();
            if let Some(ref mgr) = hotkey_manager_async {
                let _ = mgr.unregister(esc_hotkey_async);
            }
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
                (s.is_running, s.capture_rect, (s.api_endpoint.clone(), s.api_key.clone(), s.model_name.clone(), s.system_prompt.clone(), s.temperature), s.interval_sec, s.base_font_size, s.use_textbox)
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
                            let tww = textbox_weak_worker.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(overlay) = ow.upgrade() {
                                    overlay.set_translated_text("Searching...".into());
                                    overlay.set_is_searching(true);
                                    let base_fs = mw.upgrade().map(|m| m.get_base_font_size()).unwrap_or(16.0);
                                    let font_size = calculate_font_size("Searching...", overlay.get_window_w(), overlay.get_window_h(), base_fs);
                                    overlay.set_font_size(font_size);
                                    let is_visible = mw.upgrade().map(|m| m.get_overlay_visible()).unwrap_or(true);
                                    let use_textbox = mw.upgrade().map(|m| m.get_use_textbox()).unwrap_or(false);
                                    
                                    if use_textbox {
                                        overlay.set_show_text(is_visible);
                                        overlay.set_hide_text(true);
                                        overlay.set_bg_opacity(0.1);
                                    } else {
                                        overlay.set_show_text(is_visible);
                                        overlay.set_hide_text(false);
                                        overlay.set_bg_opacity(mw.upgrade().map(|m| m.get_overlay_bg_opacity()).unwrap_or(0.9));
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
                                let tww = textbox_weak_worker.clone();
                                let final_text = text;
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(overlay) = ow.upgrade() {
                                        let display_text = clean_text(&final_text);
                                        overlay.set_translated_text(display_text.clone().into());
                                        overlay.set_is_searching(false);
                                        
                                        let base_fs = mw.upgrade().map(|m| m.get_base_font_size()).unwrap_or(16.0);
                                        let font_size = calculate_font_size(&display_text, overlay.get_window_w(), overlay.get_window_h(), base_fs);
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
                                                overlay.set_bg_opacity(main.get_overlay_bg_opacity());
                                                overlay.set_hide_text(false);
                                            }
                                            
                                            overlay.set_show_text(is_visible);
                                            
                                            main.set_last_translated_text(display_text.clone().into());
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
                                        let base_fs = mw.upgrade().map(|m| m.get_base_font_size()).unwrap_or(16.0);
                                        let font_size = calculate_font_size(&err_msg, overlay.get_window_w(), overlay.get_window_h(), base_fs);
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
            let sleep_duration = if step_interval <= 0.01 { Duration::from_secs(1) } else { Duration::from_secs_f32(step_interval) };
            std::thread::sleep(sleep_duration);
        }
    });

    // Hotkey Event Loop - Dedicated Thread for Responsiveness
    let main_weak_hk = main_window.as_weak();
    let selection_weak_hk = selection_window.as_weak();
    let hk_id = hotkey_capture.id();
    let ss_id = hotkey_start_stop.id();
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
        }
    });

    main_window.run().unwrap();
    Ok(())
}
