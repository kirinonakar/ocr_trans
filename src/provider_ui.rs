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
        persist_provider_key(&current.provider, main.get_api_key().as_str());
        apply_provider(&main, api_type.as_str(), &current);

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
        if crate::providers::provider(api_type.as_str()).auto_sync {
            main.invoke_sync_lmstudio_models();
        }
    });

    let state_api_key = state.clone();
    main_window.on_api_key_changed(move |provider, api_key| {
        if let Some(main) = main_weak_api_key.upgrade() {
            let api_key = api_key.to_string();
            persist_provider_key(provider.as_str(), &api_key);

            let mut s = state_api_key.lock().unwrap();
            if main.get_api_type() == provider {
                s.api_key = api_key;
            }
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

    // Sync LMStudio Models Callback (triggered on provider switch to LMStudio)
    let main_weak_sync = main_window.as_weak();
    let http_sync = http_client.clone();
    main_window.on_sync_lmstudio_models(move || {
        let main = main_weak_sync.unwrap();
        let http = http_sync.clone();
        slint::spawn_local(sync_models(http, main)).unwrap();
    });

    // Refresh Models Callback
    let main_weak_refresh = main_window.as_weak();
    let http_refresh = http_client.clone();
    main_window.on_refresh_models_clicked(move || {
        let main = main_weak_refresh.unwrap();
        let api_key = main.get_api_key().to_string();
        persist_provider_key(main.get_api_type().as_str(), &api_key);
        let http = http_refresh.clone();
        slint::spawn_local(sync_models(http, main)).unwrap();
    });
}

/// Both startup and provider switching use exactly the same defaults and model selection.
pub(crate) fn apply_provider(main: &MainWindow, name: &str, config: &ProviderConfig) {
    let definition = crate::providers::provider(name);
    main.set_api_type(definition.name.into());
    main.set_api_type_index(
        crate::providers::PROVIDERS
            .iter()
            .position(|p| p.name == definition.name)
            .unwrap_or(0) as i32,
    );
    main.set_api_endpoint(definition.endpoint.into());
    main.set_api_key(get_provider_key(definition.name).into());
    let (models, index) = definition.model_options(
        &saved_model_for_provider(config, definition.name),
        &get_model_name(),
    );
    let models: Vec<slint::SharedString> = models.iter().map(|m| m.into()).collect();
    main.set_model_options(slint::ModelRc::from(models.as_slice()));
    main.set_model_name(models[index].clone());
    main.set_model_index(index as i32);
}

// Sync LMStudio Models helper (shared logic)
pub(crate) fn sync_models(
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
            endpoint.clone(),
            api_key,
            String::new(),
            String::new(),
            0.0,
            "default".to_string(),
            provider.clone(),
        );
        match client.get_models().await {
            Ok(models) => {
                if models.is_empty()
                    || main.get_api_type().as_str() != provider
                    || main.get_api_endpoint().as_str() != endpoint
                {
                    return;
                }
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
                        if main.get_api_type().as_str() != provider_for_save
                            || main.get_api_endpoint().as_str() != endpoint
                        {
                            return;
                        }
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
                            set_saved_model_for_provider(&mut config, &provider_for_save, model);
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
