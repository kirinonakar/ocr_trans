#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
slint::include_modules!();

mod api;
mod capture;
mod credentials;
mod ocr;
mod win_utils;

use anyhow::{Context, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use slint::ComponentHandle;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use i_slint_backend_winit::WinitWindowAccessor;
use serde::{Deserialize, Serialize}; // To access HWND on Windows

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

fn read_cerebras_txt_key() -> Option<String> {
    // 1. Check current directory
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join("cerebras.txt");
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
            let path = exe_dir.join("cerebras.txt");
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

fn get_cerebras_key() -> Option<String> {
    if let Some(key) = read_cerebras_txt_key() {
        if let Err(err) = credentials::store_cerebras_api_key(&key) {
            log::warn!("Failed to save cerebras.txt key to Credential Manager: {err:?}");
        }
        return Some(key);
    }
    credentials::read_cerebras_api_key()
}

fn persist_cerebras_api_key(api_key: &str) {
    if let Err(err) = credentials::store_cerebras_api_key(api_key) {
        log::warn!("Failed to update Cerebras API key in Credential Manager: {err:?}");
    }
}

fn read_ollama_cloud_txt_key() -> Option<String> {
    // 1. Check current directory
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join("ollama_cloud.txt");
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
            let path = exe_dir.join("ollama_cloud.txt");
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

fn get_ollama_cloud_key() -> Option<String> {
    if let Some(key) = read_ollama_cloud_txt_key() {
        if let Err(err) = credentials::store_ollama_cloud_api_key(&key) {
            log::warn!("Failed to save ollama_cloud.txt key to Credential Manager: {err:?}");
        }
        return Some(key);
    }

    credentials::read_ollama_cloud_api_key().or_else(|| std::env::var("OLLAMA_API_KEY").ok())
}

fn persist_ollama_cloud_api_key(api_key: &str) {
    if let Err(err) = credentials::store_ollama_cloud_api_key(api_key) {
        log::warn!("Failed to update Ollama Cloud API key in Credential Manager: {err:?}");
    }
}

fn get_unsloth_key() -> Option<String> {
    credentials::read_unsloth_api_key().or_else(|| std::env::var("UNSLOTH_STUDIO_AUTH_TOKEN").ok())
}

fn persist_unsloth_api_key(api_key: &str) {
    if let Err(err) = credentials::store_unsloth_api_key(api_key) {
        log::warn!("Failed to update Unsloth Desktop API key in Credential Manager: {err:?}");
    }
}

fn get_opencode_go_key() -> Option<String> {
    credentials::read_opencode_go_api_key().or_else(|| std::env::var("OPENCODE_GO_API_KEY").ok())
}

fn persist_opencode_go_api_key(api_key: &str) {
    if let Err(err) = credentials::store_opencode_go_api_key(api_key) {
        log::warn!("Failed to update OpenCode Go API key in Credential Manager: {err:?}");
    }
}

fn get_opencode_zen_key() -> Option<String> {
    credentials::read_opencode_zen_api_key().or_else(|| std::env::var("OPENCODE_ZEN_API_KEY").ok())
}

fn persist_opencode_zen_api_key(api_key: &str) {
    if let Err(err) = credentials::store_opencode_zen_api_key(api_key) {
        log::warn!("Failed to update OpenCode Zen API key in Credential Manager: {err:?}");
    }
}

const DEFAULT_SYSTEM_PROMPT: &str = "naturally translate into korean. only show translated texts.";
const SETTINGS_FILE_NAME: &str = "ocr_trans.ini";

fn read_legacy_system_prompt() -> Option<String> {
    // 1. Check current directory
    if let Ok(prompt) = std::fs::read_to_string("system_prompt.txt") {
        let prompt = prompt.trim().to_string();
        if !prompt.is_empty() {
            return Some(prompt);
        }
    }
    // 2. Check executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join("system_prompt.txt");
            if let Ok(prompt) = std::fs::read_to_string(path) {
                let prompt = prompt.trim().to_string();
                if !prompt.is_empty() {
                    return Some(prompt);
                }
            }
        }
    }
    None
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

#[derive(Serialize, Deserialize, Default, Clone)]
struct ProviderConfig {
    provider: String,
    lm_model: String,
    gemini_model: String,
    #[serde(default)]
    cerebras_model: String,
    #[serde(default)]
    ollama_model: String,
    #[serde(default)]
    ollama_cloud_model: String,
    #[serde(default)]
    unsloth_model: String,
    #[serde(default)]
    thinking_level: String,
    #[serde(default)]
    opencode_go_model: String,
    #[serde(default)]
    opencode_zen_model: String,
}

const PROVIDER_LMSTUDIO: &str = "LMStudio";
const PROVIDER_GEMINI: &str = "Google Gemini";
const PROVIDER_CEREBRAS: &str = "Cerebras";
const PROVIDER_OLLAMA: &str = "Ollama";
const PROVIDER_OLLAMA_CLOUD: &str = "Ollama Cloud";
const PROVIDER_UNSLOTH: &str = "Unsloth Desktop";
const PROVIDER_OPENCODE_GO: &str = "OpenCode Go";
const PROVIDER_OPENCODE_ZEN: &str = "OpenCode Zen";

fn configured_thinking_level(config: &ProviderConfig) -> String {
    match config.thinking_level.trim().to_lowercase().as_str() {
        "disable" | "disabled" => "disable".to_string(),
        "low" | "medium" | "high" | "xhigh" | "max" => config.thinking_level.trim().to_lowercase(),
        _ => "default".to_string(),
    }
}

fn saved_model_for_provider(config: &ProviderConfig, provider: &str) -> String {
    match provider {
        PROVIDER_GEMINI => config.gemini_model.clone(),
        PROVIDER_CEREBRAS => config.cerebras_model.clone(),
        PROVIDER_OLLAMA => config.ollama_model.clone(),
        PROVIDER_OLLAMA_CLOUD => config.ollama_cloud_model.clone(),
        PROVIDER_UNSLOTH => config.unsloth_model.clone(),
        PROVIDER_OPENCODE_GO => config.opencode_go_model.clone(),
        PROVIDER_OPENCODE_ZEN => config.opencode_zen_model.clone(),
        _ => config.lm_model.clone(),
    }
}

fn set_saved_model_for_provider(config: &mut ProviderConfig, provider: &str, model: String) {
    match provider {
        PROVIDER_GEMINI => config.gemini_model = model,
        PROVIDER_CEREBRAS => config.cerebras_model = model,
        PROVIDER_OLLAMA => config.ollama_model = model,
        PROVIDER_OLLAMA_CLOUD => config.ollama_cloud_model = model,
        PROVIDER_UNSLOTH => config.unsloth_model = model,
        PROVIDER_OPENCODE_GO => config.opencode_go_model = model,
        PROVIDER_OPENCODE_ZEN => config.opencode_zen_model = model,
        _ => config.lm_model = model,
    }
}

#[derive(Default, Clone)]
struct AppSettings {
    provider: ProviderConfig,
    capture_folder: String,
    system_prompt: String,
    app_mode: String,
    dark_theme: bool,
}

fn settings_read_path() -> Option<PathBuf> {
    // Check current directory first
    if let Ok(dir) = std::env::current_dir() {
        let path = dir.join(SETTINGS_FILE_NAME);
        if path.exists() {
            return Some(path);
        }
    }
    // Then check executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            let path = dir.join(SETTINGS_FILE_NAME);
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn settings_write_path() -> Option<PathBuf> {
    if let Some(path) = settings_read_path() {
        return Some(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|path| path.join(SETTINGS_FILE_NAME)))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.join(SETTINGS_FILE_NAME))
        })
}

fn legacy_provider_config_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::current_dir() {
        let path = dir.join("provider_config.json");
        if path.exists() {
            return Some(path);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("provider_config.json")))
}

fn legacy_capture_folder() -> Option<String> {
    let paths = [
        std::env::current_dir()
            .ok()
            .map(|dir| dir.join("capture_folder.txt")),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.join("capture_folder.txt"))),
    ];
    for path in paths.into_iter().flatten() {
        if let Ok(folder) = std::fs::read_to_string(path) {
            let folder = folder.trim();
            if !folder.is_empty() {
                return Some(folder.to_string());
            }
        }
    }
    None
}

fn ini_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn ini_unescape(value: &str) -> String {
    let mut unescaped = String::with_capacity(value.len());
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            match ch {
                'n' => unescaped.push('\n'),
                'r' => unescaped.push('\r'),
                't' => unescaped.push('\t'),
                '\\' => unescaped.push('\\'),
                _ => {
                    unescaped.push('\\');
                    unescaped.push(ch);
                }
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            unescaped.push(ch);
        }
    }
    if escaped {
        unescaped.push('\\');
    }
    unescaped
}

fn parse_ini(contents: &str) -> HashMap<(String, String), String> {
    let mut values = HashMap::new();
    let mut section = String::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        values.insert(
            (section.clone(), key.trim().to_lowercase()),
            ini_unescape(value.trim()),
        );
    }
    values
}

fn ini_value(
    values: &HashMap<(String, String), String>,
    section: &str,
    key: &str,
    fallback: String,
) -> String {
    values
        .get(&(section.to_lowercase(), key.to_lowercase()))
        .cloned()
        .unwrap_or(fallback)
}

fn ini_bool(values: &HashMap<(String, String), String>, section: &str, key: &str, fallback: bool) -> bool {
    let fallback = if fallback { "true" } else { "false" };
    matches!(
        ini_value(values, section, key, fallback.to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn load_legacy_provider_config() -> ProviderConfig {
    if let Some(path) = legacy_provider_config_path() {
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(config) = serde_json::from_str::<ProviderConfig>(&json) {
                return config;
            }
        }
    }
    ProviderConfig::default()
}

fn load_app_settings() -> AppSettings {
    let legacy_provider = load_legacy_provider_config();
    let mut settings = AppSettings {
        provider: legacy_provider,
        capture_folder: legacy_capture_folder().unwrap_or_default(),
        system_prompt: read_legacy_system_prompt()
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
        app_mode: "ocr".to_string(),
        dark_theme: false,
    };

    if let Some(path) = settings_read_path() {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let values = parse_ini(&contents);
            settings.provider.provider =
                ini_value(&values, "provider", "provider", settings.provider.provider);
            settings.provider.lm_model =
                ini_value(&values, "provider", "lm_model", settings.provider.lm_model);
            settings.provider.gemini_model = ini_value(
                &values,
                "provider",
                "gemini_model",
                settings.provider.gemini_model,
            );
            settings.provider.cerebras_model = ini_value(
                &values,
                "provider",
                "cerebras_model",
                settings.provider.cerebras_model,
            );
            settings.provider.ollama_model = ini_value(
                &values,
                "provider",
                "ollama_model",
                settings.provider.ollama_model,
            );
            settings.provider.ollama_cloud_model = ini_value(
                &values,
                "provider",
                "ollama_cloud_model",
                settings.provider.ollama_cloud_model,
            );
            settings.provider.unsloth_model = ini_value(
                &values,
                "provider",
                "unsloth_model",
                settings.provider.unsloth_model,
            );
            settings.provider.thinking_level = ini_value(
                &values,
                "provider",
                "thinking_level",
                settings.provider.thinking_level,
            );
            settings.provider.opencode_go_model = ini_value(
                &values,
                "provider",
                "opencode_go_model",
                settings.provider.opencode_go_model,
            );
            settings.provider.opencode_zen_model = ini_value(
                &values,
                "provider",
                "opencode_zen_model",
                settings.provider.opencode_zen_model,
            );
            settings.capture_folder =
                ini_value(&values, "app", "capture_folder", settings.capture_folder);
            settings.system_prompt =
                ini_value(&values, "app", "system_prompt", settings.system_prompt);
            settings.app_mode = ini_value(&values, "app", "app_mode", settings.app_mode);
            settings.dark_theme = ini_bool(&values, "app", "dark_theme", settings.dark_theme);
        }
    }
    settings
}

fn save_app_settings(settings: &AppSettings) {
    let contents = format!(
        "[provider]\nprovider={}\nlm_model={}\ngemini_model={}\ncerebras_model={}\nollama_model={}\nollama_cloud_model={}\nunsloth_model={}\nthinking_level={}\nopencode_go_model={}\nopencode_zen_model={}\n\n[app]\ncapture_folder={}\nsystem_prompt={}\napp_mode={}\ndark_theme={}\n",
        ini_escape(&settings.provider.provider),
        ini_escape(&settings.provider.lm_model),
        ini_escape(&settings.provider.gemini_model),
        ini_escape(&settings.provider.cerebras_model),
        ini_escape(&settings.provider.ollama_model),
        ini_escape(&settings.provider.ollama_cloud_model),
        ini_escape(&settings.provider.unsloth_model),
        ini_escape(&settings.provider.thinking_level),
        ini_escape(&settings.provider.opencode_go_model),
        ini_escape(&settings.provider.opencode_zen_model),
        ini_escape(settings.capture_folder.trim()),
        ini_escape(&settings.system_prompt),
        ini_escape(&settings.app_mode),
        if settings.dark_theme { "true" } else { "false" },
    );
    if let Some(path) = settings_write_path() {
        if let Err(error) = std::fs::write(path, contents) {
            log::warn!("Failed to save settings: {error}");
        }
    }
}

fn save_provider_config(config: &ProviderConfig) {
    let mut settings = load_app_settings();
    settings.provider = config.clone();
    save_app_settings(&settings);
}

fn load_provider_config() -> ProviderConfig {
    load_app_settings().provider
}

fn save_capture_folder(folder: &str) {
    let folder = folder.trim();
    if folder.is_empty() {
        return;
    }
    let mut settings = load_app_settings();
    settings.capture_folder = folder.to_string();
    save_app_settings(&settings);
}

fn save_system_prompt(prompt: &str) {
    let mut settings = load_app_settings();
    settings.system_prompt = if prompt.trim().is_empty() {
        DEFAULT_SYSTEM_PROMPT.to_string()
    } else {
        prompt.to_string()
    };
    save_app_settings(&settings);
}

fn save_app_mode(mode: &str) {
    let mut settings = load_app_settings();
    settings.app_mode = if mode.eq_ignore_ascii_case("capture") {
        "capture".to_string()
    } else {
        "ocr".to_string()
    };
    save_app_settings(&settings);
}

fn save_dark_theme(dark_theme: bool) {
    let mut settings = load_app_settings();
    settings.dark_theme = dark_theme;
    save_app_settings(&settings);
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
    thinking_level: String,
    provider: String,
    last_text: String,
    base_font_size: f32,
    overlay_bg_color: slint::Color,
    overlay_text_color: slint::Color,
    overlay_bg_opacity: f32,
    use_textbox: bool,
    capture_folder: String,
    selection_origin_x: i32,
    selection_origin_y: i32,
    selection_scale: f32,
    pending_selection: Option<SelectionPurpose>,
    recording: bool,
    recording_paused: bool,
    recording_started_at: Option<Instant>,
    recording_paused_at: Option<Instant>,
    recording_paused_total: Duration,
    recording_path: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionPurpose {
    ContinuousOcr,
    Capture,
    Record,
    ScrollCapture,
    Ocr,
    OcrTranslate,
    Vlm,
    ColorPicker,
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
    if text.is_empty() {
        return max_size;
    }

    // 1. Dynamic padding based on window size to maximize space in small overlays
    let padding_v = if height < 120.0 {
        (height * 0.2).max(20.0)
    } else {
        48.0
    };
    let padding_h = if width < 120.0 {
        (width * 0.1).max(12.0)
    } else {
        32.0
    };

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
                    let char_w = if (c >= '\u{3000}' && c <= '\u{9FFF}')
                        || (c >= '\u{AC00}' && c <= '\u{D7AF}')
                    {
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
            if total_height > available_h {
                return false;
            }
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
        rgba.as_raw(),
        width,
        height,
    );
    slint::Image::from_rgba8(buffer)
}

fn sync_capture_state(state: &mut AppState, main: &MainWindow) {
    state.api_endpoint = main.get_api_endpoint().to_string();
    state.api_key = main.get_api_key().to_string();
    state.model_name = main.get_model_name().to_string();
    state.interval_sec = main.get_interval();
    state.system_prompt = main.get_system_prompt().to_string();
    state.temperature = main.get_temperature();
    state.thinking_level = main.get_thinking_level().to_string();
    state.provider = main.get_api_type().to_string();
    state.base_font_size = main.get_base_font_size();
    state.overlay_bg_color = main.get_overlay_bg_color();
    state.overlay_text_color = main.get_overlay_text_color();
    state.overlay_bg_opacity = main.get_overlay_bg_opacity();
    state.use_textbox = main.get_use_textbox();
    state.capture_folder = main.get_capture_folder().to_string();
}

fn make_api_client(http: &reqwest::Client, main: &MainWindow) -> api::ApiClient {
    api::ApiClient::new(
        http.clone(),
        main.get_api_endpoint().to_string(),
        main.get_api_key().to_string(),
        main.get_model_name().to_string(),
        main.get_system_prompt().to_string(),
        main.get_temperature(),
        main.get_thinking_level().to_string(),
        main.get_api_type().to_string(),
    )
}

fn physical_selection_rect(
    state: &AppState,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> capture::CaptureRect {
    let scale = if state.selection_scale > 0.0 {
        state.selection_scale
    } else {
        1.0
    };
    capture::CaptureRect {
        x: state.selection_origin_x + (x * scale).round() as i32,
        y: state.selection_origin_y + (y * scale).round() as i32,
        width: (width * scale).round() as i32,
        height: (height * scale).round() as i32,
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{:02}:{:02}", (seconds / 60) % 60, seconds % 60)
}

fn current_recording_elapsed(state: &AppState) -> Duration {
    let Some(started) = state.recording_started_at else {
        return Duration::ZERO;
    };
    let paused = state.recording_paused_total
        + state
            .recording_paused_at
            .map(|at| at.elapsed())
            .unwrap_or_default();
    started.elapsed().saturating_sub(paused)
}

fn sync_capture_toolbar_size(toolbar: &CaptureToolbarWindow) {
    let height = if toolbar.get_recording()
        || (toolbar.get_status_text().is_empty() && toolbar.get_active_tooltip().is_empty())
    {
        48.0
    } else {
        68.0
    };
    toolbar
        .window()
        .set_size(slint::LogicalSize::new(CAPTURE_TOOLBAR_WIDTH, height));
}

const CAPTURE_TOOLBAR_WIDTH: f32 = 560.0;
const OCR_WINDOW_WIDTH: f32 = 400.0;
const OCR_WINDOW_CLOSED_HEIGHT: f32 = 880.0;
const OCR_WINDOW_STYLE_HEIGHT: f32 = 1000.0;

fn sync_ocr_window_size(main: &MainWindow) {
    let height = if main.get_show_style_settings() {
        OCR_WINDOW_STYLE_HEIGHT
    } else {
        OCR_WINDOW_CLOSED_HEIGHT
    };
    main.window()
        .set_size(slint::LogicalSize::new(OCR_WINDOW_WIDTH, height));
}

#[cfg(target_os = "windows")]
fn configure_main_window_native_theme(main: &MainWindow, dark_theme: bool) {
    let _ = main.window().with_winit_window(|winit_window| {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
        win_utils::set_mica_backdrop(hwnd);
        win_utils::set_title_bar_theme(hwnd, dark_theme);
    });
}

#[cfg(target_os = "windows")]
fn configure_capture_toolbar_native_window(toolbar: &CaptureToolbarWindow) {
    let _ = toolbar.window().with_winit_window(|winit_window| {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
        win_utils::set_layered(hwnd);
        win_utils::set_tool_window(hwnd, true);
        win_utils::set_exclude_from_capture(hwnd);
        win_utils::disable_window_transitions(hwnd);
    });
}

fn set_capture_toolbar_status(toolbar: &CaptureToolbarWindow, status: String) {
    toolbar.set_status_text(status.into());
    sync_capture_toolbar_size(toolbar);
}

fn position_capture_toolbar(toolbar: &CaptureToolbarWindow) {
    sync_capture_toolbar_size(toolbar);
    let (cursor_x, cursor_y) = capture::cursor_position();
    let monitor = {
        #[cfg(target_os = "windows")]
        {
            capture::native_monitor_rect_at_point(cursor_x, cursor_y)
        }
        #[cfg(not(target_os = "windows"))]
        {
            capture::monitor_rect_at_point(cursor_x, cursor_y).ok()
        }
    };
    let scale = toolbar.window().scale_factor().max(1.0);
    let (x, y) = if let Some(monitor) = monitor {
        let monitor_width = monitor.width as f32 / scale;
        let toolbar_width = CAPTURE_TOOLBAR_WIDTH;
        (
            monitor.x as f32 / scale + ((monitor_width - toolbar_width) / 2.0).max(12.0),
            monitor.y as f32 / scale + 18.0,
        )
    } else {
        // A safe fallback keeps the toolbar usable even while monitor enumeration is unavailable.
        (18.0, 18.0)
    };
    toolbar
        .window()
        .set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
            x, y,
        )));
}

fn show_capture_toolbar_at_top_center(toolbar: &CaptureToolbarWindow) -> bool {
    // Position before showing to avoid a visible jump, then repeat once the native window has
    // a monitor scale so the final location is exactly centered on the top edge.
    toolbar.set_active_tooltip(String::new().into());
    position_capture_toolbar(toolbar);
    if toolbar.show().is_err() {
        return false;
    }
    #[cfg(target_os = "windows")]
    configure_capture_toolbar_native_window(toolbar);
    position_capture_toolbar(toolbar);
    true
}

fn prepare_selection_window(
    selection: &SelectionWindow,
    state: &Arc<Mutex<AppState>>,
    purpose: SelectionPurpose,
    window_mode: bool,
    color_picker_mode: bool,
    selection_initialized: &Arc<Mutex<bool>>,
    hotkey_manager: &Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
    owner: Option<isize>,
) -> bool {
    let _ = selection.hide();
    selection.set_window_mode(window_mode);
    selection.set_color_picker_mode(color_picker_mode);
    selection.invoke_reset();

    let (cursor_x, cursor_y) = capture::cursor_position();
    let (monitor_rect, screenshot) = match capture::capture_monitor_at_point(cursor_x, cursor_y) {
        Ok((rect, image)) => (rect, image),
        Err(_) => {
            let Ok(image) = capture::capture_full_screen() else {
                if let Ok(mut state) = state.lock() {
                    state.pending_selection = None;
                }
                return false;
            };
            (
                capture::CaptureRect {
                    x: 0,
                    y: 0,
                    width: image.width() as i32,
                    height: image.height() as i32,
                },
                image,
            )
        }
    };

    let (width, height) = screenshot.dimensions();
    let scale = selection.window().scale_factor().max(1.0);
    {
        let mut state = state.lock().unwrap();
        state.pending_selection = Some(purpose);
        state.selection_origin_x = monitor_rect.x;
        state.selection_origin_y = monitor_rect.y;
        state.selection_scale = scale;
    }
    selection.set_screenshot(rgba_to_slint_image(screenshot));
    selection.window().set_size(slint::LogicalSize::new(
        width as f32 / scale,
        height as f32 / scale,
    ));
    selection
        .window()
        .set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
            monitor_rect.x as f32 / scale,
            monitor_rect.y as f32 / scale,
        )));

    let should_initialize = {
        let mut initialized = selection_initialized.lock().unwrap();
        if *initialized {
            false
        } else {
            *initialized = true;
            true
        }
    };
    let _ = selection.show();
    // Configure the native selection window only after show(). Winit creates secondary windows
    // lazily, so doing this before show() can leave the first capture action unconfigured.
    #[cfg(target_os = "windows")]
    if should_initialize {
        selection.window().with_winit_window(move |winit_window| {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = winit_window.window_handle() {
                if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                    let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
                    win_utils::set_tool_window(hwnd, true);
                    win_utils::set_exclude_from_capture(hwnd);
                    win_utils::disable_window_transitions(hwnd);
                    if let Some(owner) = owner {
                        win_utils::set_window_owner(
                            hwnd,
                            windows::Win32::Foundation::HWND(owner as _),
                        );
                    }
                }
            }
        });
    }
    if let Some(manager) = hotkey_manager {
        let _ = manager.register(esc_hotkey);
    }
    true
}

fn begin_toolbar_selection(
    toolbar: &CaptureToolbarWindow,
    selection_weak: &slint::Weak<SelectionWindow>,
    state: &Arc<Mutex<AppState>>,
    purpose: SelectionPurpose,
    window_mode: bool,
    color_picker_mode: bool,
    selection_initialized: &Arc<Mutex<bool>>,
    hotkey_manager: &Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
    owner: Option<isize>,
) {
    if toolbar.get_recording() {
        return;
    }

    // Do not hide the toolbar or create/show another native window while Winit is dispatching
    // the toolbar's pointer event. On Windows that re-entrant window change can crash the app.
    let toolbar_weak = toolbar.as_weak();
    let selection_weak = selection_weak.clone();
    let state = state.clone();
    let selection_initialized = selection_initialized.clone();
    let hotkey_manager = hotkey_manager.clone();
    slint::Timer::single_shot(Duration::from_millis(1), move || {
        let Some(toolbar) = toolbar_weak.upgrade() else {
            return;
        };
        begin_toolbar_selection_now(
            &toolbar,
            &selection_weak,
            &state,
            purpose,
            window_mode,
            color_picker_mode,
            &selection_initialized,
            &hotkey_manager,
            esc_hotkey,
            owner,
        );
    });
}

fn begin_toolbar_selection_now(
    toolbar: &CaptureToolbarWindow,
    selection_weak: &slint::Weak<SelectionWindow>,
    state: &Arc<Mutex<AppState>>,
    purpose: SelectionPurpose,
    window_mode: bool,
    color_picker_mode: bool,
    selection_initialized: &Arc<Mutex<bool>>,
    hotkey_manager: &Option<Arc<GlobalHotKeyManager>>,
    esc_hotkey: HotKey,
    owner: Option<isize>,
) {
    if toolbar.get_recording() {
        return;
    }
    let Some(selection) = selection_weak.upgrade() else {
        return;
    };
    toolbar.set_active_tooltip(String::new().into());
    if toolbar.hide().is_err() {
        return;
    }

    // Let the hide request reach the native window before xcap captures the monitor. This also
    // prevents a second Winit window mutation from occurring in the toolbar's event turn.
    let toolbar_weak = toolbar.as_weak();
    let selection_weak = selection.as_weak();
    let state = state.clone();
    let selection_initialized = selection_initialized.clone();
    let hotkey_manager = hotkey_manager.clone();
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        let (Some(toolbar), Some(selection)) = (toolbar_weak.upgrade(), selection_weak.upgrade())
        else {
            return;
        };
        if toolbar.get_recording() {
            let _ = toolbar.show();
            return;
        }
        if !prepare_selection_window(
            &selection,
            &state,
            purpose,
            window_mode,
            color_picker_mode,
            &selection_initialized,
            &hotkey_manager,
            esc_hotkey,
            owner,
        ) {
            let _ = toolbar.show();
        }
    });
}

fn begin_fullscreen_toolbar_action(
    toolbar: &CaptureToolbarWindow,
    main: &MainWindow,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http: reqwest::Client,
) {
    if toolbar.get_recording() {
        return;
    }

    // Match the selection actions above: native window changes must happen after the pointer
    // callback returns, not while the capture toolbar is still handling the click.
    let toolbar_weak = toolbar.as_weak();
    let main_weak = main.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1), move || {
        let (Some(toolbar), Some(main)) = (toolbar_weak.upgrade(), main_weak.upgrade()) else {
            return;
        };
        begin_fullscreen_toolbar_action_now(&toolbar, &main, state, recorder_slot, http);
    });
}

fn begin_fullscreen_toolbar_action_now(
    toolbar: &CaptureToolbarWindow,
    main: &MainWindow,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http: reqwest::Client,
) {
    if toolbar.get_recording() {
        return;
    }
    let action = if toolbar.get_record_mode() {
        SelectionPurpose::Record
    } else {
        SelectionPurpose::Capture
    };
    toolbar.set_active_tooltip(String::new().into());
    if toolbar.hide().is_err() {
        return;
    }

    // Give the native hide request a chance to complete before asking xcap for the monitor
    // image. In particular, this keeps the toolbar out of a full-screen capture on Windows.
    let toolbar_weak = toolbar.as_weak();
    let main_weak = main.as_weak();
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        let (Some(toolbar), Some(main)) = (toolbar_weak.upgrade(), main_weak.upgrade()) else {
            return;
        };
        let (cursor_x, cursor_y) = capture::cursor_position();
        let rect = match capture::monitor_rect_at_point(cursor_x, cursor_y) {
            Ok(rect) => rect,
            Err(error) => {
                set_capture_toolbar_status(&toolbar, format!("Error: {error}"));
                let _ = toolbar.show();
                return;
            }
        };
        let spawn_result = slint::spawn_local(run_toolbar_action(
            action,
            rect,
            None,
            main.as_weak(),
            toolbar.as_weak(),
            state,
            recorder_slot,
            http,
        ));
        if let Err(error) = spawn_result {
            log::error!("Failed to start capture action: {error:?}");
            set_capture_toolbar_status(&toolbar, format!("Error: {error:?}"));
            let _ = toolbar.show();
        }
    });
}

fn rgba_to_bgra_bytes(image: &image::RgbaImage) -> Vec<u8> {
    let mut bytes = image.as_raw().clone();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    bytes
}

fn compose_ocr_clipboard(original: &str, translated: Option<&str>) -> String {
    let original = original.trim();
    match translated.map(str::trim).filter(|text| !text.is_empty()) {
        Some(translated) => format!("{original}\n\n{translated}"),
        None => original.to_string(),
    }
}

fn spawn_recording_clock(toolbar: slint::Weak<CaptureToolbarWindow>, state: Arc<Mutex<AppState>>) {
    std::thread::spawn(move || loop {
        let (recording, elapsed) = {
            let guard = state.lock().unwrap();
            (guard.recording, current_recording_elapsed(&guard))
        };
        if !recording {
            break;
        }
        let elapsed_text = format_elapsed(elapsed);
        let toolbar_weak = toolbar.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(toolbar) = toolbar_weak.upgrade() {
                toolbar.set_recording_elapsed(elapsed_text.into());
            }
        });
        std::thread::sleep(Duration::from_millis(250));
    });
}

async fn run_toolbar_action(
    action: SelectionPurpose,
    rect: capture::CaptureRect,
    target: Option<capture::WindowTarget>,
    main: slint::Weak<MainWindow>,
    toolbar: slint::Weak<CaptureToolbarWindow>,
    state: Arc<Mutex<AppState>>,
    recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>>,
    http: reqwest::Client,
) {
    let Some(main) = main.upgrade() else {
        return;
    };
    let Some(toolbar) = toolbar.upgrade() else {
        return;
    };
    let configured_folder = {
        let mut state_guard = state.lock().unwrap();
        sync_capture_state(&mut state_guard, &main);
        state_guard.capture_folder.clone()
    };
    let result: Result<String> = async {
        match action {
            SelectionPurpose::Capture => {
                let image = tokio::task::spawn_blocking(move || {
                    if let Some(target) = target {
                        capture::capture_window(target)
                    } else {
                        capture::capture_area(&rect, &None)
                    }
                })
                .await
                .context("Capture worker stopped")??;
                let configured_folder = configured_folder.clone();
                let path = tokio::task::spawn_blocking(move || {
                    capture::save_png_and_copy_to(
                        &image,
                        "OCR_Translator_Capture",
                        Some(configured_folder.as_str()),
                    )
                })
                .await
                .context("Capture save worker stopped")??;
                Ok(format!("Saved and copied: {}", path.display()))
            }
            SelectionPurpose::ScrollCapture => {
                let target = target.context("A window target is required for scrolling capture")?;
                let image = tokio::task::spawn_blocking(move || capture::scrolling_capture(target))
                    .await
                    .context("Scrolling capture worker stopped")??;
                let configured_folder = configured_folder.clone();
                let path = tokio::task::spawn_blocking(move || {
                    capture::save_png_and_copy_to(
                        &image,
                        "OCR_Translator_ScrollCapture",
                        Some(configured_folder.as_str()),
                    )
                })
                .await
                .context("Scrolling capture save worker stopped")??;
                Ok(format!("Saved and copied: {}", path.display()))
            }
            SelectionPurpose::Record => {
                if recorder_slot.lock().unwrap().is_some() {
                    anyhow::bail!("A recording is already in progress");
                }
                let path = capture::unique_output_path_in(
                    "OCR_Translator_Recording",
                    "mp4",
                    Some(configured_folder.as_str()),
                )?;
                let recorder = capture::ScreenRecorder::start(rect, path.clone(), 30)?;
                {
                    let mut slot = recorder_slot.lock().unwrap();
                    *slot = Some(recorder);
                }
                {
                    let mut guard = state.lock().unwrap();
                    guard.recording = true;
                    guard.recording_paused = false;
                    guard.recording_started_at = Some(Instant::now());
                    guard.recording_paused_at = None;
                    guard.recording_paused_total = Duration::ZERO;
                    guard.recording_path = Some(path);
                }
                toolbar.set_recording(true);
                toolbar.set_recording_paused(false);
                toolbar.set_recording_elapsed("00:00".into());
                set_capture_toolbar_status(&toolbar, String::new());
                let _ = toolbar.show();
                spawn_recording_clock(toolbar.as_weak(), state);
                Ok("Recording started".to_string())
            }
            SelectionPurpose::Ocr | SelectionPurpose::OcrTranslate | SelectionPurpose::Vlm => {
                let image =
                    tokio::task::spawn_blocking(move || capture::capture_area(&rect, &None))
                        .await
                        .context("Capture worker stopped")??;
                if action == SelectionPurpose::Vlm {
                    let client = make_api_client(&http, &main);
                    let text = client.translate_image(&image).await?;
                    if text.trim().is_empty() {
                        anyhow::bail!("VLM returned no text");
                    }
                    capture::copy_text_to_clipboard(&text)?;
                    main.set_last_translated_text(clean_text(&text).into());
                    return Ok("VLM result copied to clipboard".to_string());
                }

                let width = image.width();
                let height = image.height();
                let pixels = rgba_to_bgra_bytes(&image);
                let text = tokio::task::spawn_blocking(move || {
                    ocr::recognize_text(&pixels, width, height)
                })
                .await
                .context("OCR worker stopped")??;
                if text.trim().is_empty() {
                    return Ok("No text was recognized".to_string());
                }
                let translation = if action == SelectionPurpose::OcrTranslate {
                    Some(make_api_client(&http, &main).translate_text(&text).await?)
                } else {
                    None
                };
                let composed = compose_ocr_clipboard(&text, translation.as_deref());
                capture::copy_text_to_clipboard(&composed)?;
                if translation.is_some() {
                    Ok("OCR text and translation copied".to_string())
                } else {
                    Ok(format!(
                        "Recognized text copied ({} characters)",
                        text.trim().chars().count()
                    ))
                }
            }
            SelectionPurpose::ContinuousOcr | SelectionPurpose::ColorPicker => {
                anyhow::bail!("This action requires a different selection flow")
            }
        }
    }
    .await;

    if action == SelectionPurpose::Record {
        return;
    }
    match result {
        Ok(message) => set_capture_toolbar_status(&toolbar, message),
        Err(error) => set_capture_toolbar_status(&toolbar, format!("Error: {error}")),
    }
    toolbar.set_recording(false);
    let _ = toolbar.show();
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let main_window = MainWindow::new()?;
    let overlay_window = OverlayWindow::new()?;
    let selection_window = SelectionWindow::new()?;
    let textbox_window = TextboxWindow::new()?;
    let capture_toolbar = CaptureToolbarWindow::new()?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("Failed to build HTTP client");

    // Setup initial window states
    sync_ocr_window_size(&main_window);
    textbox_window
        .window()
        .set_size(slint::LogicalSize::new(600.0, 200.0));
    capture_toolbar
        .window()
        .set_size(slint::LogicalSize::new(CAPTURE_TOOLBAR_WIDTH, 48.0));
    let mut initial_settings = load_app_settings();
    let initial_capture_folder = if !initial_settings.capture_folder.trim().is_empty()
        && Path::new(initial_settings.capture_folder.trim()).is_dir()
    {
        initial_settings.capture_folder.trim().to_string()
    } else {
        capture::output_directory().to_string_lossy().to_string()
    };
    let initial_system_prompt = if initial_settings.system_prompt.trim().is_empty() {
        DEFAULT_SYSTEM_PROMPT.to_string()
    } else {
        initial_settings.system_prompt.clone()
    };
    let initial_app_mode = if initial_settings.app_mode.eq_ignore_ascii_case("capture") {
        "capture".to_string()
    } else {
        "ocr".to_string()
    };
    initial_settings.capture_folder = initial_capture_folder.clone();
    initial_settings.system_prompt = initial_system_prompt.clone();
    initial_settings.app_mode = initial_app_mode.clone();
    let initial_dark_theme = initial_settings.dark_theme;
    // Migrate the former JSON/TXT settings on startup and keep subsequent writes in one INI file.
    save_app_settings(&initial_settings);
    main_window.set_capture_folder(initial_capture_folder.clone().into());
    main_window.set_dark_theme(initial_dark_theme);
    capture_toolbar.set_dark_theme(initial_dark_theme);
    // Load saved provider configuration
    let config = initial_settings.provider.clone();

    // Initialize based on saved config (fallback to defaults if empty)
    if config.provider == PROVIDER_GEMINI {
        main_window.set_api_endpoint("https://generativelanguage.googleapis.com".into());
        main_window.set_api_key(get_gemini_key().unwrap_or_default().into());
        main_window.set_api_type(PROVIDER_GEMINI.into());
        main_window.set_api_type_index(1);

        let gemini_base: Vec<&str> = vec![
            "gemini-flash-lite-latest",
            "gemini-flash-latest",
            "gemini-pro-latest",
            "gemma-4-26b-a4b-it",
            "gemma-4-31b-it",
        ];
        let mut gemini_models: Vec<String> =
            gemini_base.into_iter().map(|s| s.to_string()).collect();
        if !config.gemini_model.is_empty() && !gemini_models.contains(&config.gemini_model) {
            gemini_models.push(config.gemini_model.clone());
        }
        let gemini_models_slint: Vec<slint::SharedString> =
            gemini_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(gemini_models_slint.as_slice()));

        let idx = gemini_models
            .iter()
            .position(|m| m == &config.gemini_model)
            .unwrap_or(0);
        main_window.set_model_name(gemini_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    } else if config.provider == PROVIDER_CEREBRAS {
        main_window.set_api_endpoint("https://api.cerebras.ai/v1".into());
        main_window.set_api_key(get_cerebras_key().unwrap_or_default().into());
        main_window.set_api_type(PROVIDER_CEREBRAS.into());
        main_window.set_api_type_index(2);

        let cerebras_base: Vec<&str> = vec!["gemma-4-31b", "gpt-oss-120b", "zai-glm-4.7"];
        let mut cerebras_models: Vec<String> =
            cerebras_base.into_iter().map(|s| s.to_string()).collect();
        if !config.cerebras_model.is_empty() && !cerebras_models.contains(&config.cerebras_model) {
            cerebras_models.push(config.cerebras_model.clone());
        }
        let cerebras_models_slint: Vec<slint::SharedString> =
            cerebras_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(cerebras_models_slint.as_slice()));

        let idx = cerebras_models
            .iter()
            .position(|m| m == &config.cerebras_model)
            .unwrap_or(0);
        main_window.set_model_name(cerebras_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    } else if config.provider == PROVIDER_OLLAMA {
        main_window.set_api_endpoint("http://localhost:11434/api".into());
        main_window.set_api_key("".into());
        main_window.set_api_type(PROVIDER_OLLAMA.into());
        main_window.set_api_type_index(3);

        let ollama_base: Vec<&str> = vec![
            "gemma4",
            "llava",
            "moondream",
            "qwen2.5vl:7b",
            "gpt-oss:120b-cloud",
        ];
        let mut ollama_models: Vec<String> =
            ollama_base.into_iter().map(|s| s.to_string()).collect();
        if !config.ollama_model.is_empty() && !ollama_models.contains(&config.ollama_model) {
            ollama_models.push(config.ollama_model.clone());
        }
        let ollama_models_slint: Vec<slint::SharedString> =
            ollama_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(ollama_models_slint.as_slice()));

        let idx = ollama_models
            .iter()
            .position(|m| m == &config.ollama_model)
            .unwrap_or(0);
        main_window.set_model_name(ollama_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    } else if config.provider == PROVIDER_OLLAMA_CLOUD {
        main_window.set_api_endpoint("https://ollama.com/api".into());
        main_window.set_api_key(get_ollama_cloud_key().unwrap_or_default().into());
        main_window.set_api_type(PROVIDER_OLLAMA_CLOUD.into());
        main_window.set_api_type_index(4);

        let ollama_cloud_base: Vec<&str> = vec!["gemma4:31b", "gpt-oss:120b", "gpt-oss:20b"];
        let mut ollama_cloud_models: Vec<String> = ollama_cloud_base
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        if !config.ollama_cloud_model.is_empty()
            && !ollama_cloud_models.contains(&config.ollama_cloud_model)
        {
            ollama_cloud_models.push(config.ollama_cloud_model.clone());
        }
        let ollama_cloud_models_slint: Vec<slint::SharedString> =
            ollama_cloud_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(ollama_cloud_models_slint.as_slice()));

        let idx = ollama_cloud_models
            .iter()
            .position(|m| m == &config.ollama_cloud_model)
            .unwrap_or(0);
        main_window.set_model_name(ollama_cloud_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    } else if config.provider == PROVIDER_UNSLOTH {
        main_window.set_api_endpoint("http://localhost:8888/v1".into());
        main_window.set_api_key(get_unsloth_key().unwrap_or_default().into());
        main_window.set_api_type(PROVIDER_UNSLOTH.into());
        main_window.set_api_type_index(5);

        let mut unsloth_models = vec!["default".to_string()];
        if !config.unsloth_model.is_empty() && !unsloth_models.contains(&config.unsloth_model) {
            unsloth_models.push(config.unsloth_model.clone());
        }
        let unsloth_models_slint: Vec<slint::SharedString> =
            unsloth_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(unsloth_models_slint.as_slice()));

        let idx = unsloth_models
            .iter()
            .position(|m| m == &config.unsloth_model)
            .unwrap_or(0);
        main_window.set_model_name(unsloth_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    } else if config.provider == PROVIDER_OPENCODE_GO {
        main_window.set_api_endpoint("https://opencode.ai/zen/go/v1".into());
        main_window.set_api_key(get_opencode_go_key().unwrap_or_default().into());
        main_window.set_api_type(PROVIDER_OPENCODE_GO.into());
        main_window.set_api_type_index(6);

        let mut opencode_go_models = vec![
            "kimi-k3".to_string(),
            "glm-5.3".to_string(),
            "deepseek-v4-flash".to_string(),
            "mimo-v2.5".to_string(),
            "gpt-5.6-luna".to_string(),
        ];
        if !config.opencode_go_model.is_empty()
            && !opencode_go_models.contains(&config.opencode_go_model)
        {
            opencode_go_models.push(config.opencode_go_model.clone());
        }
        let opencode_go_models_slint: Vec<slint::SharedString> =
            opencode_go_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(opencode_go_models_slint.as_slice()));

        let idx = opencode_go_models
            .iter()
            .position(|m| m == &config.opencode_go_model)
            .unwrap_or(0);
        main_window.set_model_name(opencode_go_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    } else if config.provider == PROVIDER_OPENCODE_ZEN {
        main_window.set_api_endpoint("https://opencode.ai/zen/v1".into());
        main_window.set_api_key(get_opencode_zen_key().unwrap_or_default().into());
        main_window.set_api_type(PROVIDER_OPENCODE_ZEN.into());
        main_window.set_api_type_index(7);

        let mut opencode_zen_models = vec![
            "gpt-5.5".to_string(),
            "claude-sonnet-4-6".to_string(),
            "deepseek-v4-flash".to_string(),
            "kimi-k3".to_string(),
        ];
        if !config.opencode_zen_model.is_empty()
            && !opencode_zen_models.contains(&config.opencode_zen_model)
        {
            opencode_zen_models.push(config.opencode_zen_model.clone());
        }
        let opencode_zen_models_slint: Vec<slint::SharedString> =
            opencode_zen_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(opencode_zen_models_slint.as_slice()));

        let idx = opencode_zen_models
            .iter()
            .position(|m| m == &config.opencode_zen_model)
            .unwrap_or(0);
        main_window.set_model_name(opencode_zen_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    } else {
        // LMStudio (default)
        main_window.set_api_endpoint("http://localhost:1234/v1".into());
        main_window.set_api_key("lm-studio".into());
        main_window.set_api_type(PROVIDER_LMSTUDIO.into());
        main_window.set_api_type_index(0);

        let default_model = get_model_name();
        let lm_base: Vec<&str> = vec![
            "unsloth/gemma-4-26b-a4b-it",
            "qwen/qwen3.5-9b",
            "translate-gemma-12b-it",
            "gemma-4-e4b-it",
            "gemma-4-31b-it",
            "qwen3.5-4b",
        ];
        let mut lm_models: Vec<String> = lm_base.into_iter().map(|s| s.to_string()).collect();
        if !lm_models.contains(&default_model) {
            lm_models.insert(0, default_model.clone());
        }
        if !config.lm_model.is_empty() && !lm_models.contains(&config.lm_model) {
            lm_models.push(config.lm_model.clone());
        }
        let lm_models_slint: Vec<slint::SharedString> =
            lm_models.iter().map(|s| s.into()).collect();
        main_window.set_model_options(slint::ModelRc::from(lm_models_slint.as_slice()));

        let idx = lm_models
            .iter()
            .position(|m| m == &config.lm_model)
            .unwrap_or(0);
        main_window.set_model_name(lm_models_slint[idx].clone());
        main_window.set_model_index(idx as i32);
    }

    main_window.set_thinking_level(configured_thinking_level(&config).into());
    main_window.set_system_prompt(initial_system_prompt.into());
    main_window.set_interval(0.0);
    main_window.set_base_font_size(16.0);

    // Initial Model Sync (Localhost/LM Studio)
    let main_weak_startup = main_window.as_weak();
    let http_startup = http_client.clone();
    slint::spawn_local(async move {
        if let Some(main) = main_weak_startup.upgrade() {
            let endpoint = main.get_api_endpoint().to_string();
            let api_key = main.get_api_key().to_string();

            if endpoint.contains("localhost")
                || endpoint.contains("127.0.0.1")
                || endpoint.contains("ollama.com")
                || endpoint.contains("opencode.ai")
            {
                let saved_config = load_provider_config();
                let provider = main.get_api_type().to_string();
                let client = api::ApiClient::new(
                    http_startup,
                    endpoint,
                    api_key,
                    String::new(),
                    String::new(),
                    0.0,
                    "default".to_string(),
                    provider.clone(),
                );
                if let Ok(models) = client.get_models().await {
                    let slint_models: Vec<slint::SharedString> =
                        models.into_iter().map(|s| s.into()).collect();
                    let current_model_str = main.get_model_name().as_str().to_string();
                    let default_model_str = get_model_name();
                    let saved_model_str = saved_model_for_provider(&saved_config, &provider);

                    // Debug: print what models we got from LM Studio
                    println!(
                        "[Startup Sync] Models from API: {:?}",
                        slint_models
                            .iter()
                            .map(|m| m.as_str().to_string())
                            .collect::<Vec<_>>()
                    );
                    println!(
                        "[Startup Sync] Looking for current: {:?}, default: {:?}",
                        current_model_str, default_model_str
                    );

                    main.set_model_options(slint::ModelRc::from(slint_models.as_slice()));

                    let mut found_index = None;
                    if let Some(idx) = slint_models
                        .iter()
                        .position(|m| m.as_str() == current_model_str)
                    {
                        found_index = Some(idx);
                    } else if let Some(idx) = slint_models
                        .iter()
                        .position(|m| m.as_str() == default_model_str)
                    {
                        found_index = Some(idx);
                    } else if !saved_model_str.is_empty() {
                        if let Some(idx) = slint_models
                            .iter()
                            .position(|m| m.as_str() == saved_model_str)
                        {
                            found_index = Some(idx);
                        }
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
    })
    .unwrap();

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
                    win_utils::set_title_bar_theme(hwnd, initial_dark_theme);
                    hwnd_out = Some(hwnd);
                }
            }
        });
        hwnd_out
    };

    // The capture toolbar's native handle is configured after its first show. Winit creates
    // native windows lazily, so an accessor call here (before the event loop starts) is a no-op.

    #[cfg(target_os = "windows")]
    let folder_owner = main_hwnd.map(|hwnd| hwnd.0 as isize);
    #[cfg(not(target_os = "windows"))]
    let folder_owner: Option<isize> = None;

    let main_weak = main_window.as_weak();
    let main_weak_api = main_window.as_weak();
    let main_weak_api_key = main_window.as_weak();
    let overlay_weak = overlay_window.as_weak();
    let selection_weak = selection_window.as_weak();
    let textbox_weak = textbox_window.as_weak();
    let state_clone = state.clone();

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
    main_window.on_theme_toggle_clicked(move || {
        let Some(main) = main_weak_main_theme.upgrade() else {
            return;
        };
        let dark_theme = !main.get_dark_theme();
        main.set_dark_theme(dark_theme);
        if let Some(toolbar) = toolbar_weak_main_theme.upgrade() {
            toolbar.set_dark_theme(dark_theme);
        }
        #[cfg(target_os = "windows")]
        configure_main_window_native_theme(&main, dark_theme);
        save_dark_theme(dark_theme);
    });

    // Apply the persisted title-bar theme once Winit has created the native OCR window. This
    // timer is necessary for a dark theme restored on startup; the initial pre-run accessor may
    // not have a native handle yet.
    #[cfg(target_os = "windows")]
    {
        let main_weak_native_theme = main_window.as_weak();
        slint::Timer::single_shot(Duration::from_millis(0), move || {
            if let Some(main) = main_weak_native_theme.upgrade() {
                configure_main_window_native_theme(&main, initial_dark_theme);
            }
        });
    }

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
                    configure_main_window_native_theme(&main, main.get_dark_theme());
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
    capture_toolbar.on_theme_toggle_clicked(move || {
        let Some(toolbar) = toolbar_weak_toolbar_theme.upgrade() else {
            return;
        };
        let dark_theme = !toolbar.get_dark_theme();
        toolbar.set_dark_theme(dark_theme);
        if let Some(main) = main_weak_toolbar_theme.upgrade() {
            main.set_dark_theme(dark_theme);
            #[cfg(target_os = "windows")]
            configure_main_window_native_theme(&main, dark_theme);
        }
        save_dark_theme(dark_theme);
    });

    // The toolbar's small UI button is the reverse path back to the full OCR settings UI.
    let main_weak_toolbar_ui = main_window.as_weak();
    let toolbar_weak_toolbar_ui = capture_toolbar.as_weak();
    main_window.set_app_mode(initial_app_mode.clone().into());
    capture_toolbar.on_ui_toggle_clicked(move || {
        // A toolbar button is part of the window currently dispatching the pointer event.
        // Deferring the native show/hide pair avoids changing that window set re-entrantly,
        // which can terminate the Winit event loop on Windows.
        let main_weak = main_weak_toolbar_ui.clone();
        let toolbar_weak = toolbar_weak_toolbar_ui.clone();
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
            configure_main_window_native_theme(&main, main.get_dark_theme());
            save_app_mode("ocr");
            if let Some(toolbar) = toolbar_weak.upgrade() {
                let _ = toolbar.hide();
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
    }

    let recorder_slot: Arc<Mutex<Option<capture::ScreenRecorder>>> = Arc::new(Mutex::new(None));

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
                folder_owner,
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
                folder_owner,
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
                folder_owner,
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
                folder_owner,
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
                folder_owner,
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
                folder_owner,
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
                folder_owner,
            );
        }
    });

    let toolbar_weak_drag = capture_toolbar.as_weak();
    capture_toolbar.on_drag_requested(move || {
        #[cfg(target_os = "windows")]
        if let Some(toolbar) = toolbar_weak_drag.upgrade() {
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
        }
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
            return;
        }
        toolbar.set_recording(false);
        toolbar.set_recording_paused(false);
        let _ = slint::spawn_local(async move {
            let result = tokio::task::spawn_blocking(move || recorder.stop())
                .await
                .context("Recording worker stopped")
                .and_then(|result| result);
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
    });

    capture_toolbar.on_close_clicked(move || {
        std::process::exit(0);
    });
    capture_toolbar.window().on_close_requested(move || {
        std::process::exit(0);
    });

    // API Type Changed Callback
    let main_weak_api_config = main_window.as_weak();
    main_window.on_api_type_changed(move |api_type| {
        let main = main_weak_api.unwrap();
        let current = load_provider_config();
        if api_type == PROVIDER_GEMINI {
            main.set_api_endpoint("https://generativelanguage.googleapis.com".into());
            main.set_api_key(get_gemini_key().unwrap_or_default().into());

            let gemini_base: Vec<&str> = vec![
                "gemini-flash-lite-latest",
                "gemini-flash-latest",
                "gemini-pro-latest",
                "gemma-4-26b-a4b-it",
                "gemma-4-31b-it",
            ];
            let mut gemini_models: Vec<String> =
                gemini_base.into_iter().map(|s| s.to_string()).collect();
            if !current.gemini_model.is_empty() && !gemini_models.contains(&current.gemini_model) {
                gemini_models.push(current.gemini_model.clone());
            }
            let gemini_models_slint: Vec<slint::SharedString> =
                gemini_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(gemini_models_slint.as_slice()));

            let idx = gemini_models
                .iter()
                .position(|m| m == &current.gemini_model)
                .unwrap_or(0);
            main.set_model_name(gemini_models_slint[idx].clone());
            main.set_model_index(idx as i32);
            main.set_system_prompt(main.get_system_prompt());
        } else if api_type == PROVIDER_CEREBRAS {
            main.set_api_endpoint("https://api.cerebras.ai/v1".into());
            main.set_api_key(get_cerebras_key().unwrap_or_default().into());

            let cerebras_base: Vec<&str> = vec!["gemma-4-31b", "gpt-oss-120b", "zai-glm-4.7"];
            let mut cerebras_models: Vec<String> =
                cerebras_base.into_iter().map(|s| s.to_string()).collect();
            if !current.cerebras_model.is_empty()
                && !cerebras_models.contains(&current.cerebras_model)
            {
                cerebras_models.push(current.cerebras_model.clone());
            }
            let cerebras_models_slint: Vec<slint::SharedString> =
                cerebras_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(cerebras_models_slint.as_slice()));

            let idx = cerebras_models
                .iter()
                .position(|m| m == &current.cerebras_model)
                .unwrap_or(0);
            main.set_model_name(cerebras_models_slint[idx].clone());
            main.set_model_index(idx as i32);
        } else if api_type == PROVIDER_OLLAMA {
            main.set_api_endpoint("http://localhost:11434/api".into());
            main.set_api_key("".into());

            let ollama_base: Vec<&str> = vec![
                "gemma4",
                "llava",
                "moondream",
                "qwen2.5vl:7b",
                "gpt-oss:120b-cloud",
            ];
            let mut ollama_models: Vec<String> =
                ollama_base.into_iter().map(|s| s.to_string()).collect();
            if !current.ollama_model.is_empty() && !ollama_models.contains(&current.ollama_model) {
                ollama_models.push(current.ollama_model.clone());
            }
            let ollama_models_slint: Vec<slint::SharedString> =
                ollama_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(ollama_models_slint.as_slice()));

            let idx = ollama_models
                .iter()
                .position(|m| m == &current.ollama_model)
                .unwrap_or(0);
            main.set_model_name(ollama_models_slint[idx].clone());
            main.set_model_index(idx as i32);
        } else if api_type == PROVIDER_OLLAMA_CLOUD {
            main.set_api_endpoint("https://ollama.com/api".into());
            main.set_api_key(get_ollama_cloud_key().unwrap_or_default().into());

            let ollama_cloud_base: Vec<&str> = vec!["gemma4:31b", "gpt-oss:120b", "gpt-oss:20b"];
            let mut ollama_cloud_models: Vec<String> = ollama_cloud_base
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            if !current.ollama_cloud_model.is_empty()
                && !ollama_cloud_models.contains(&current.ollama_cloud_model)
            {
                ollama_cloud_models.push(current.ollama_cloud_model.clone());
            }
            let ollama_cloud_models_slint: Vec<slint::SharedString> =
                ollama_cloud_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(ollama_cloud_models_slint.as_slice()));

            let idx = ollama_cloud_models
                .iter()
                .position(|m| m == &current.ollama_cloud_model)
                .unwrap_or(0);
            main.set_model_name(ollama_cloud_models_slint[idx].clone());
            main.set_model_index(idx as i32);
        } else if api_type == PROVIDER_UNSLOTH {
            main.set_api_endpoint("http://localhost:8888/v1".into());
            main.set_api_key(get_unsloth_key().unwrap_or_default().into());

            let mut unsloth_models = vec!["default".to_string()];
            if !current.unsloth_model.is_empty() && !unsloth_models.contains(&current.unsloth_model)
            {
                unsloth_models.push(current.unsloth_model.clone());
            }
            let unsloth_models_slint: Vec<slint::SharedString> =
                unsloth_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(unsloth_models_slint.as_slice()));

            let idx = unsloth_models
                .iter()
                .position(|m| m == &current.unsloth_model)
                .unwrap_or(0);
            main.set_model_name(unsloth_models_slint[idx].clone());
            main.set_model_index(idx as i32);
        } else if api_type == PROVIDER_OPENCODE_GO {
            main.set_api_endpoint("https://opencode.ai/zen/go/v1".into());
            main.set_api_key(get_opencode_go_key().unwrap_or_default().into());

            let mut opencode_go_models = vec![
                "kimi-k3".to_string(),
                "glm-5.3".to_string(),
                "deepseek-v4-flash".to_string(),
                "mimo-v2.5".to_string(),
                "gpt-5.6-luna".to_string(),
            ];
            if !current.opencode_go_model.is_empty()
                && !opencode_go_models.contains(&current.opencode_go_model)
            {
                opencode_go_models.push(current.opencode_go_model.clone());
            }
            let opencode_go_models_slint: Vec<slint::SharedString> =
                opencode_go_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(opencode_go_models_slint.as_slice()));

            let idx = opencode_go_models
                .iter()
                .position(|m| m == &current.opencode_go_model)
                .unwrap_or(0);
            main.set_model_name(opencode_go_models_slint[idx].clone());
            main.set_model_index(idx as i32);
        } else if api_type == PROVIDER_OPENCODE_ZEN {
            main.set_api_endpoint("https://opencode.ai/zen/v1".into());
            main.set_api_key(get_opencode_zen_key().unwrap_or_default().into());

            let mut opencode_zen_models = vec![
                "glm-5".to_string(),
                "kimi-k3".to_string(),
                "deepseek-v4-flash".to_string(),
                "minimax-m3".to_string(),
            ];
            if !current.opencode_zen_model.is_empty()
                && !opencode_zen_models.contains(&current.opencode_zen_model)
            {
                opencode_zen_models.push(current.opencode_zen_model.clone());
            }
            let opencode_zen_models_slint: Vec<slint::SharedString> =
                opencode_zen_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(opencode_zen_models_slint.as_slice()));

            let idx = opencode_zen_models
                .iter()
                .position(|m| m == &current.opencode_zen_model)
                .unwrap_or(0);
            main.set_model_name(opencode_zen_models_slint[idx].clone());
            main.set_model_index(idx as i32);
        } else {
            main.set_api_endpoint("http://localhost:1234/v1".into());
            main.set_api_key("lm-studio".into());

            // Restore saved LMStudio model from config
            let default_model = get_model_name();
            let lm_base: Vec<&str> = vec![
                "unsloth/gemma-4-26b-a4b-it",
                "qwen/qwen3.5-9b",
                "translate-gemma-12b-it",
                "gemma-4-e4b-it",
                "gemma-4-31b-it",
                "qwen3.5-4b",
            ];
            let mut lm_models: Vec<String> = lm_base.into_iter().map(|s| s.to_string()).collect();
            if !lm_models.contains(&default_model) {
                lm_models.insert(0, default_model.clone());
            }
            if !current.lm_model.is_empty() && !lm_models.contains(&current.lm_model) {
                lm_models.push(current.lm_model.clone());
            }
            let lm_models_slint: Vec<slint::SharedString> =
                lm_models.iter().map(|s| s.into()).collect();
            main.set_model_options(slint::ModelRc::from(lm_models_slint.as_slice()));

            let idx = lm_models
                .iter()
                .position(|m| m == &current.lm_model)
                .unwrap_or(0);
            main.set_model_name(lm_models_slint[idx].clone());
            main.set_model_index(idx as i32);
        }

        // Save provider change with current model
        let config_main = main_weak_api_config.unwrap();
        let mut config = ProviderConfig {
            provider: api_type.to_string(),
            ..current
        };
        set_saved_model_for_provider(
            &mut config,
            api_type.as_str(),
            config_main.get_model_name().to_string(),
        );
        config.thinking_level = config_main.get_thinking_level().to_string();
        save_provider_config(&config);
    });

    let state_api_key = state.clone();
    main_window.on_api_key_changed(move |api_key| {
        if let Some(main) = main_weak_api_key.upgrade() {
            let api_key = api_key.to_string();
            if main.get_api_type().as_str() == PROVIDER_GEMINI {
                persist_google_api_key(&api_key);
            } else if main.get_api_type().as_str() == PROVIDER_CEREBRAS {
                persist_cerebras_api_key(&api_key);
            } else if main.get_api_type().as_str() == PROVIDER_OLLAMA_CLOUD {
                persist_ollama_cloud_api_key(&api_key);
            } else if main.get_api_type().as_str() == PROVIDER_UNSLOTH {
                persist_unsloth_api_key(&api_key);
            } else if main.get_api_type().as_str() == PROVIDER_OPENCODE_GO {
                persist_opencode_go_api_key(&api_key);
            } else if main.get_api_type().as_str() == PROVIDER_OPENCODE_ZEN {
                persist_opencode_zen_api_key(&api_key);
            }

            let mut s = state_api_key.lock().unwrap();
            s.api_key = api_key;
        }
    });

    // Model Selection Changed Callback (save the provider/model settings in ocr_trans.ini)
    let main_weak_model = main_window.as_weak();
    main_window.on_model_changed(move |model_name| {
        if let Some(main) = main_weak_model.upgrade() {
            let current = load_provider_config();
            let mut config = ProviderConfig {
                provider: main.get_api_type().to_string(),
                ..current
            };
            set_saved_model_for_provider(
                &mut config,
                main.get_api_type().as_str(),
                model_name.to_string(),
            );
            save_provider_config(&config);
        }
    });

    let main_weak_thinking = main_window.as_weak();
    let state_thinking = state.clone();
    main_window.on_thinking_level_changed(move |thinking_level| {
        if let Some(main) = main_weak_thinking.upgrade() {
            let mut config = load_provider_config();
            config.provider = main.get_api_type().to_string();
            config.thinking_level = thinking_level.to_string();
            set_saved_model_for_provider(
                &mut config,
                main.get_api_type().as_str(),
                main.get_model_name().to_string(),
            );
            save_provider_config(&config);
            if let Ok(mut state) = state_thinking.lock() {
                state.thinking_level = thinking_level.to_string();
            }
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
            let provider = main.get_api_type().to_string();
            let saved_config = load_provider_config();
            let client = api::ApiClient::new(
                http,
                endpoint,
                api_key,
                String::new(),
                String::new(),
                0.0,
                "default".to_string(),
                provider.clone(),
            );
            match client.get_models().await {
                Ok(models) => {
                    let slint_models: Vec<slint::SharedString> =
                        models.into_iter().map(|s| s.into()).collect();
                    let current_model_str = main.get_model_name().as_str().to_string();
                    let default_model_str = get_model_name();
                    let saved_model_str = saved_model_for_provider(&saved_config, &provider);

                    main.set_model_options(slint::ModelRc::from(slint_models.as_slice()));

                    let mut found_index = None;
                    if let Some(idx) = slint_models
                        .iter()
                        .position(|m| m.as_str() == current_model_str)
                    {
                        found_index = Some(idx);
                    } else if let Some(idx) = slint_models
                        .iter()
                        .position(|m| m.as_str() == default_model_str)
                    {
                        found_index = Some(idx);
                    } else if !saved_model_str.is_empty() {
                        if let Some(idx) = slint_models
                            .iter()
                            .position(|m| m.as_str() == saved_model_str)
                        {
                            found_index = Some(idx);
                        }
                    }

                    let main_weak = main.as_weak();
                    let provider_for_save = provider.clone();
                    slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                        if let Some(main) = main_weak.upgrade() {
                            let mut selected_model = None;
                            if let Some(idx) = found_index {
                                main.set_model_name(slint_models[idx].clone());
                                main.set_model_index(idx as i32);
                                selected_model = Some(slint_models[idx].as_str().to_string());
                            } else if let Some(first) = slint_models.first() {
                                main.set_model_name(first.clone());
                                main.set_model_index(0);
                                selected_model = Some(first.as_str().to_string());
                            }

                            if let Some(model) = selected_model {
                                let mut config = load_provider_config();
                                config.provider = provider_for_save.clone();
                                set_saved_model_for_provider(
                                    &mut config,
                                    &provider_for_save,
                                    model,
                                );
                                save_provider_config(&config);
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
        if main.get_api_type().as_str() == PROVIDER_GEMINI {
            persist_google_api_key(&api_key);
        } else if main.get_api_type().as_str() == PROVIDER_CEREBRAS {
            persist_cerebras_api_key(&api_key);
        } else if main.get_api_type().as_str() == PROVIDER_OLLAMA_CLOUD {
            persist_ollama_cloud_api_key(&api_key);
        } else if main.get_api_type().as_str() == PROVIDER_UNSLOTH {
            persist_unsloth_api_key(&api_key);
        } else if main.get_api_type().as_str() == PROVIDER_OPENCODE_GO {
            persist_opencode_go_api_key(&api_key);
        } else if main.get_api_type().as_str() == PROVIDER_OPENCODE_ZEN {
            persist_opencode_zen_api_key(&api_key);
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
    let main_hwnd_selection = main_hwnd;
    let hotkey_manager_trigger = hotkey_manager.clone();
    let esc_hotkey_trigger = esc_hotkey.clone();
    main_window.on_select_area_clicked(move || {
        let selection = s_weak.unwrap();
        let _ = selection.hide(); // Hide if already showing to avoid double dimming

        selection.set_window_mode(false);
        selection.set_color_picker_mode(false);
        selection.invoke_reset();

        // Stop active capture
        {
            let mut s = state_for_selection_trigger.lock().unwrap();
            s.is_running = false;
            s.pending_selection = Some(SelectionPurpose::ContinuousOcr);
        }
        if let Some(main) = main_weak_for_selection_trigger.upgrade() {
            main.set_is_running(false);
        }

        // Hide existing overlay if any
        if let Some(overlay) = overlay_weak_for_select.upgrade() {
            let _ = overlay.hide();
            overlay.set_translated_text("".into());
            overlay.set_show_text(false);
        }
        // Capture the monitor under the cursor, matching the reference selector's multi-monitor
        // behavior while retaining the original primary-monitor fallback.
        let (monitor_rect, screenshot) = {
            let (cursor_x, cursor_y) = capture::cursor_position();
            match capture::capture_monitor_at_point(cursor_x, cursor_y) {
                Ok((rect, image)) => (rect, Some(image)),
                Err(_) => (
                    capture::CaptureRect {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                    },
                    capture::capture_full_screen().ok(),
                ),
            }
        };
        if let Some(img) = screenshot {
            let (w, h) = img.dimensions();
            let slint_img = rgba_to_slint_image(img);
            selection.set_screenshot(slint_img);

            // Set window size to match physical screenshot dimensions
            let sf = selection.window().scale_factor().max(1.0);
            {
                let mut s = state_for_selection_trigger.lock().unwrap();
                s.selection_origin_x = monitor_rect.x;
                s.selection_origin_y = monitor_rect.y;
                s.selection_scale = sf;
            }
            selection
                .window()
                .set_size(slint::LogicalSize::new(w as f32 / sf, h as f32 / sf));
            selection
                .window()
                .set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
                    monitor_rect.x as f32 / sf,
                    monitor_rect.y as f32 / sf,
                )));
        }

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
    let state_for_selection_close = state.clone();
    let main_weak_for_selection_close = main_weak_for_selection.clone();
    let toolbar_weak_for_selection_close = capture_toolbar.as_weak();
    selection_window.on_closed(move || {
        let selection = selection_weak_for_close.unwrap();
        let _ = selection.hide();
        if let Ok(mut s) = state_for_selection_close.lock() {
            s.pending_selection = None;
        }
        if let Some(ref mgr) = hotkey_manager_for_close {
            let _ = mgr.unregister(esc_hotkey_for_close);
        }
        if let Some(main) = main_weak_for_selection_close.upgrade() {
            if main.get_app_mode() == "capture" {
                if let Some(toolbar) = toolbar_weak_for_selection_close.upgrade() {
                    if !toolbar.get_recording() {
                        let _ = toolbar.show();
                    }
                }
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
            if w < 5.0 || h < 5.0 {
                let _ = selection.hide();
                if let Some(ref manager) = hotkey_manager_area {
                    let _ = manager.unregister(esc_hotkey_area);
                }
                if let Some(toolbar) = toolbar_weak_for_selection_actions.upgrade() {
                    let _ = toolbar.show();
                }
                state_for_selection.lock().unwrap().pending_selection = None;
                return;
            }

            let rect = {
                let mut state = state_for_selection.lock().unwrap();
                state.pending_selection = None;
                physical_selection_rect(&state, x, y, w, h)
            };
            let _ = selection.hide();
            if let Some(ref manager) = hotkey_manager_area {
                let _ = manager.unregister(esc_hotkey_area);
            }

            if purpose == SelectionPurpose::ColorPicker {
                if let Some(toolbar) = toolbar_weak_for_selection_actions.upgrade() {
                    set_capture_toolbar_status(
                        &toolbar,
                        "Click a pixel to choose a color".to_string(),
                    );
                    let _ = toolbar.show();
                }
                return;
            }

            if let (Some(main), Some(toolbar)) = (
                main_weak_for_selection.upgrade(),
                toolbar_weak_for_selection_actions.upgrade(),
            ) {
                let _ = slint::spawn_local(run_toolbar_action(
                    purpose,
                    rect,
                    None,
                    main.as_weak(),
                    toolbar.as_weak(),
                    state_for_selection.clone(),
                    recorder_slot_for_selection_actions.clone(),
                    http_for_selection_actions.clone(),
                ));
            }
            return;
        }

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

            let _ = selection.hide();
            if let Some(ref mgr) = hotkey_manager_async {
                let _ = mgr.unregister(esc_hotkey_async);
            }
        });
        if let Err(error) = spawn_result {
            log::error!("Failed to start OCR selection action: {error:?}");
            let _ = selection.hide();
            if let Some(ref mgr) = hotkey_manager_area {
                let _ = mgr.unregister(esc_hotkey_area);
            }
            if let Some(toolbar) = toolbar_weak_for_selection_actions.upgrade() {
                let _ = toolbar.show();
            }
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

    let selection_weak_window_selected = selection_window.as_weak();
    let state_window_selected = state.clone();
    let hotkey_manager_window_selected = hotkey_manager.clone();
    let esc_hotkey_window_selected = esc_hotkey.clone();
    let main_weak_window_selected = main_window.as_weak();
    let toolbar_weak_window_selected = capture_toolbar.as_weak();
    let recorder_slot_window_selected = recorder_slot.clone();
    let http_window_selected = http_client.clone();
    selection_window.on_window_selected(move || {
        let Some(selection) = selection_weak_window_selected.upgrade() else {
            return;
        };
        let (purpose, origin_x, origin_y, scale, hover_x, hover_y, hover_w, hover_h) = {
            let mut state = state_window_selected.lock().unwrap();
            let Some(purpose) = state.pending_selection.take() else {
                return;
            };
            (
                purpose,
                state.selection_origin_x,
                state.selection_origin_y,
                state.selection_scale.max(1.0),
                selection.get_hover_x(),
                selection.get_hover_y(),
                selection.get_hover_w(),
                selection.get_hover_h(),
            )
        };
        let _ = selection.hide();
        if let Some(manager) = &hotkey_manager_window_selected {
            let _ = manager.unregister(esc_hotkey_window_selected);
        }

        if purpose == SelectionPurpose::ColorPicker || hover_w <= 0.0 || hover_h <= 0.0 {
            if let Some(toolbar) = toolbar_weak_window_selected.upgrade() {
                set_capture_toolbar_status(&toolbar, "No window was selected".to_string());
                let _ = toolbar.show();
            }
            return;
        }

        let center_x = origin_x + ((hover_x + hover_w / 2.0) * scale).round() as i32;
        let center_y = origin_y + ((hover_y + hover_h / 2.0) * scale).round() as i32;
        let Some(target) = capture::window_target_at_point(center_x, center_y) else {
            if let Some(toolbar) = toolbar_weak_window_selected.upgrade() {
                set_capture_toolbar_status(&toolbar, "No external window was selected".to_string());
                let _ = toolbar.show();
            }
            return;
        };
        let Some(main) = main_weak_window_selected.upgrade() else {
            return;
        };
        let Some(toolbar) = toolbar_weak_window_selected.upgrade() else {
            return;
        };
        let _ = slint::spawn_local(run_toolbar_action(
            purpose,
            target.bounds,
            Some(target),
            main.as_weak(),
            toolbar.as_weak(),
            state_window_selected.clone(),
            recorder_slot_window_selected.clone(),
            http_window_selected.clone(),
        ));
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
        let _ = selection.hide();
        if let Some(manager) = &hotkey_manager_color_picked {
            let _ = manager.unregister(esc_hotkey_color_picked);
        }
        if purpose != SelectionPurpose::ColorPicker {
            if let Some(toolbar) = toolbar_weak_color_picked.upgrade() {
                let _ = toolbar.show();
            }
            return;
        }

        let screen_x = origin_x + (x * scale).round() as i32;
        let screen_y = origin_y + (y * scale).round() as i32;
        let Some(toolbar) = toolbar_weak_color_picked.upgrade() else {
            return;
        };
        let _ = slint::spawn_local(async move {
            let result = tokio::task::spawn_blocking(move || {
                let color = capture::sample_pixel_at_point(screen_x, screen_y)?;
                let hex = format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]);
                capture::copy_text_to_clipboard(&hex)?;
                Ok::<String, anyhow::Error>(hex)
            })
            .await
            .context("Color picker worker stopped")
            .and_then(|result| result);
            match result {
                Ok(hex) => set_capture_toolbar_status(&toolbar, format!("Copied {hex}")),
                Err(error) => set_capture_toolbar_status(&toolbar, format!("Error: {error}")),
            }
            let _ = toolbar.show();
        });
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

    if let Err(error) = main_window.run() {
        log::error!("OCR Translator event loop stopped: {error:?}");
    }
    Ok(())
}
