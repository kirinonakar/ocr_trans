use crate::settings::*;
use crate::state::AppState;
use crate::{api, MainWindow};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

pub(crate) fn register_callbacks(
    main_window: &MainWindow,
    state: Arc<Mutex<AppState>>,
    http_client: reqwest::Client,
) {
    let main_weak_api = main_window.as_weak();
    let main_weak_api_key = main_window.as_weak();
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
}
