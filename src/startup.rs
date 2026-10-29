use crate::capture_workflow::{sync_ocr_window_size, CAPTURE_TOOLBAR_WIDTH};
use crate::settings::*;
use crate::{capture, CaptureToolbarWindow, MainWindow, TextboxWindow};
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
    main_window.set_skip_empty_ocr(initial_settings.skip_empty_ocr);
    main_window.set_translation_glossary(initial_settings.translation_glossary.clone().into());
    // Windows OCR is enumerated on a worker thread: the `windows` crate initializes the caller
    // into the MTA, which makes Winit's later `OleInitialize` on the main thread fail.
    let language_menu = match std::thread::spawn({
        let saved_language = initial_settings.ocr_language.clone();
        move || crate::ocr::installed_language_menu(&saved_language)
    })
    .join()
    {
        Ok(Ok(menu)) => menu,
        Ok(Err(error)) => {
            log::warn!("Unable to initialize OCR languages: {error:#}");
            Default::default()
        }
        Err(_) => {
            // Join errors are recoverable only in unwind builds; release panics abort.
            log::warn!("The OCR language worker panicked");
            Default::default()
        }
    };
    let language_options: Vec<slint::SharedString> = language_menu
        .languages
        .iter()
        .map(|language| language.label.as_str().into())
        .collect();
    let language_tags: Vec<slint::SharedString> = language_menu
        .languages
        .iter()
        .map(|language| language.tag.as_str().into())
        .collect();
    capture_toolbar
        .set_ocr_language_options(std::rc::Rc::new(slint::VecModel::from(language_options)).into());
    capture_toolbar
        .set_ocr_language_tags(std::rc::Rc::new(slint::VecModel::from(language_tags)).into());
    capture_toolbar.set_ocr_language_index(language_menu.selected.map_or(-1, |index| index as i32));
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
    // Reflect the stored shortcut bindings on the OCR window's action buttons.
    main_window.set_select_area_shortcut_label(
        crate::shortcuts::binding_label(&initial_settings.shortcuts.select_area).into(),
    );
    main_window.set_start_shortcut_label(
        crate::shortcuts::binding_label(&initial_settings.shortcuts.start).into(),
    );
    main_window.set_dark_theme(initial_dark_theme);
    textbox_window.set_dark_theme(initial_dark_theme);
    capture_toolbar.set_dark_theme(initial_dark_theme);
    // Load saved provider configuration
    let config = initial_settings.provider.clone();

    main_window.set_provider_options(slint::ModelRc::from(
        crate::providers::PROVIDERS
            .iter()
            .map(|p| slint::SharedString::from(p.name))
            .collect::<Vec<_>>()
            .as_slice(),
    ));
    crate::provider_ui::apply_provider(main_window, &config.provider, &config);

    main_window.set_thinking_level(configured_thinking_level(&config).into());
    main_window.set_system_prompt(initial_system_prompt.into());
    // Mirror the stored system prompt presets on the OCR window's preset combo box.
    let preset_names: Vec<slint::SharedString> = initial_settings
        .system_prompt_presets
        .iter()
        .map(|preset| preset.name.as_str().into())
        .collect();
    main_window.set_preset_options(slint::ModelRc::from(preset_names.as_slice()));
    main_window.set_preset_index(
        initial_settings
            .system_prompt_presets
            .iter()
            .position(|preset| preset.prompt.trim() == initial_settings.system_prompt.trim())
            .map_or(-1, |index| index as i32),
    );
    main_window.set_interval(0.0);
    // Restore the Style panel values so the overlay keeps its last look across restarts.
    main_window.set_base_font_size(initial_settings.base_font_size);
    let font_families = crate::fonts::font_menu(&initial_settings.result_font_family);
    main_window.set_result_font_index(
        font_families.iter()
            .position(|name| name.eq_ignore_ascii_case(&initial_settings.result_font_family))
            .map_or(-1, |index| index as i32),
    );
    let font_options: Vec<slint::SharedString> = font_families.iter()
        .map(|name| name.as_str().into())
        .collect();
    main_window.set_result_font_options(slint::ModelRc::from(font_options.as_slice()));
    main_window.set_result_font_family(initial_settings.result_font_family.clone().into());
    textbox_window.set_result_font_family(initial_settings.result_font_family.clone().into());
    // Result visibility is session-only: every launch starts with the textbox closed.
    main_window.set_use_textbox(false);
    capture_toolbar.set_textbox_visible(false);
    if let Some(color) = color_from_hex(&initial_settings.overlay_bg_color) {
        main_window.set_overlay_bg_color(color);
    }
    if let Some(color) = color_from_hex(&initial_settings.overlay_text_color) {
        main_window.set_overlay_text_color(color);
    }
    main_window.set_overlay_bg_opacity(initial_settings.overlay_bg_opacity);
    // Restore the fold states of the style panel and the API card, then re-apply the window size.
    main_window.set_show_style_settings(initial_settings.style_panel_open);
    main_window.set_api_settings_collapsed(initial_settings.api_settings_collapsed);
    main_window.set_api_settings_saved_height(initial_settings.api_settings_saved_height);
    sync_ocr_window_size(main_window);

    crate::window_position::track(
        main_window.window(),
        initial_settings.ocr_window_position,
        save_ocr_window_position,
    );
    crate::window_position::track(
        textbox_window.window(),
        initial_settings.textbox_window_position,
        save_textbox_window_position,
    );

    if crate::providers::provider(main_window.get_api_type().as_str()).auto_sync {
        slint::spawn_local(crate::provider_ui::sync_models(
            http_client.clone(),
            main_window.clone_strong(),
        ))
        .unwrap();
    }

    StartupState {
        capture_folder: initial_capture_folder,
        app_mode: initial_app_mode,
        dark_theme: initial_dark_theme,
    }
}
