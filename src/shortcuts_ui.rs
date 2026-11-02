//! Wiring for the shortcut settings window.

use crate::settings::{load_shortcut_config, save_shortcut_config};
use crate::shortcuts::{
    action_options, apply_config, binding_label, build_binding, key_index, split_binding,
    HotkeyState, ShortcutConfig, ToolbarAction, KEY_TOKENS,
};
use crate::{MainWindow, ShortcutSettingsWindow};
use global_hotkey::GlobalHotKeyManager;
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn register_callbacks(
    main_window: &MainWindow,
    shortcut_window: &ShortcutSettingsWindow,
    manager: Option<Arc<GlobalHotKeyManager>>,
    state: Arc<Mutex<HotkeyState>>,
) {
    // Both models come from Rust so the index encoded by the Slint properties has a single
    // source of truth.
    shortcut_window.set_key_options(
        std::rc::Rc::new(slint::VecModel::from(
            KEY_TOKENS
                .iter()
                .map(|token| SharedString::from(*token))
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    shortcut_window.set_action_options(
        std::rc::Rc::new(slint::VecModel::from(
            action_options()
                .iter()
                .map(|label| SharedString::from(*label))
                .collect::<Vec<_>>(),
        ))
        .into(),
    );

    let shortcut_weak_open = shortcut_window.as_weak();
    let main_weak_open = main_window.as_weak();
    main_window.on_shortcut_settings_clicked(move || {
        let Some(window) = shortcut_weak_open.upgrade() else {
            return;
        };
        populate_window(&window, &load_shortcut_config());
        if let Some(main) = main_weak_open.upgrade() {
            window.set_dark_theme(main.get_dark_theme());
        }
        window.set_status_error(false);
        window.set_status_text(SharedString::new());

        // Secondary native windows are created lazily; defer show() until this click handler and
        // the current event-loop turn have finished.
        let window_weak = window.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            if window.show().is_ok() {
                #[cfg(target_os = "windows")]
                configure_shortcut_window_native(&window, window.get_dark_theme());
            }
        });
    });

    let shortcut_weak_save = shortcut_window.as_weak();
    let main_weak_save = main_window.as_weak();
    let manager_save = manager;
    let state_save = state;
    shortcut_window.on_save_clicked(move || {
        let Some(window) = shortcut_weak_save.upgrade() else {
            return;
        };
        let config = match read_config(&window) {
            Ok(config) => config,
            Err(message) => {
                window.set_status_error(true);
                window.set_status_text(SharedString::from(message));
                return;
            }
        };

        save_shortcut_config(&config);
        apply_config(manager_save.as_deref(), &state_save, config.clone());
        if let Some(main) = main_weak_save.upgrade() {
            main.set_select_area_shortcut_label(binding_label(&config.select_area).into());
            main.set_start_shortcut_label(binding_label(&config.start).into());
        }
        window.set_status_error(false);
        window.set_status_text(SharedString::from("Saved"));
    });

    let shortcut_weak_close = shortcut_window.as_weak();
    shortcut_window.on_close_clicked(move || {
        let window_weak = shortcut_weak_close.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            if let Some(window) = window_weak.upgrade() {
                let _ = window.hide();
            }
        });
    });

    shortcut_window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
}

/// Copies the persisted configuration into the window's editing properties.
fn populate_window(window: &ShortcutSettingsWindow, config: &ShortcutConfig) {
    let (win, alt, ctrl, shift, key) = split_binding(&config.select_area);
    window.set_select_win(win);
    window.set_select_alt(alt);
    window.set_select_ctrl(ctrl);
    window.set_select_shift(shift);
    window.set_select_key_index(key_index(&key));

    let (win, alt, ctrl, shift, key) = split_binding(&config.start);
    window.set_start_win(win);
    window.set_start_alt(alt);
    window.set_start_ctrl(ctrl);
    window.set_start_shift(shift);
    window.set_start_key_index(key_index(&key));

    let (win, alt, ctrl, shift, key) = split_binding(&config.restore_last_mode);
    window.set_restore_win(win);
    window.set_restore_alt(alt);
    window.set_restore_ctrl(ctrl);
    window.set_restore_shift(shift);
    window.set_restore_key_index(key_index(&key));

    let (win, alt, ctrl, shift, key) = split_binding(&config.toolbar1_key);
    window.set_tool1_win(win);
    window.set_tool1_alt(alt);
    window.set_tool1_ctrl(ctrl);
    window.set_tool1_shift(shift);
    window.set_tool1_key_index(key_index(&key));
    window.set_tool1_action_index(
        ToolbarAction::from_token(&config.toolbar1_action)
            .map(ToolbarAction::option_index)
            .unwrap_or(0),
    );

    let (win, alt, ctrl, shift, key) = split_binding(&config.toolbar2_key);
    window.set_tool2_win(win);
    window.set_tool2_alt(alt);
    window.set_tool2_ctrl(ctrl);
    window.set_tool2_shift(shift);
    window.set_tool2_key_index(key_index(&key));
    window.set_tool2_action_index(
        ToolbarAction::from_token(&config.toolbar2_action)
            .map(ToolbarAction::option_index)
            .unwrap_or(0),
    );

    let (win, alt, ctrl, shift, key) = split_binding(&config.toolbar3_key);
    window.set_tool3_win(win);
    window.set_tool3_alt(alt);
    window.set_tool3_ctrl(ctrl);
    window.set_tool3_shift(shift);
    window.set_tool3_key_index(key_index(&key));
    window.set_tool3_action_index(
        ToolbarAction::from_token(&config.toolbar3_action)
            .map(ToolbarAction::option_index)
            .unwrap_or(0),
    );

    let (win, alt, ctrl, shift, key) = split_binding(&config.toolbar4_key);
    window.set_tool4_win(win);
    window.set_tool4_alt(alt);
    window.set_tool4_ctrl(ctrl);
    window.set_tool4_shift(shift);
    window.set_tool4_key_index(key_index(&key));
    window.set_tool4_action_index(
        ToolbarAction::from_token(&config.toolbar4_action)
            .map(ToolbarAction::option_index)
            .unwrap_or(0),
    );

    let (win, alt, ctrl, shift, key) = split_binding(&config.toolbar5_key);
    window.set_tool5_win(win);
    window.set_tool5_alt(alt);
    window.set_tool5_ctrl(ctrl);
    window.set_tool5_shift(shift);
    window.set_tool5_key_index(key_index(&key));
    window.set_tool5_action_index(
        ToolbarAction::from_token(&config.toolbar5_action)
            .map(ToolbarAction::option_index)
            .unwrap_or(0),
    );
}

/// Reads and validates the editing properties.
fn read_config(window: &ShortcutSettingsWindow) -> Result<ShortcutConfig, String> {
    let select_area = slot_binding(
        window.get_select_win(),
        window.get_select_alt(),
        window.get_select_ctrl(),
        window.get_select_shift(),
        window.get_select_key_index(),
        "Select area",
        false,
    )?;
    let start = slot_binding(
        window.get_start_win(),
        window.get_start_alt(),
        window.get_start_ctrl(),
        window.get_start_shift(),
        window.get_start_key_index(),
        "Start",
        false,
    )?;

    let restore_last_mode = slot_binding(
        window.get_restore_win(),
        window.get_restore_alt(),
        window.get_restore_ctrl(),
        window.get_restore_shift(),
        window.get_restore_key_index(),
        "Restore/hide last mode",
        false,
    )?;

    let tool1_action = ToolbarAction::from_option_index(window.get_tool1_action_index());
    let toolbar1_key = slot_binding(
        window.get_tool1_win(),
        window.get_tool1_alt(),
        window.get_tool1_ctrl(),
        window.get_tool1_shift(),
        window.get_tool1_key_index(),
        "Capture toolbar shortcut 1",
        tool1_action.is_some(),
    )?;

    let tool2_action = ToolbarAction::from_option_index(window.get_tool2_action_index());
    let toolbar2_key = slot_binding(
        window.get_tool2_win(),
        window.get_tool2_alt(),
        window.get_tool2_ctrl(),
        window.get_tool2_shift(),
        window.get_tool2_key_index(),
        "Capture toolbar shortcut 2",
        tool2_action.is_some(),
    )?;

    let tool3_action = ToolbarAction::from_option_index(window.get_tool3_action_index());
    let toolbar3_key = slot_binding(
        window.get_tool3_win(),
        window.get_tool3_alt(),
        window.get_tool3_ctrl(),
        window.get_tool3_shift(),
        window.get_tool3_key_index(),
        "Capture toolbar shortcut 3",
        tool3_action.is_some(),
    )?;

    let tool4_action = ToolbarAction::from_option_index(window.get_tool4_action_index());
    let toolbar4_key = slot_binding(
        window.get_tool4_win(),
        window.get_tool4_alt(),
        window.get_tool4_ctrl(),
        window.get_tool4_shift(),
        window.get_tool4_key_index(),
        "Capture toolbar shortcut 4",
        tool4_action.is_some(),
    )?;

    let tool5_action = ToolbarAction::from_option_index(window.get_tool5_action_index());
    let toolbar5_key = slot_binding(
        window.get_tool5_win(),
        window.get_tool5_alt(),
        window.get_tool5_ctrl(),
        window.get_tool5_shift(),
        window.get_tool5_key_index(),
        "Capture toolbar shortcut 5",
        tool5_action.is_some(),
    )?;

    Ok(ShortcutConfig {
        select_area,
        start,
        restore_last_mode,
        toolbar1_action: tool1_action.map(|action| action.token().to_string()).unwrap_or_default(),
        toolbar1_key,
        toolbar2_action: tool2_action.map(|action| action.token().to_string()).unwrap_or_default(),
        toolbar2_key,
        toolbar3_action: tool3_action.map(|action| action.token().to_string()).unwrap_or_default(),
        toolbar3_key,
        toolbar4_action: tool4_action.map(|action| action.token().to_string()).unwrap_or_default(),
        toolbar4_key,
        toolbar5_action: tool5_action.map(|action| action.token().to_string()).unwrap_or_default(),
        toolbar5_key,
    })
}

/// Validates one shortcut row. When `required` is false an empty row is allowed and stored empty.
fn slot_binding(
    win: bool,
    alt: bool,
    ctrl: bool,
    shift: bool,
    key_index: i32,
    label: &str,
    required: bool,
) -> Result<String, String> {
    if !(win || alt || ctrl || shift) {
        if required {
            return Err(format!("{label}: select at least one of Win / Alt / Ctrl / Shift"));
        }
        return Ok(String::new());
    }

    let key = if key_index >= 0 {
        KEY_TOKENS.get(key_index as usize).copied().unwrap_or("")
    } else {
        ""
    };
    if key.is_empty() {
        return Err(format!("{label}: select a key"));
    }

    Ok(build_binding(win, alt, ctrl, shift, key))
}

#[cfg(target_os = "windows")]
fn configure_shortcut_window_native(window: &ShortcutSettingsWindow, dark_theme: bool) {
    use i_slint_backend_winit::WinitWindowAccessor;

    let _ = window.window().with_winit_window(|winit_window| {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
        crate::win_utils::set_title_bar_theme(hwnd, dark_theme);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use std::rc::Rc;

    #[test]
    fn shortcut_form_roundtrips_active_and_cleared_shortcuts() {
        struct Headless;
        impl slint::platform::Platform for Headless {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(Headless)).unwrap();
        let main = MainWindow::new().unwrap();
        let window = ShortcutSettingsWindow::new().unwrap();
        register_callbacks(&main, &window, None, Arc::new(Mutex::new(HotkeyState::default())));
        for config in [
            ShortcutConfig::default(),
            ShortcutConfig {
                restore_last_mode: "alt+ctrl+R".into(),
                toolbar1_action: "fullscreen".into(),
                toolbar1_key: "ctrl+shift+F1".into(),
                toolbar2_action: "ocr_translate".into(),
                toolbar2_key: "ctrl+shift+F2".into(),
                toolbar3_action: "toggle_textbox".into(),
                toolbar3_key: "ctrl+shift+F3".into(),
                toolbar4_action: "toggle_overlay".into(),
                toolbar4_key: "ctrl+shift+F4".into(),
                toolbar5_action: "change_mode".into(),
                toolbar5_key: "ctrl+shift+F5".into(),
                ..ShortcutConfig::default()
            },
            ShortcutConfig::default(),
            ShortcutConfig {
                select_area: String::new(),
                start: String::new(),
                ..ShortcutConfig::default()
            },
        ] {
            populate_window(&window, &config);
            assert_eq!(read_config(&window).unwrap(), config);
        }
    }
}
