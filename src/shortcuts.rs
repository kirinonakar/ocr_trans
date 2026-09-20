//! Global shortcut configuration and (re)registration.
//!
//! The OCR window exposes five configurable shortcuts:
//! * `select_area`  - opens the capture-area selector (default `win+alt+A`)
//! * `start`        - starts/stops the continuous OCR loop (default `win+alt+P`)
//! * `toolbar1`     - runs one capture-toolbar action
//! * `toolbar2`     - runs a second capture-toolbar action
//! * `toolbar3`     - runs a third capture-toolbar action
//!
//! Bindings are stored as `modifier+modifier+KEY` strings (for example `win+alt+A`) so they can
//! live in the plain-text settings file. The toolbar slots additionally carry an action token.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::GlobalHotKeyManager;
use std::sync::{Arc, Mutex};

/// Key tokens offered in the shortcut settings UI. The order is also the combo-box order and the
/// index that the Slint properties encode.
pub(crate) const KEY_TOKENS: [&str; 60] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "F1", "F2",
    "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "`", ",", ".", "/", ";", "'",
    "[", "]", "\\", "-", "=", "PrintScreen",
];

/// Persisted shortcut configuration. `Default` intentionally enables only the two original
/// shortcuts; the capture-toolbar slots start empty.
#[derive(Clone, Debug)]
pub(crate) struct ShortcutConfig {
    pub(crate) select_area: String,
    pub(crate) start: String,
    pub(crate) toolbar1_action: String,
    pub(crate) toolbar1_key: String,
    pub(crate) toolbar2_action: String,
    pub(crate) toolbar2_key: String,
    pub(crate) toolbar3_action: String,
    pub(crate) toolbar3_key: String,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            select_area: "win+alt+A".to_string(),
            start: "win+alt+P".to_string(),
            toolbar1_action: String::new(),
            toolbar1_key: String::new(),
            toolbar2_action: String::new(),
            toolbar2_key: String::new(),
            toolbar3_action: String::new(),
            toolbar3_key: String::new(),
        }
    }
}

/// Actions a capture-toolbar shortcut can trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolbarAction {
    Fullscreen,
    Window,
    Region,
    Ocr,
    OcrTranslate,
    Vlm,
    ColorPicker,
    Ruler,
}

pub(crate) const TOOLBAR_ACTIONS: [ToolbarAction; 8] = [
    ToolbarAction::Fullscreen,
    ToolbarAction::Window,
    ToolbarAction::Region,
    ToolbarAction::Ocr,
    ToolbarAction::OcrTranslate,
    ToolbarAction::Vlm,
    ToolbarAction::ColorPicker,
    ToolbarAction::Ruler,
];

impl ToolbarAction {
    pub(crate) fn token(self) -> &'static str {
        match self {
            ToolbarAction::Fullscreen => "fullscreen",
            ToolbarAction::Window => "window",
            ToolbarAction::Region => "region",
            ToolbarAction::Ocr => "ocr",
            ToolbarAction::OcrTranslate => "ocr_translate",
            ToolbarAction::Vlm => "vlm",
            ToolbarAction::ColorPicker => "color_picker",
            ToolbarAction::Ruler => "ruler",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ToolbarAction::Fullscreen => "Capture: Fullscreen",
            ToolbarAction::Window => "Capture: Window",
            ToolbarAction::Region => "Capture: Region",
            ToolbarAction::Ocr => "OCR",
            ToolbarAction::OcrTranslate => "OCR + Translate",
            ToolbarAction::Vlm => "VLM",
            ToolbarAction::ColorPicker => "Color picker",
            ToolbarAction::Ruler => "Ruler",
        }
    }

    pub(crate) fn from_token(token: &str) -> Option<Self> {
        let token = token.trim();
        TOOLBAR_ACTIONS
            .iter()
            .copied()
            .find(|action| action.token() == token)
    }

    /// Index inside the settings combo box, where 0 means "(not set)".
    pub(crate) fn option_index(self) -> i32 {
        TOOLBAR_ACTIONS
            .iter()
            .position(|action| *action == self)
            .map(|index| index as i32 + 1)
            .unwrap_or(0)
    }

    /// Inverse of [`ToolbarAction::option_index`].
    pub(crate) fn from_option_index(index: i32) -> Option<Self> {
        if index <= 0 {
            return None;
        }
        TOOLBAR_ACTIONS.get((index - 1) as usize).copied()
    }
}

/// Labels for the action combo box, with "(not set)" at index 0.
pub(crate) fn action_options() -> Vec<&'static str> {
    let mut options = vec!["(not set)"];
    options.extend(TOOLBAR_ACTIONS.iter().map(|action| action.label()));
    options
}

/// Hotkeys currently registered with the OS, keyed by slot.
#[derive(Clone, Copy, Default)]
pub(crate) struct RegisteredHotkeys {
    pub(crate) select_area: Option<HotKey>,
    pub(crate) start: Option<HotKey>,
    pub(crate) toolbar1: Option<HotKey>,
    pub(crate) toolbar2: Option<HotKey>,
    pub(crate) toolbar3: Option<HotKey>,
}

/// Shared, `Send` state describing the currently registered shortcuts. The OS manager itself is
/// not `Send` (it owns a native window handle), so it stays on the UI thread and is passed to
/// [`apply_config`] separately.
#[derive(Clone, Default)]
pub(crate) struct HotkeyState {
    pub(crate) config: ShortcutConfig,
    pub(crate) registered: RegisteredHotkeys,
}

/// Parses `win+alt+A` style bindings into a [`HotKey`]. Returns `None` for empty or unknown input.
pub(crate) fn parse_binding(binding: &str) -> Option<HotKey> {
    let binding = binding.trim();
    if binding.is_empty() {
        return None;
    }

    let mut modifiers = Modifiers::empty();
    let mut has_modifier = false;
    let mut code = None;

    for part in binding.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.to_ascii_lowercase().as_str() {
            "win" | "meta" | "super" => {
                modifiers |= Modifiers::META;
                has_modifier = true;
            }
            "alt" => {
                modifiers |= Modifiers::ALT;
                has_modifier = true;
            }
            "ctrl" | "control" => {
                modifiers |= Modifiers::CONTROL;
                has_modifier = true;
            }
            "shift" => {
                modifiers |= Modifiers::SHIFT;
                has_modifier = true;
            }
            _ => {
                // A second non-modifier token makes the binding ambiguous.
                if code.is_some() {
                    return None;
                }
                code = key_code_from_token(part);
                if code.is_none() {
                    return None;
                }
            }
        }
    }

    let code = code?;
    let modifiers = has_modifier.then_some(modifiers);
    Some(HotKey::new(modifiers, code))
}

/// Maps a UI key token (`A`, `5`, `F12`) to a keyboard `Code`.
pub(crate) fn key_code_from_token(token: &str) -> Option<Code> {
    let token = token.trim().to_ascii_uppercase();
    Some(match token.as_str() {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "`" => Code::Backquote,
        "," => Code::Comma,
        "." => Code::Period,
        "/" => Code::Slash,
        ";" => Code::Semicolon,
        "'" => Code::Quote,
        "[" => Code::BracketLeft,
        "]" => Code::BracketRight,
        "\\" => Code::Backslash,
        "-" => Code::Minus,
        "=" => Code::Equal,
        "PRINTSCREEN" => Code::PrintScreen,
        _ => return None,
    })
}

/// Splits a stored binding into its modifier flags and its key token.
pub(crate) fn split_binding(binding: &str) -> (bool, bool, bool, bool, String) {
    let mut win = false;
    let mut alt = false;
    let mut ctrl = false;
    let mut shift = false;
    let mut key = String::new();

    for part in binding.split('+') {
        let part = part.trim();
        match part.to_ascii_lowercase().as_str() {
            "win" | "meta" | "super" => win = true,
            "alt" => alt = true,
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "" => {}
            _ => key = part.to_ascii_uppercase(),
        }
    }

    (win, alt, ctrl, shift, key)
}

/// Builds a stored binding from the settings UI values.
pub(crate) fn build_binding(win: bool, alt: bool, ctrl: bool, shift: bool, key: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if win {
        parts.push("win".to_string());
    }
    if alt {
        parts.push("alt".to_string());
    }
    if ctrl {
        parts.push("ctrl".to_string());
    }
    if shift {
        parts.push("shift".to_string());
    }
    let key = key.trim();
    if !key.is_empty() {
        parts.push(key.to_ascii_uppercase());
    }
    parts.join("+")
}

/// Index of a key token inside [`KEY_TOKENS`], or -1 when unknown.
pub(crate) fn key_index(token: &str) -> i32 {
    // Compare without case so mixed-case tokens such as `PrintScreen` round-trip through the
    // stored binding (which is upper-cased by `build_binding`).
    let token = token.trim();
    KEY_TOKENS
        .iter()
        .position(|candidate| (*candidate).eq_ignore_ascii_case(token))
        .map(|index| index as i32)
        .unwrap_or(-1)
}

/// Human readable form used on the OCR window's buttons, for example `Win + Alt + A`.
pub(crate) fn binding_label(binding: &str) -> String {
    let binding = binding.trim();
    if binding.is_empty() {
        return "not set".to_string();
    }

    let mut parts: Vec<String> = Vec::new();
    for part in binding.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let label = match part.to_ascii_lowercase().as_str() {
            "win" | "meta" | "super" => "Win".to_string(),
            "alt" => "Alt".to_string(),
            "ctrl" | "control" => "Ctrl".to_string(),
            "shift" => "Shift".to_string(),
            other => other.to_ascii_uppercase(),
        };
        parts.push(label);
    }
    parts.join(" + ")
}

fn register_one(
    manager: &GlobalHotKeyManager,
    binding: &str,
    label: &str,
) -> Option<HotKey> {
    let hotkey = parse_binding(binding)?;
    if let Err(error) = manager.register(hotkey) {
        log::warn!("Failed to register {label} shortcut ({binding}): {error:?}");
        return None;
    }
    Some(hotkey)
}

/// Unregisters the previous bindings and registers the ones described by `config`.
///
/// A binding that fails to register (for example a duplicate combination) is logged and left
/// unbound; the remaining shortcuts still work.
pub(crate) fn apply_config(
    manager: Option<&GlobalHotKeyManager>,
    state: &Arc<Mutex<HotkeyState>>,
    config: ShortcutConfig,
) {
    let mut state = state.lock().unwrap();

    if let Some(manager) = manager {
        for hotkey in [
            state.registered.select_area,
            state.registered.start,
            state.registered.toolbar1,
            state.registered.toolbar2,
            state.registered.toolbar3,
        ]
        .into_iter()
        .flatten()
        {
            let _ = manager.unregister(hotkey);
        }

        let mut registered = RegisteredHotkeys::default();
        registered.select_area = register_one(manager, &config.select_area, "Select area");
        registered.start = register_one(manager, &config.start, "Start");
        if ToolbarAction::from_token(&config.toolbar1_action).is_some() {
            registered.toolbar1 = register_one(manager, &config.toolbar1_key, "Toolbar shortcut 1");
        }
        if ToolbarAction::from_token(&config.toolbar2_action).is_some() {
            registered.toolbar2 = register_one(manager, &config.toolbar2_key, "Toolbar shortcut 2");
        }
        if ToolbarAction::from_token(&config.toolbar3_action).is_some() {
            registered.toolbar3 = register_one(manager, &config.toolbar3_key, "Toolbar shortcut 3");
        }
        state.registered = registered;
    }

    state.config = config;
}
