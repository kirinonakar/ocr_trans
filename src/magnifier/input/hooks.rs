//! Map hardware input once and allow our tagged native events through unchanged.
use super::InputBridge;
use anyhow::{Context, Result};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Weak,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_F12,
        },
        WindowsAndMessaging::*,
    },
};

thread_local! {
    static OWNER: RefCell<Weak<crate::magnifier::WindowMagnifier>> = RefCell::new(Weak::new());
    static NATIVE_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static CLOSE_REQUESTED: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<VecDeque<HardwareEvent>> = const { RefCell::new(VecDeque::new()) };
}
#[derive(Clone, Copy)]
pub(super) struct HardwareEvent {
    pub message: u32,
    pub data: MSLLHOOKSTRUCT,
    pub origin: POINT,
}
const HOTKEY_ID: i32 = 0x4f43;
const SUBCLASS_ID: usize = 0x4f435249;
type SubclassProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;
#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(
        window: HWND,
        callback: Option<SubclassProc>,
        id: usize,
        data: usize,
    ) -> i32;
    fn RemoveWindowSubclass(window: HWND, callback: Option<SubclassProc>, id: usize) -> i32;
    fn DefSubclassProc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
}

pub(in crate::magnifier) fn set_owner(owner: Weak<crate::magnifier::WindowMagnifier>) {
    OWNER.with(|slot| *slot.borrow_mut() = owner);
}
pub(super) fn native_active(active: bool) {
    NATIVE_ACTIVE.set(active);
}
pub(in crate::magnifier) fn take_close_requested() -> bool {
    CLOSE_REQUESTED.replace(false)
}
pub(super) fn take_pending() -> VecDeque<HardwareEvent> {
    PENDING.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
}

fn with_input(action: impl FnOnce(&mut InputBridge) -> bool) -> Option<bool> {
    OWNER.with(|slot| {
        let owner = slot.try_borrow().ok()?.upgrade()?;
        let mut input = owner.input.try_borrow_mut().ok()?;
        input.as_mut().map(action)
    })
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let message = wparam.0 as u32;
    if code >= 0
        && lparam.0 != 0
        && matches!(
            message,
            WM_MOUSEMOVE
                | WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_RBUTTONDOWN
                | WM_RBUTTONUP
                | WM_MBUTTONDOWN
                | WM_MBUTTONUP
                | WM_XBUTTONDOWN
                | WM_XBUTTONUP
                | WM_MOUSEWHEEL
                | WM_MOUSEHWHEEL
        )
    {
        let data = *(lparam.0 as *const MSLLHOOKSTRUCT);
        // Our SendInput events must reach Windows exactly once, without being
        // remapped by this hook or entering the re-entrant pending queue.
        if data.dwExtraInfo == super::native::INPUT_TAG {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // Sample BEFORE Windows applies this hardware event. Store the origin
        // with queued events: intervening injected moves must not change their
        // deltas when a re-entrant event is replayed later.
        let _dpi = super::TargetDpi::physical();
        let mut origin = POINT::default();
        if GetCursorPos(&mut origin).is_err() {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let event = HardwareEvent {
            message,
            data,
            origin,
        };
        match with_input(|input| input.native_mouse(event)) {
            Some(true) => return LRESULT(1),
            None if NATIVE_ACTIVE.get() => {
                PENDING.with(|slot| slot.borrow_mut().push_back(event));
                return LRESULT(1);
            }
            _ => {}
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if message == WM_HOTKEY && wparam.0 == HOTKEY_ID as usize {
        CLOSE_REQUESTED.set(true);
        return LRESULT(0);
    }
    DefSubclassProc(window, message, wparam, lparam)
}

pub(super) struct NativeHooks {
    viewer: HWND,
    mouse: Option<HHOOK>,
    subclassed: bool,
    hotkey: bool,
}
impl NativeHooks {
    pub fn install(viewer: HWND) -> Result<Self> {
        let mut this = Self {
            viewer,
            mouse: None,
            subclassed: false,
            hotkey: false,
        };
        unsafe {
            anyhow::ensure!(
                SetWindowSubclass(viewer, Some(window_proc), SUBCLASS_ID, 0) != 0,
                "Unable to install source-window recovery shortcut"
            );
            this.subclassed = true;
            RegisterHotKey(
                Some(viewer),
                HOTKEY_ID,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                VK_F12.0 as u32,
            )
            .context("Unable to reserve Ctrl+Alt+F12 for restoring the source window")?;
            this.hotkey = true;
            this.mouse = Some(SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0)?);
            // Hover also needs native hit testing; keep the hook until close.
        }
        Ok(this)
    }
}
impl Drop for NativeHooks {
    fn drop(&mut self) {
        NATIVE_ACTIVE.set(false);
        CLOSE_REQUESTED.set(false);
        PENDING.with(|slot| slot.borrow_mut().clear());
        unsafe {
            if let Some(hook) = self.mouse {
                let _ = UnhookWindowsHookEx(hook);
            }
            if self.hotkey {
                let _ = UnregisterHotKey(Some(self.viewer), HOTKEY_ID);
            }
            if self.subclassed {
                RemoveWindowSubclass(self.viewer, Some(window_proc), SUBCLASS_ID);
            }
        }
    }
}
