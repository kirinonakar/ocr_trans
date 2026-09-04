use crate::capture_workflow::{sync_ocr_window_size, CAPTURE_TOOLBAR_WIDTH};
use crate::settings::*;
use crate::{api, capture, CaptureToolbarWindow, MainWindow, TextboxWindow};
use slint::ComponentHandle;
use std::path::Path;

pub(crate) struct StartupState {
    pub(crate) capture_folder: String,
    pub(crate) app_mode: String,
    pub(crate) dark_theme: bool,
}

pub(crate) fn initialize_ui(
    main_window: &MainWindow,
    textbox_window: &TextboxWindow,
    capture_toolbar: &CaptureToolbarWindow,
    http_client: &reqwest::Client,
) -> StartupState {
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

    StartupState {
        capture_folder: initial_capture_folder,
        app_mode: initial_app_mode,
        dark_theme: initial_dark_theme,
    }
}
