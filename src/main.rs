#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod about_ui;
mod api;
mod app;
mod autostart;
mod capture;
mod capture_workflow;
mod credentials;
mod fonts;
mod image_processing;
mod lifecycle;
mod markdown;
mod magnifier;
mod ocr;
mod ocr_ui;
mod overlay_style;
mod provider_ui;
mod providers;
mod runtime_workers;
mod scroll_alignment;
mod selection_ui;
mod settings;
mod shortcuts;
mod shortcuts_ui;
mod startup;
mod state;
mod text_layout;
mod toolbar_ui;
mod win_utils;
mod window_mode;
mod window_position;

fn main() -> anyhow::Result<()> {
    if magnifier::run_restore_watchdog()? {
        return Ok(());
    }
    env_logger::init();
    let Some(instance) = lifecycle::NativeHost::start()? else {
        return Ok(());
    };
    let hotkey_manager = global_hotkey::GlobalHotKeyManager::new().ok().map(std::sync::Arc::new);
    let hotkey_state = std::sync::Arc::new(std::sync::Mutex::new(shortcuts::HotkeyState::default()));
    shortcuts::apply_config(
        hotkey_manager.as_deref(),
        &hotkey_state,
        settings::load_shortcut_config(),
    );
    let start_in_tray = instance.start_in_tray();
    if start_in_tray && !instance.wait_in_tray()? {
        return Ok(());
    }

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_keep_alive(std::time::Duration::from_secs(15))
        .enable_all()
        .build()?
        .block_on(app::run(instance, start_in_tray, hotkey_manager, hotkey_state))
}
