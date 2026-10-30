//! Single-instance activation and notification-area integration. The hidden native window
//! lives on the Slint thread, whose Winit message loop also dispatches its Win32 messages.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: PCWSTR = w!("OCRTrans.Lifecycle.Broker.v1");
const OWNER_TITLE: PCWSTR = w!("OCRTrans.SingleInstance.v1");
const MUTEX_NAME: PCWSTR = w!("Local\\OCRTrans.SingleInstance.v1");
const ACTIVATE: u32 = WM_APP + 41;
const TRAY_EVENT: u32 = WM_APP + 42;
// shellapi.h defines NIN_KEYSELECT as NIN_SELECT | NINF_KEY (1).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Restore,
    Capture,
    Ocr,
    Exit,
    TrayUnavailable,
}

struct NativeState {
    actions: RefCell<VecDeque<Action>>,
    icon: HICON,
    tray_requested: Cell<bool>,
    tray_added: Cell<bool>,
    taskbar_created: u32,
}

thread_local! { static BROKER: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) }; }

fn icon_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: TRAY_EVENT,
        hIcon: icon,
        ..Default::default()
    };
    for (target, value) in data.szTip.iter_mut().zip("OCR Translator".encode_utf16()) {
        *target = value;
    }
    data
}

impl NativeState {
    fn ensure_icon(&self, hwnd: HWND) -> anyhow::Result<()> {
        if self.tray_added.get() {
            return Ok(());
        }
        let mut data = icon_data(hwnd, self.icon);
        unsafe {
            anyhow::ensure!(
                Shell_NotifyIconW(NIM_ADD, &data).as_bool(),
                "Unable to add the tray icon"
            );
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            if !Shell_NotifyIconW(NIM_SETVERSION, &data).as_bool() {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                anyhow::bail!("Unable to configure the tray icon");
            }
        }
        self.tray_added.set(true);
        Ok(())
    }
}

/// Remove the icon before the existing hard-exit path (which does not run destructors).
pub(crate) fn remove_tray_icon() {
    BROKER.with(|broker| {
        let hwnd = broker.get();
        if !hwnd.0.is_null() {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd, HICON::default()));
            }
        }
    });
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let state = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const NativeState).as_ref();
    if let Some(state) = state {
        let action = match message {
            ACTIVATE => Some(Action::Restore),
            TRAY_EVENT => match (lp.0 as u32) & 0xffff {
                WM_LBUTTONDBLCLK | NIN_KEYSELECT => Some(Action::Restore),
                WM_CONTEXTMENU => {
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    let labels = ["Capture".to_string(), "OCR".to_string(), "Exit".to_string()];
                    match crate::win_utils::choose_popup_item(hwnd, &labels, -1, cursor.x, cursor.y)
                    {
                        Ok(Some(0)) => Some(Action::Capture),
                        Ok(Some(1)) => Some(Action::Ocr),
                        Ok(Some(2)) => Some(Action::Exit),
                        _ => None,
                    }
                }
                _ => None,
            },
            id if state.taskbar_created != 0 && id == state.taskbar_created => {
                state.tray_added.set(false);
                if state.tray_requested.get() && state.ensure_icon(hwnd).is_err() {
                    Some(Action::TrayUnavailable)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(action) = action {
            state.actions.borrow_mut().push_back(action);
        }
    }
    if message == ACTIVATE || message == TRAY_EVENT {
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wp, lp)
}

struct InstanceLock(HANDLE);

impl InstanceLock {
    fn acquire() -> anyhow::Result<Option<Self>> {
        Self::acquire_named(MUTEX_NAME)
    }

    fn acquire_named(name: PCWSTR) -> anyhow::Result<Option<Self>> {
        unsafe {
            let handle = CreateMutexW(None, false, name)?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                Ok(None)
            } else {
                Ok(Some(Self(handle)))
            }
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) struct NativeHost {
    hwnd: HWND,
    state: Box<NativeState>,
    lock: RefCell<Option<InstanceLock>>,
}

impl NativeHost {
    /// None means the existing instance received activation; do not initialize another UI.
    pub(crate) fn start() -> anyhow::Result<Option<Self>> {
        let settings = crate::settings::load_app_settings();
        // Respect an enabled instance even when launched from a different working directory.
        let restore_existing = !crate::autostart::requested();
        if Self::activate_existing(restore_existing) {
            return Ok(None);
        }
        let lock = if settings.single_instance {
            match InstanceLock::acquire()? {
                Some(lock) => Some(lock),
                None => {
                    // The winning process may still be constructing its broker window.
                    for _ in 0..100 {
                        if Self::activate_existing(restore_existing) {
                            return Ok(None);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    anyhow::bail!("The existing OCR Translator instance is not responding");
                }
            }
        } else {
            None
        };
        let host = Self::create(lock)?;
        if settings.minimize_to_tray {
            // Keep startup usable if Explorer is unavailable. Closing will retry adding it.
            if let Err(error) = host.set_tray(true) {
                log::warn!("{error}");
            }
        }
        Ok(Some(host))
    }

    fn create(lock: Option<InstanceLock>) -> anyhow::Result<Self> {
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            // Registration is per process. Tests may create a second broker in the same process.
            RegisterClassW(&class);
            let icon = LoadImageW(
                Some(module.into()),
                PCWSTR(1usize as *const u16),
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTSIZE | LR_SHARED,
            )?;
            let state = Box::new(NativeState {
                actions: RefCell::new(VecDeque::new()),
                icon: HICON(icon.0),
                tray_requested: Cell::new(false),
                tray_added: Cell::new(false),
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            });
            // A hidden top-level window receives Explorer's TaskbarCreated broadcast.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS_NAME,
                if lock.is_some() { OWNER_TITLE } else { w!("") },
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module.into()),
                Some(state.as_ref() as *const NativeState as *const _),
            )?;
            BROKER.with(|broker| broker.set(hwnd));
            let host = Self {
                hwnd,
                state,
                lock: RefCell::new(lock),
            };
            Ok(host)
        }
    }

    pub(crate) fn start_in_tray(&self) -> bool {
        // If Explorer could not create the icon, keep the controls visible and recoverable.
        crate::autostart::requested() && self.state.tray_added.get()
    }

    fn activate_existing(restore: bool) -> bool {
        unsafe {
            let Ok(hwnd) = FindWindowW(CLASS_NAME, OWNER_TITLE) else {
                return false;
            };
            if !restore {
                return true;
            }
            let mut process = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut process));
            let _ = AllowSetForegroundWindow(process);
            PostMessageW(Some(hwnd), ACTIVATE, WPARAM(0), LPARAM(0)).is_ok()
        }
    }
    fn set_single_instance(&self, enabled: bool) -> anyhow::Result<()> {
        if enabled && self.lock.borrow().is_none() {
            let lock = InstanceLock::acquire()?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Another OCR Translator instance already has Single instance enabled"
                )
            })?;
            unsafe {
                SetWindowTextW(self.hwnd, OWNER_TITLE)?;
            }
            *self.lock.borrow_mut() = Some(lock);
        } else if !enabled {
            unsafe {
                SetWindowTextW(self.hwnd, w!(""))?;
            }
            self.lock.borrow_mut().take();
        }
        Ok(())
    }

    fn set_tray(&self, enabled: bool) -> anyhow::Result<()> {
        self.state.tray_requested.set(enabled);
        if enabled {
            self.state.ensure_icon(self.hwnd)?;
        } else {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(self.hwnd, self.state.icon));
            }
            self.state.tray_added.set(false);
        }
        Ok(())
    }

    fn take_actions(&self) -> Vec<Action> {
        self.state.actions.borrow_mut().drain(..).collect()
    }
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        let _ = self.set_tray(false);
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        BROKER.with(|broker| broker.set(HWND::default()));
    }
}

use crate::{
    CaptureFrameWindow, CaptureToolbarWindow, MainWindow, RecordingBorderWindow,
    ShortcutSettingsWindow,
};
use slint::ComponentHandle;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn focus_window(window: &slint::Window) {
    use i_slint_backend_winit::WinitWindowAccessor;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window.with_winit_window(|native| {
        native.set_minimized(false);
        native.focus_window();
        if let Ok(handle) = native.window_handle() {
            if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                unsafe {
                    let _ = SetForegroundWindow(HWND(handle.hwnd.get() as _));
                }
            }
        }
    });
}

pub(crate) fn register_callbacks(
    host: NativeHost,
    main: &MainWindow,
    toolbar: &CaptureToolbarWindow,
    frame: &CaptureFrameWindow,
    shortcuts: &ShortcutSettingsWindow,
    recorder: Arc<Mutex<Option<crate::capture::ScreenRecorder>>>,
    border: &RecordingBorderWindow,
) -> slint::Timer {
    let host = Rc::new(host);
    let settings = crate::settings::load_app_settings();
    main.set_single_instance(settings.single_instance);
    main.set_minimize_to_tray(settings.minimize_to_tray);

    let mut registered_autostart = match crate::autostart::is_enabled() {
        Ok(enabled) => enabled,
        Err(error) => {
            main.set_lifecycle_error(error.to_string().into());
            false
        }
    };
    if registered_autostart && !settings.minimize_to_tray {
        // Also reconcile a manually edited INI file with the prerequisite.
        match crate::autostart::set_enabled(false) {
            Ok(()) => registered_autostart = false,
            Err(error) => main.set_lifecycle_error(error.to_string().into()),
        }
    }
    main.set_autostart(registered_autostart && settings.minimize_to_tray);
    let saved_autostart = Cell::new(registered_autostart);

    let options_main = main.as_weak();
    let options_host = host.clone();
    main.on_lifecycle_options_changed(move || {
        let Some(main) = options_main.upgrade() else {
            return;
        };
        let previous = crate::settings::load_app_settings();
        let single = main.get_single_instance();
        let tray = single && main.get_minimize_to_tray();
        let previous_autostart = saved_autostart.get();
        let autostart = tray && main.get_autostart();
        let result = options_host
            .set_single_instance(single)
            .and_then(|()| options_host.set_tray(tray))
            .and_then(|()| {
                if autostart != previous_autostart {
                    crate::autostart::set_enabled(autostart)
                } else {
                    Ok(())
                }
            });
        match result {
            Ok(()) => {
                main.set_minimize_to_tray(tray);
                main.set_autostart(autostart);
                saved_autostart.set(autostart);
                main.set_lifecycle_error("".into());
                crate::settings::save_lifecycle_options(single, tray);
                // Another launch must see the new setting immediately.
                crate::settings::flush_settings();
            }
            Err(error) => {
                let _ = options_host.set_single_instance(previous.single_instance);
                let _ = options_host.set_tray(previous.minimize_to_tray);
                main.set_single_instance(previous.single_instance);
                main.set_minimize_to_tray(previous.minimize_to_tray);
                main.set_autostart(previous_autostart && previous.minimize_to_tray);
                main.set_lifecycle_error(error.to_string().into());
            }
        }
    });

    let close_main = main.as_weak();
    let close_toolbar = toolbar.as_weak();
    let close_frame = frame.as_weak();
    let close_shortcuts = shortcuts.as_weak();
    let close_host = host.clone();
    let close_recorder = recorder.clone();
    let close_border = border.as_weak();
    let close: Rc<dyn Fn(&str)> = Rc::new(move |mode| {
        let settings = crate::settings::load_app_settings();
        if !settings.single_instance || !settings.minimize_to_tray {
            crate::capture_workflow::shutdown_and_exit(&close_recorder, &close_border);
        }
        // Never hide the controls unless the notification icon is actually available.
        if let Err(error) = close_host.set_tray(true) {
            log::warn!("Cannot hide to tray: {error}");
            if let Some(main) = close_main.upgrade() {
                main.set_lifecycle_error(error.to_string().into());
                main.invoke_mode_toggle_clicked("ocr".into());
            }
            return;
        }
        crate::settings::save_app_mode(mode);
        let (main, toolbar, frame, shortcuts) = (
            close_main.clone(),
            close_toolbar.clone(),
            close_frame.clone(),
            close_shortcuts.clone(),
        );
        // Do not change native windows while Winit is dispatching the close/button event.
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            if let Some(main) = main.upgrade() {
                let _ = main.hide();
            }
            if let Some(toolbar) = toolbar.upgrade() {
                let _ = toolbar.hide();
            }
            if let Some(frame) = frame.upgrade() {
                let _ = frame.hide();
            }
            if let Some(shortcuts) = shortcuts.upgrade() {
                let _ = shortcuts.hide();
            }
        });
    });
    let main_close = close.clone();
    main.window().on_close_requested(move || {
        main_close("ocr");
        slint::CloseRequestResponse::KeepWindowShown
    });
    let button_close = close.clone();
    toolbar.on_close_clicked(move || button_close("capture"));
    toolbar.window().on_close_requested(move || {
        close("capture");
        slint::CloseRequestResponse::KeepWindowShown
    });

    let main = main.as_weak();
    let toolbar = toolbar.as_weak();
    let frame = frame.as_weak();
    let border = border.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(50),
        move || {
            for action in host.take_actions() {
                if action == Action::Exit {
                    crate::capture_workflow::shutdown_and_exit(&recorder, &border);
                }
                let Some(main) = main.upgrade() else {
                    continue;
                };
                let mode = match action {
                    Action::Capture => "capture".to_string(),
                    Action::Ocr | Action::TrayUnavailable => "ocr".to_string(),
                    _ => crate::settings::load_app_settings().app_mode,
                };
                if action == Action::TrayUnavailable {
                    main.set_lifecycle_error(
                        "Unable to restore the tray icon. Close the window to retry.".into(),
                    );
                }
                if mode != "capture" {
                    if let Some(frame) = frame.upgrade() {
                        let _ = frame.hide();
                    }
                }
                main.invoke_mode_toggle_clicked(mode.clone().into());
                let main = main.as_weak();
                let toolbar = toolbar.clone();
                let frame = frame.clone();
                slint::Timer::single_shot(Duration::from_millis(20), move || {
                    if mode == "capture" {
                        if let Some(toolbar) = toolbar.upgrade() {
                            if toolbar.get_frame_mode() && !toolbar.get_recording() {
                                if let Some(frame) = frame.upgrade() {
                                    let _ = frame.show();
                                    crate::capture_workflow::configure_capture_frame_native_window(
                                        &frame,
                                    );
                                }
                            }
                            focus_window(toolbar.window());
                        }
                    } else if let Some(main) = main.upgrade() {
                        focus_window(main.window());
                    }
                });
            }
        },
    );
    timer
}

#[cfg(test)]
mod tests {
    use super::*;

    // Child-process entry point: exercise Windows' cross-process named object lifetime.
    #[test]
    fn named_instance_lock_probe() {
        let Ok(name) = std::env::var("OCR_TRANS_TEST_MUTEX") else {
            return;
        };
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let lock = InstanceLock::acquire_named(PCWSTR(name.as_ptr())).unwrap();
        let expected = std::env::var("OCR_TRANS_TEST_MUTEX_AVAILABLE").unwrap() == "true";
        assert_eq!(lock.is_some(), expected);
    }

    #[test]
    fn single_instance_lock_excludes_other_processes_and_releases_on_drop() {
        let name = format!("Local\\OCRTrans.Test.{}", uuid::Uuid::new_v4());
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let lock = InstanceLock::acquire_named(PCWSTR(wide.as_ptr()))
            .unwrap()
            .unwrap();
        let probe = |available: bool| {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "lifecycle::tests::named_instance_lock_probe",
                    "--nocapture",
                ])
                .env("OCR_TRANS_TEST_MUTEX", &name)
                .env("OCR_TRANS_TEST_MUTEX_AVAILABLE", available.to_string())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        probe(false);
        drop(lock);
        probe(true);
    }

    #[test]
    fn native_activation_and_tray_double_click_queue_restore_until_ui_is_ready() {
        let host = NativeHost::create(None).unwrap();
        unsafe {
            SendMessageW(host.hwnd, ACTIVATE, Some(WPARAM(0)), Some(LPARAM(0)));
            // Version 4 notifications encode the icon ID in the upper word.
            SendMessageW(
                host.hwnd,
                TRAY_EVENT,
                Some(WPARAM(0)),
                Some(LPARAM(((1 << 16) | WM_LBUTTONDBLCLK) as isize)),
            );
            SendMessageW(
                host.hwnd,
                TRAY_EVENT,
                Some(WPARAM(0)),
                Some(LPARAM(((1 << 16) | WM_LBUTTONUP) as isize)),
            );
        }
        assert_eq!(host.take_actions(), [Action::Restore, Action::Restore]);
        assert!(host.take_actions().is_empty());
        assert!(!host.state.tray_added.get());
    }
}
