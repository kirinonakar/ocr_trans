//! Keep drag coordinates mapped even when a native control captures the mouse.
use super::InputBridge;
use anyhow::{Context, Result};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Weak,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_F12,
        },
        WindowsAndMessaging::*,
    },
};

thread_local! {
    static OWNER: RefCell<Weak<crate::magnifier::WindowMagnifier>> = RefCell::new(Weak::new());
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
    static CLOSE_REQUESTED: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<VecDeque<(u32, MSLLHOOKSTRUCT)>> = const { RefCell::new(VecDeque::new()) };
}
const HOTKEY_ID: i32 = 0x4f43;
pub(super) fn is_button_release(message: u32) -> bool {
    matches!(
        message,
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP
    )
}
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
pub(super) fn dragging(active: bool) {
    DRAGGING.set(active);
}
pub(in crate::magnifier) fn take_close_requested() -> bool {
    CLOSE_REQUESTED.replace(false)
}
pub(super) fn take_pending() -> VecDeque<(u32, MSLLHOOKSTRUCT)> {
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
        && DRAGGING.get()
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
        match with_input(|input| input.global_mouse(message, data)) {
            Some(true) if !is_button_release(message) => return LRESULT(1),
            None => {
                // Queue re-entrant events instead of passing unscaled coordinates.
                PENDING.with(|slot| {
                    let mut pending = slot.borrow_mut();
                    if message == WM_MOUSEMOVE
                        && pending
                            .back()
                            .is_some_and(|(last, _)| *last == WM_MOUSEMOVE)
                    {
                        pending.pop_back();
                    }
                    pending.push_back((message, data));
                });
                if !is_button_release(message) {
                    return LRESULT(1);
                }
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
            // Only retain the hook while dragging, so a long capture/save cannot
            // stall unrelated desktop mouse input on the UI thread.
            UnhookWindowsHookEx(this.mouse.unwrap())?;
            this.mouse = None;
        }
        Ok(this)
    }

    pub fn set_dragging(&mut self, active: bool) -> Result<()> {
        unsafe {
            if active && self.mouse.is_none() {
                self.mouse = Some(SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0)?);
            }
            dragging(active);
            if !active {
                if let Some(hook) = self.mouse.take() {
                    let _ = UnhookWindowsHookEx(hook);
                }
            }
        }
        Ok(())
    }
}
impl Drop for NativeHooks {
    fn drop(&mut self) {
        DRAGGING.set(false);
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
