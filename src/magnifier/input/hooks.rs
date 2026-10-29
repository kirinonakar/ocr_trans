//! Map hardware input once and allow our tagged native events through unchanged.
use anyhow::{Context, Result};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Weak,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        Input::KeyboardAndMouse::{
            RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_F12,
        },
        WindowsAndMessaging::*,
    },
};

thread_local! {
    static OWNER: RefCell<Weak<crate::magnifier::WindowMagnifier>> = RefCell::new(Weak::new());
    static UPDATE_QUEUED: Cell<bool> = const { Cell::new(false) };
    static CURSOR_QUEUED: Cell<bool> = const { Cell::new(false) };
    static GEOMETRY_VERSION: Cell<u64> = const { Cell::new(1) };
    static SOURCE: Cell<isize> = const { Cell::new(0) };
    static CLOSE_REQUESTED: Cell<bool> = const { Cell::new(false) };
}
#[derive(Clone, Copy)]
pub(super) struct HardwareEvent {
    pub message: u32,
    pub data: MSLLHOOKSTRUCT,
    pub origin: POINT,
    // True for source-relative samples; false for absolute viewer hover/entry.
    pub native: bool,
    pub intercepted: bool,
}

fn queue_pending(queue: &mut VecDeque<HardwareEvent>, event: HardwareEvent) {
    if event.message == WM_MOUSEMOVE {
        if let Some(last) = queue
            .back_mut()
            .filter(|last| last.message == WM_MOUSEMOVE && last.native == event.native)
        {
            if !event.native {
                *last = event;
                return;
            }
            // Positions were sampled against different native cursor origins.
            // Sum relative deltas, rather than keeping only the last position.
            let sum = |first: i32, next: i32, next_origin: i32| {
                (i64::from(first) + i64::from(next) - i64::from(next_origin))
                    .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
            };
            last.data.pt.x = sum(last.data.pt.x, event.data.pt.x, event.origin.x);
            last.data.pt.y = sum(last.data.pt.y, event.data.pt.y, event.origin.y);
            last.data.time = event.data.time;
            return;
        }
    }
    queue.push_back(event);
}

mod mouse_thread;
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
pub(in crate::magnifier) fn geometry_version() -> u64 {
    GEOMETRY_VERSION.get()
}
pub(in crate::magnifier) fn geometry_changed() {
    GEOMETRY_VERSION.set(GEOMETRY_VERSION.get().wrapping_add(1));
    request_update();
}
pub(in crate::magnifier) fn request_update() {
    if UPDATE_QUEUED.replace(true) {
        return;
    }
    if slint::invoke_from_event_loop(|| {
        UPDATE_QUEUED.set(false);
        OWNER.with(|slot| {
            if let Some(owner) = slot.borrow().upgrade() {
                owner.update_or_close();
            }
        });
    })
    .is_err()
    {
        UPDATE_QUEUED.set(false);
    }
}
pub(in crate::magnifier) fn present_frame() {
    OWNER.with(|slot| {
        if let Some(owner) = slot.borrow().upgrade() {
            owner.present_frame();
        }
    });
}
fn dispatch_cursor() -> bool {
    OWNER.with(|slot| {
        let Ok(slot) = slot.try_borrow() else {
            return false;
        };
        slot.upgrade().is_none_or(|owner| owner.update_cursor())
    })
}
fn request_cursor_update() {
    if CURSOR_QUEUED.replace(true) {
        return;
    }
    if slint::invoke_from_event_loop(|| {
        CURSOR_QUEUED.set(false);
        if !dispatch_cursor() {
            slint::Timer::single_shot(std::time::Duration::from_millis(1), request_cursor_update);
        }
    })
    .is_err()
    {
        CURSOR_QUEUED.set(false);
    }
}
unsafe extern "system" fn source_location_changed(
    _: HWINEVENTHOOK,
    _: u32,
    window: HWND,
    object: i32,
    child: i32,
    _: u32,
    _: u32,
) {
    // Ignore caret/child-control motion. Out-of-context WinEvents are delivered
    // on the installing UI thread; never query the remote HWND in this callback.
    if window.0 as isize == SOURCE.get() && object == OBJID_WINDOW.0 && child == 0 {
        geometry_changed();
    }
}
pub(super) fn native_active(active: bool) {
    mouse_thread::set_active(active);
}
pub(super) fn set_preview(preview: Option<RECT>) {
    mouse_thread::set_preview(preview);
}
pub(in crate::magnifier) fn take_close_requested() -> bool {
    CLOSE_REQUESTED.replace(false)
}
pub(super) fn take_pending() -> VecDeque<HardwareEvent> {
    mouse_thread::take_pending()
}
pub(super) fn clear_pending() {
    mouse_thread::clear_pending();
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if message == mouse_thread::DISPATCH {
        if mouse_thread::matches_generation(wparam.0) && !dispatch_cursor() {
            request_cursor_update();
        }
        return LRESULT(0);
    }
    if message == WM_HOTKEY && wparam.0 == HOTKEY_ID as usize {
        CLOSE_REQUESTED.set(true);
        request_update();
        return LRESULT(0);
    }
    DefSubclassProc(window, message, wparam, lparam)
}

pub(super) struct NativeHooks {
    viewer: HWND,
    mouse: Option<mouse_thread::MouseThread>,
    location: HWINEVENTHOOK,
    subclassed: bool,
    hotkey: bool,
}
impl NativeHooks {
    pub fn install(source: HWND, viewer: HWND) -> Result<Self> {
        let mut this = Self {
            viewer,
            mouse: None,
            location: HWINEVENTHOOK::default(),
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
            this.mouse = Some(mouse_thread::MouseThread::start(viewer)?);
            SOURCE.set(source.0 as isize);
            let mut pid = 0;
            GetWindowThreadProcessId(source, Some(&mut pid));
            this.location = SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(source_location_changed),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if this.location.is_invalid() {
                log::warn!("Source location hook unavailable; using maintenance refresh");
            }
            // Hover also needs native hit testing; keep the hook until close.
        }
        Ok(this)
    }
}
impl NativeHooks {
    pub fn mouse_failed(&self) -> bool {
        self.mouse.as_ref().is_some_and(|mouse| mouse.failed())
    }
    pub fn tracks_geometry(&self) -> bool {
        !self.location.is_invalid()
    }
}
impl Drop for NativeHooks {
    fn drop(&mut self) {
        native_active(false);
        SOURCE.set(0);
        CLOSE_REQUESTED.set(false);
        clear_pending();
        unsafe {
            if !self.location.is_invalid() {
                let _ = UnhookWinEvent(self.location);
            }
            self.mouse.take();
            if self.hotkey {
                let _ = UnregisterHotKey(Some(self.viewer), HOTKEY_ID);
            }
            if self.subclassed {
                RemoveWindowSubclass(self.viewer, Some(window_proc), SUBCLASS_ID);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(message: u32, origin_x: i32, dx: i32, dy: i32) -> HardwareEvent {
        HardwareEvent {
            message,
            native: true,
            intercepted: true,
            origin: POINT {
                x: origin_x,
                y: 300,
            },
            data: MSLLHOOKSTRUCT {
                pt: POINT {
                    x: origin_x + dx,
                    y: 300 + dy,
                },
                ..Default::default()
            },
        }
    }

    #[test]
    fn inactive_hover_uses_latest_absolute_position_without_merging_relative_motion() {
        let mut queue = VecDeque::new();
        let mut first = event(WM_MOUSEMOVE, 100, 0, 0);
        first.native = false;
        first.intercepted = false;
        let mut last = event(WM_MOUSEMOVE, 120, 0, 0);
        last.native = false;
        last.intercepted = false;
        queue_pending(&mut queue, first);
        queue_pending(&mut queue, last);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].data.pt.x, 120);
        queue_pending(&mut queue, event(WM_MOUSEMOVE, -1000, 4, 2));
        assert_eq!(queue.len(), 2);
        assert!(!queue[0].native);
        assert!(queue[1].native);
    }

    #[test]
    fn pending_motion_combines_deltas_without_crossing_click_or_wheel_boundaries() {
        let mut queue = VecDeque::new();
        for input in [
            event(WM_MOUSEMOVE, -1000, 2, 3),
            event(WM_MOUSEMOVE, -900, 4, 1),
            event(WM_LBUTTONDOWN, -850, 0, 0),
            event(WM_MOUSEMOVE, -850, -3, 2),
            event(WM_MOUSEMOVE, -860, 1, -1),
            event(WM_MOUSEWHEEL, -850, 0, 0),
            event(WM_LBUTTONUP, -850, 0, 0),
        ] {
            queue_pending(&mut queue, input);
        }
        assert_eq!(
            queue.iter().map(|event| event.message).collect::<Vec<_>>(),
            [
                WM_MOUSEMOVE,
                WM_LBUTTONDOWN,
                WM_MOUSEMOVE,
                WM_MOUSEWHEEL,
                WM_LBUTTONUP
            ]
        );
        let first = queue[0];
        assert_eq!(
            (
                first.data.pt.x - first.origin.x,
                first.data.pt.y - first.origin.y
            ),
            (6, 4)
        );
        let second = queue[2];
        assert_eq!(
            (
                second.data.pt.x - second.origin.x,
                second.data.pt.y - second.origin.y
            ),
            (-2, 1)
        );
    }
}
