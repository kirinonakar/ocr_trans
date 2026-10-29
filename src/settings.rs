use crate::credentials;
use crate::shortcuts::ShortcutConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::providers::{provider, PROVIDERS};

fn read_txt_key(file_name: &str) -> Option<String> {
    let directories = std::env::current_dir().ok().into_iter().chain(
        std::env::current_exe().ok().and_then(|path| path.parent().map(PathBuf::from)),
    );
    directories.filter_map(|directory| std::fs::read_to_string(directory.join(file_name)).ok())
        .map(|key| key.trim().to_string()).find(|key| !key.is_empty())
}

pub(crate) fn get_provider_key(name: &str) -> String {
    let definition = provider(name);
    if let Some(key) = definition.key_file.and_then(read_txt_key) {
        persist_provider_key(name, &key);
        return key;
    }
    credentials::read_api_key(definition)
        .or_else(|| definition.key_env.and_then(|name| std::env::var(name).ok()))
        .unwrap_or_else(|| definition.default_key.to_string())
}

pub(crate) fn persist_provider_key(name: &str, api_key: &str) {
    if let Err(err) = credentials::store_api_key(provider(name), api_key) {
        log::warn!("Failed to update {name} API key in Credential Manager: {err:?}");
    }
}

pub(crate) const DEFAULT_SYSTEM_PROMPT: &str =
    "naturally translate into korean. only show translated texts.";
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

pub(crate) fn get_model_name() -> String {
    let default = PROVIDERS[0].models[0];
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
pub(crate) struct ProviderConfig {
    pub(crate) provider: String,
    #[serde(default)]
    pub(crate) thinking_level: String,
    // Keep the former JSON/INI model keys for migration and backwards compatibility.
    #[serde(flatten)]
    pub(crate) models: HashMap<String, String>,
}

pub(crate) fn configured_thinking_level(config: &ProviderConfig) -> String {
    match config.thinking_level.trim().to_lowercase().as_str() {
        "disable" | "disabled" => "disable".to_string(),
        "low" | "medium" | "high" | "xhigh" | "max" => config.thinking_level.trim().to_lowercase(),
        _ => "default".to_string(),
    }
}

pub(crate) fn saved_model_for_provider(config: &ProviderConfig, provider: &str) -> String {
    config.models.get(crate::providers::provider(provider).model_key).cloned().unwrap_or_default()
}

pub(crate) fn set_saved_model_for_provider(config: &mut ProviderConfig, name: &str, model: String) {
    config.models.insert(provider(name).model_key.to_string(), model);
}

#[derive(Default, Clone)]
pub(crate) struct AppSettings {
    pub(crate) provider: ProviderConfig,
    pub(crate) capture_folder: String,
    pub(crate) system_prompt: String,
    pub(crate) system_prompt_presets: Vec<PromptPreset>,
    pub(crate) app_mode: String,
    pub(crate) dark_theme: bool,
    pub(crate) ocr_language: String,
    pub(crate) skip_empty_ocr: bool,
    pub(crate) translation_glossary: String,
    pub(crate) base_font_size: f32,
    pub(crate) result_font_family: String,
    pub(crate) overlay_bg_color: String,
    pub(crate) overlay_text_color: String,
    pub(crate) overlay_bg_opacity: f32,
    pub(crate) style_panel_open: bool,
    pub(crate) api_settings_collapsed: bool,
    pub(crate) api_settings_saved_height: f32,
    pub(crate) ocr_window_position: Option<(i32, i32)>,
    pub(crate) textbox_window_position: Option<(i32, i32)>,
    pub(crate) shortcuts: ShortcutConfig,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub(crate) struct PromptPreset {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) prompt: String,
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

fn ini_bool(
    values: &HashMap<(String, String), String>,
    section: &str,
    key: &str,
    fallback: bool,
) -> bool {
    let fallback = if fallback { "true" } else { "false" };
    matches!(
        ini_value(values, section, key, fallback.to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn ini_window_position(
    values: &HashMap<(String, String), String>,
    window: &str,
) -> Option<(i32, i32)> {
    let x = ini_value(values, "windows", &format!("{window}_x"), String::new())
        .parse()
        .ok()?;
    let y = ini_value(values, "windows", &format!("{window}_y"), String::new())
        .parse()
        .ok()?;
    Some((x, y))
}

fn ini_float(
    values: &HashMap<(String, String), String>,
    section: &str,
    key: &str,
    fallback: f32,
) -> f32 {
    ini_value(values, section, key, fallback.to_string())
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
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

fn settings_cache() -> &'static std::sync::Mutex<AppSettings> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<AppSettings>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(read_app_settings()))
}

pub(crate) fn load_app_settings() -> AppSettings {
    settings_cache().lock().unwrap().clone()
}

/// Only copy the values used by the continuous capture tick.
pub(crate) fn continuous_ocr_settings() -> (String, bool) {
    let settings = settings_cache().lock().unwrap();
    (settings.ocr_language.clone(), settings.skip_empty_ocr)
}

pub(crate) fn save_translation_options(skip_empty_ocr: bool, glossary: &str) {
    update_settings(|settings| {
        settings.skip_empty_ocr = skip_empty_ocr;
        settings.translation_glossary = glossary.to_string();
    });
}

enum SaveMessage {
    Changed,
    Flush(std::sync::mpsc::Sender<()>),
}

fn settings_writer() -> &'static std::sync::mpsc::Sender<SaveMessage> {
    static WRITER: std::sync::OnceLock<std::sync::mpsc::Sender<SaveMessage>> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(mut message) = rx.recv() {
                // Coalesce slider drags and related changes without touching disk on the UI thread.
                while matches!(message, SaveMessage::Changed) {
                    match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                        Ok(next) => message = next,
                        Err(_) => break,
                    }
                }
                let snapshot = load_app_settings();
                write_app_settings(&snapshot);
                if let SaveMessage::Flush(done) = message { let _ = done.send(()); }
            }
        });
        tx
    })
}

fn update_settings(update: impl FnOnce(&mut AppSettings)) {
    let mut settings = settings_cache().lock().unwrap();
    update(&mut settings);
    let _ = settings_writer().send(SaveMessage::Changed);
}

pub(crate) fn save_app_settings(settings: &AppSettings) {
    update_settings(|current| *current = settings.clone());
}

pub(crate) fn flush_settings() {
    let (done, completed) = std::sync::mpsc::channel();
    if settings_writer().send(SaveMessage::Flush(done)).is_ok() {
        let _ = completed.recv();
    }
}

fn read_app_settings() -> AppSettings {
    let legacy_provider = load_legacy_provider_config();
    let mut settings = AppSettings {
        provider: legacy_provider,
        capture_folder: legacy_capture_folder().unwrap_or_default(),
        system_prompt: read_legacy_system_prompt()
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
        system_prompt_presets: Vec::new(),
        app_mode: "ocr".to_string(),
        dark_theme: false,
        ocr_language: String::new(),
        skip_empty_ocr: false,
        translation_glossary: String::new(),
        base_font_size: 16.0,
        result_font_family: crate::fonts::default_result_font().to_string(),
        overlay_bg_color: "#ffffff".to_string(),
        overlay_text_color: "#1e293b".to_string(),
        overlay_bg_opacity: 0.9,
        style_panel_open: false,
        api_settings_collapsed: false,
        api_settings_saved_height: 0.0,
        ocr_window_position: None,
        textbox_window_position: None,
        shortcuts: ShortcutConfig::default(),
    };

    if let Some(path) = settings_read_path() {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let values = parse_ini(&contents);
            settings.provider.provider =
                ini_value(&values, "provider", "provider", settings.provider.provider);
            for definition in PROVIDERS {
                let saved = saved_model_for_provider(&settings.provider, definition.name);
                let model = ini_value(&values, "provider", definition.model_key, saved);
                set_saved_model_for_provider(&mut settings.provider, definition.name, model);
            }
            settings.provider.thinking_level = ini_value(
                &values, "provider", "thinking_level", settings.provider.thinking_level,
            );
            settings.capture_folder =
                ini_value(&values, "app", "capture_folder", settings.capture_folder);
            settings.system_prompt =
                ini_value(&values, "app", "system_prompt", settings.system_prompt);
            if let Ok(presets) = serde_json::from_str::<Vec<PromptPreset>>(&ini_value(
                &values,
                "app",
                "system_prompt_presets",
                String::new(),
            )) {
                settings.system_prompt_presets = presets
                    .into_iter()
                    .filter(|preset| !preset.name.trim().is_empty())
                    .collect();
            }
            settings.app_mode = ini_value(&values, "app", "app_mode", settings.app_mode);
            settings.dark_theme = ini_bool(&values, "app", "dark_theme", settings.dark_theme);
            settings.ocr_language = ini_value(&values, "app", "ocr_language", settings.ocr_language);
            settings.skip_empty_ocr = ini_bool(&values, "translation", "skip_empty_ocr", false);
            settings.translation_glossary = ini_value(&values, "translation", "glossary", String::new());
            settings.base_font_size =
                ini_float(&values, "style", "base_font_size", settings.base_font_size)
                    .clamp(8.0, 72.0);
            settings.result_font_family = crate::fonts::result_font_family(&ini_value(
                &values, "style", "result_font_family", settings.result_font_family,
            )).to_string();
            settings.overlay_bg_color = ini_value(
                &values,
                "style",
                "overlay_bg_color",
                settings.overlay_bg_color,
            );
            settings.overlay_text_color = ini_value(
                &values,
                "style",
                "overlay_text_color",
                settings.overlay_text_color,
            );
            settings.overlay_bg_opacity =
                ini_float(&values, "style", "overlay_bg_opacity", settings.overlay_bg_opacity)
                    .clamp(0.1, 1.0);
            settings.style_panel_open =
                ini_bool(&values, "ui", "style_panel_open", settings.style_panel_open);
            settings.api_settings_collapsed = ini_bool(
                &values,
                "ui",
                "api_settings_collapsed",
                settings.api_settings_collapsed,
            );
            settings.api_settings_saved_height = ini_float(
                &values,
                "ui",
                "api_settings_saved_height",
                settings.api_settings_saved_height,
            )
            .clamp(0.0, 1000.0);
            settings.shortcuts.select_area = ini_value(&values, "shortcuts", "select_area", settings.shortcuts.select_area);
            settings.shortcuts.start = ini_value(&values, "shortcuts", "start", settings.shortcuts.start);
            settings.shortcuts.toolbar1_action = ini_value(&values, "shortcuts", "toolbar1_action", settings.shortcuts.toolbar1_action);
            settings.shortcuts.toolbar1_key = ini_value(&values, "shortcuts", "toolbar1_key", settings.shortcuts.toolbar1_key);
            settings.shortcuts.toolbar2_action = ini_value(&values, "shortcuts", "toolbar2_action", settings.shortcuts.toolbar2_action);
            settings.shortcuts.toolbar2_key = ini_value(&values, "shortcuts", "toolbar2_key", settings.shortcuts.toolbar2_key);
            settings.shortcuts.toolbar3_action = ini_value(&values, "shortcuts", "toolbar3_action", settings.shortcuts.toolbar3_action);
            settings.shortcuts.toolbar3_key = ini_value(&values, "shortcuts", "toolbar3_key", settings.shortcuts.toolbar3_key);
            settings.ocr_window_position = ini_window_position(&values, "ocr");
            settings.textbox_window_position = ini_window_position(&values, "textbox");
        }
    }
    settings
}

fn atomic_write(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        // Both files are in the same directory. Rust replaces an existing Windows destination
        // with MoveFileExW(REPLACE_EXISTING); never delete the old settings before replacement.
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temporary); }
    result
}

fn serialize_app_settings(settings: &AppSettings) -> String {
    let models = PROVIDERS.iter().map(|definition| {
        format!("{}={}\n", definition.model_key, ini_escape(&saved_model_for_provider(&settings.provider, definition.name)))
    }).collect::<String>();
    let mut contents = format!(
        "[provider]\nprovider={}\n{}thinking_level={}\n\n[app]\ncapture_folder={}\nsystem_prompt={}\nsystem_prompt_presets={}\napp_mode={}\ndark_theme={}\nocr_language={}\n\n[style]\nbase_font_size={}\nresult_font_family={}\noverlay_bg_color={}\noverlay_text_color={}\noverlay_bg_opacity={}\n\n[ui]\nstyle_panel_open={}\napi_settings_collapsed={}\napi_settings_saved_height={}\n\n[shortcuts]\nselect_area={}\nstart={}\ntoolbar1_action={}\ntoolbar1_key={}\ntoolbar2_action={}\ntoolbar2_key={}\ntoolbar3_action={}\ntoolbar3_key={}\n",
        ini_escape(&settings.provider.provider),
        models,
        ini_escape(&settings.provider.thinking_level),
        ini_escape(settings.capture_folder.trim()),
        ini_escape(&settings.system_prompt),
        ini_escape(&serde_json::to_string(&settings.system_prompt_presets).unwrap_or_default()),
        ini_escape(&settings.app_mode),
        if settings.dark_theme { "true" } else { "false" },
        ini_escape(settings.ocr_language.trim()),
        settings.base_font_size,
        ini_escape(&settings.result_font_family),
        ini_escape(&settings.overlay_bg_color),
        ini_escape(&settings.overlay_text_color),
        settings.overlay_bg_opacity,
        if settings.style_panel_open { "true" } else { "false" },
        if settings.api_settings_collapsed { "true" } else { "false" },
        settings.api_settings_saved_height,
        ini_escape(&settings.shortcuts.select_area),
        ini_escape(&settings.shortcuts.start),
        ini_escape(&settings.shortcuts.toolbar1_action),
        ini_escape(&settings.shortcuts.toolbar1_key),
        ini_escape(&settings.shortcuts.toolbar2_action),
        ini_escape(&settings.shortcuts.toolbar2_key),
        ini_escape(&settings.shortcuts.toolbar3_action),
        ini_escape(&settings.shortcuts.toolbar3_key),
    );
    contents.push_str("\n[windows]\n");
    for (window, position) in [
        ("ocr", settings.ocr_window_position),
        ("textbox", settings.textbox_window_position),
    ] {
        if let Some((x, y)) = position {
            contents.push_str(&format!("{window}_x={x}\n{window}_y={y}\n"));
        }
    }
    contents.push_str(&format!(
        "\n[translation]\nskip_empty_ocr={}\nglossary={}\n",
        settings.skip_empty_ocr,
        ini_escape(&settings.translation_glossary)
    ));
    contents
}

fn write_app_settings(settings: &AppSettings) {
    let contents = serialize_app_settings(settings);
    if let Some(path) = settings_write_path() {
        if let Err(error) = atomic_write(&path, contents.as_bytes()) {
            log::warn!("Failed to save settings: {error}");
        }
    }
}

pub(crate) fn save_provider_config(config: &ProviderConfig) {
    update_settings(|settings| {
        settings.provider = config.clone();
    });
}

pub(crate) fn load_provider_config() -> ProviderConfig {
    load_app_settings().provider
}

pub(crate) fn save_capture_folder(folder: &str) {
    let folder = folder.trim();
    if folder.is_empty() {
        return;
    }
    update_settings(|settings| {
        settings.capture_folder = folder.to_string();
    });
}

pub(crate) fn save_system_prompt(prompt: &str) {
    update_settings(|settings| {
        settings.system_prompt = if prompt.trim().is_empty() {
            DEFAULT_SYSTEM_PROMPT.to_string()
        } else {
            prompt.to_string()
        };
    });
}

pub(crate) fn load_system_prompt_presets() -> Vec<PromptPreset> {
    load_app_settings().system_prompt_presets
}

pub(crate) fn save_system_prompt_presets(presets: &[PromptPreset]) {
    update_settings(|settings| {
        settings.system_prompt_presets = presets.to_vec();
    });
}

pub(crate) fn save_app_mode(mode: &str) {
    update_settings(|settings| {
        settings.app_mode = if mode.eq_ignore_ascii_case("capture") {
            "capture".to_string()
        } else {
            "ocr".to_string()
        };
    });
}

pub(crate) fn save_dark_theme(dark_theme: bool) {
    update_settings(|settings| {
        settings.dark_theme = dark_theme;
    });
}

pub(crate) fn save_ocr_window_position(position: (i32, i32)) {
    update_settings(|settings| settings.ocr_window_position = Some(position));
}

pub(crate) fn save_textbox_window_position(position: (i32, i32)) {
    update_settings(|settings| settings.textbox_window_position = Some(position));
}

pub(crate) fn save_ocr_language(language: &str) {
    update_settings(|settings| {
        settings.ocr_language = language.trim().to_string();
    });
}

/// Persists the shared textbox and overlay font.
pub(crate) fn save_result_font_family(family: &str) {
    update_settings(|settings| {
        settings.result_font_family = crate::fonts::result_font_family(family).to_string();
    });
}

/// Persists the Style panel values so the overlay keeps its last look across restarts.
pub(crate) fn save_style_settings(
    base_font_size: f32,
    overlay_bg_color: &str,
    overlay_text_color: &str,
    overlay_bg_opacity: f32,
) {
    update_settings(|settings| {
        settings.base_font_size = base_font_size.clamp(8.0, 72.0);
        settings.overlay_bg_color = overlay_bg_color.to_string();
        settings.overlay_text_color = overlay_text_color.to_string();
        settings.overlay_bg_opacity = overlay_bg_opacity.clamp(0.1, 1.0);
    });
}

/// Persists the fold states of the style panel and the API settings card.
pub(crate) fn save_ui_fold_states(
    style_panel_open: bool,
    api_settings_collapsed: bool,
    api_settings_saved_height: f32,
) {
    update_settings(|settings| {
        settings.style_panel_open = style_panel_open;
        settings.api_settings_collapsed = api_settings_collapsed;
        settings.api_settings_saved_height = api_settings_saved_height.clamp(0.0, 1000.0);
    });
}

/// Formats a Slint color as `#rrggbb` for the settings file.
pub(crate) fn color_to_hex(color: slint::Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red(), color.green(), color.blue())
}

/// Parses a `#rrggbb` color from the settings file. Returns `None` for other input.
pub(crate) fn color_from_hex(value: &str) -> Option<slint::Color> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(slint::Color::from_rgb_u8(red, green, blue))
}

/// Loads just the shortcut configuration.
pub(crate) fn load_shortcut_config() -> ShortcutConfig {
    load_app_settings().shortcuts
}

/// Persists the shortcut configuration while preserving every other setting.
pub(crate) fn save_shortcut_config(config: &ShortcutConfig) {
    update_settings(|settings| {
        settings.shortcuts = config.clone();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_positions_roundtrip_independently_and_reject_incomplete_or_invalid_pairs() {
        let settings = AppSettings {
            ocr_window_position: Some((-1840, 120)),
            textbox_window_position: Some((2400, -600)),
            ..Default::default()
        };
        let values = parse_ini(&serialize_app_settings(&settings));
        assert_eq!(ini_window_position(&values, "ocr"), Some((-1840, 120)));
        assert_eq!(ini_window_position(&values, "textbox"), Some((2400, -600)));
        for contents in [
            "[ui]\nstyle_panel_open=true\n",
            "[windows]\nocr_x=-1920\n",
            "[windows]\nocr_x=invalid\nocr_y=100\n",
            "[windows]\nocr_x=2147483648\nocr_y=100\n",
        ] {
            assert_eq!(ini_window_position(&parse_ini(contents), "ocr"), None);
        }
        let values = parse_ini(&serialize_app_settings(&AppSettings::default()));
        assert_eq!(ini_window_position(&values, "ocr"), None);
        assert_eq!(ini_window_position(&values, "textbox"), None);
    }

    #[test]
    fn provider_models_keep_legacy_json_and_ini_keys() {
        let mut legacy = serde_json::json!({"provider": "OpenCode Zen", "thinking_level": "high"});
        for definition in PROVIDERS {
            legacy[definition.model_key] = format!("saved {}", definition.name).into();
        }
        let mut config: ProviderConfig = serde_json::from_value(legacy).unwrap();
        for definition in PROVIDERS {
            assert_eq!(saved_model_for_provider(&config, definition.name), format!("saved {}", definition.name));
            set_saved_model_for_provider(&mut config, definition.name, format!("custom {}", definition.name));
        }
        let contents = serialize_app_settings(&AppSettings { provider: config.clone(), ..Default::default() });
        let ini = parse_ini(&contents);
        for definition in PROVIDERS {
            assert_eq!(ini_value(&ini, "provider", definition.model_key, String::new()), saved_model_for_provider(&config, definition.name));
        }
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["opencode_zen_model"], "custom OpenCode Zen");
        assert_eq!(configured_thinking_level(&config), "high");
    }

    #[test]
    fn atomic_settings_replace_existing_file_and_leave_old_data_on_failure() {
        let directory = std::env::temp_dir().join(format!("ocr-settings-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("settings.ini");
        atomic_write(&path, b"old settings").unwrap();
        atomic_write(&path, b"new settings").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new settings");
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // A real replacement failure on Windows: another reader denies delete sharing.
            let locked = std::fs::OpenOptions::new().read(true).share_mode(1).open(&path).unwrap();
            assert!(atomic_write(&path, b"replacement must fail").is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"new settings");
            drop(locked);
        }
        let protected_destination = directory.join("directory.ini");
        std::fs::create_dir(&protected_destination).unwrap();
        assert!(atomic_write(&protected_destination, b"must fail").is_err());
        assert!(protected_destination.is_dir());
        assert_eq!(std::fs::read(&path).unwrap(), b"new settings");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        std::fs::remove_dir(&protected_destination).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory).unwrap();
    }
}
