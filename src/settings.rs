use crate::credentials;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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

pub(crate) fn get_gemini_key() -> Option<String> {
    if let Some(key) = read_gemini_txt_key() {
        if let Err(err) = credentials::store_google_api_key(&key) {
            log::warn!("Failed to save gemini.txt key to Credential Manager: {err:?}");
        }
        return Some(key);
    }

    credentials::read_google_api_key()
}

pub(crate) fn persist_google_api_key(api_key: &str) {
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

pub(crate) fn get_cerebras_key() -> Option<String> {
    if let Some(key) = read_cerebras_txt_key() {
        if let Err(err) = credentials::store_cerebras_api_key(&key) {
            log::warn!("Failed to save cerebras.txt key to Credential Manager: {err:?}");
        }
        return Some(key);
    }
    credentials::read_cerebras_api_key()
}

pub(crate) fn persist_cerebras_api_key(api_key: &str) {
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

pub(crate) fn get_ollama_cloud_key() -> Option<String> {
    if let Some(key) = read_ollama_cloud_txt_key() {
        if let Err(err) = credentials::store_ollama_cloud_api_key(&key) {
            log::warn!("Failed to save ollama_cloud.txt key to Credential Manager: {err:?}");
        }
        return Some(key);
    }

    credentials::read_ollama_cloud_api_key().or_else(|| std::env::var("OLLAMA_API_KEY").ok())
}

pub(crate) fn persist_ollama_cloud_api_key(api_key: &str) {
    if let Err(err) = credentials::store_ollama_cloud_api_key(api_key) {
        log::warn!("Failed to update Ollama Cloud API key in Credential Manager: {err:?}");
    }
}

pub(crate) fn get_unsloth_key() -> Option<String> {
    credentials::read_unsloth_api_key().or_else(|| std::env::var("UNSLOTH_STUDIO_AUTH_TOKEN").ok())
}

pub(crate) fn persist_unsloth_api_key(api_key: &str) {
    if let Err(err) = credentials::store_unsloth_api_key(api_key) {
        log::warn!("Failed to update Unsloth Desktop API key in Credential Manager: {err:?}");
    }
}

pub(crate) fn get_opencode_go_key() -> Option<String> {
    credentials::read_opencode_go_api_key().or_else(|| std::env::var("OPENCODE_GO_API_KEY").ok())
}

pub(crate) fn persist_opencode_go_api_key(api_key: &str) {
    if let Err(err) = credentials::store_opencode_go_api_key(api_key) {
        log::warn!("Failed to update OpenCode Go API key in Credential Manager: {err:?}");
    }
}

pub(crate) fn get_opencode_zen_key() -> Option<String> {
    credentials::read_opencode_zen_api_key().or_else(|| std::env::var("OPENCODE_ZEN_API_KEY").ok())
}

pub(crate) fn persist_opencode_zen_api_key(api_key: &str) {
    if let Err(err) = credentials::store_opencode_zen_api_key(api_key) {
        log::warn!("Failed to update OpenCode Zen API key in Credential Manager: {err:?}");
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
pub(crate) struct ProviderConfig {
    pub(crate) provider: String,
    pub(crate) lm_model: String,
    pub(crate) gemini_model: String,
    #[serde(default)]
    pub(crate) cerebras_model: String,
    #[serde(default)]
    pub(crate) ollama_model: String,
    #[serde(default)]
    pub(crate) ollama_cloud_model: String,
    #[serde(default)]
    pub(crate) unsloth_model: String,
    #[serde(default)]
    pub(crate) thinking_level: String,
    #[serde(default)]
    pub(crate) opencode_go_model: String,
    #[serde(default)]
    pub(crate) opencode_zen_model: String,
}

pub(crate) const PROVIDER_LMSTUDIO: &str = "LMStudio";
pub(crate) const PROVIDER_GEMINI: &str = "Google Gemini";
pub(crate) const PROVIDER_CEREBRAS: &str = "Cerebras";
pub(crate) const PROVIDER_OLLAMA: &str = "Ollama";
pub(crate) const PROVIDER_OLLAMA_CLOUD: &str = "Ollama Cloud";
pub(crate) const PROVIDER_UNSLOTH: &str = "Unsloth Desktop";
pub(crate) const PROVIDER_OPENCODE_GO: &str = "OpenCode Go";
pub(crate) const PROVIDER_OPENCODE_ZEN: &str = "OpenCode Zen";

pub(crate) fn configured_thinking_level(config: &ProviderConfig) -> String {
    match config.thinking_level.trim().to_lowercase().as_str() {
        "disable" | "disabled" => "disable".to_string(),
        "low" | "medium" | "high" | "xhigh" | "max" => config.thinking_level.trim().to_lowercase(),
        _ => "default".to_string(),
    }
}

pub(crate) fn saved_model_for_provider(config: &ProviderConfig, provider: &str) -> String {
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

pub(crate) fn set_saved_model_for_provider(
    config: &mut ProviderConfig,
    provider: &str,
    model: String,
) {
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
pub(crate) struct AppSettings {
    pub(crate) provider: ProviderConfig,
    pub(crate) capture_folder: String,
    pub(crate) system_prompt: String,
    pub(crate) app_mode: String,
    pub(crate) dark_theme: bool,
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

pub(crate) fn load_app_settings() -> AppSettings {
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

pub(crate) fn save_app_settings(settings: &AppSettings) {
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

pub(crate) fn save_provider_config(config: &ProviderConfig) {
    let mut settings = load_app_settings();
    settings.provider = config.clone();
    save_app_settings(&settings);
}

pub(crate) fn load_provider_config() -> ProviderConfig {
    load_app_settings().provider
}

pub(crate) fn save_capture_folder(folder: &str) {
    let folder = folder.trim();
    if folder.is_empty() {
        return;
    }
    let mut settings = load_app_settings();
    settings.capture_folder = folder.to_string();
    save_app_settings(&settings);
}

pub(crate) fn save_system_prompt(prompt: &str) {
    let mut settings = load_app_settings();
    settings.system_prompt = if prompt.trim().is_empty() {
        DEFAULT_SYSTEM_PROMPT.to_string()
    } else {
        prompt.to_string()
    };
    save_app_settings(&settings);
}

pub(crate) fn save_app_mode(mode: &str) {
    let mut settings = load_app_settings();
    settings.app_mode = if mode.eq_ignore_ascii_case("capture") {
        "capture".to_string()
    } else {
        "ocr".to_string()
    };
    save_app_settings(&settings);
}

pub(crate) fn save_dark_theme(dark_theme: bool) {
    let mut settings = load_app_settings();
    settings.dark_theme = dark_theme;
    save_app_settings(&settings);
}
