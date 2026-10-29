//! Keep the complete top control strip in the monitor work area,
//! including during Windows' modal move/resize loop (where Slint timers can pause).
use anyhow::Result;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    UI::WindowsAndMessaging::{
        WM_DISPLAYCHANGE, WM_DPICHANGED, WM_MOVING, WM_SETTINGCHANGE, WM_SIZING,
        WM_WINDOWPOSCHANGED,
    },
};

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
const SUBCLASS_ID: usize = 0x4f43524d;

pub(super) struct MovementGuard(pub(super) HWND);
impl MovementGuard {
    pub(super) fn new(window: HWND) -> Result<Self> {
        anyhow::ensure!(
            unsafe { SetWindowSubclass(window, Some(constrain_movement), SUBCLASS_ID, 0) } != 0,
            "Unable to protect the magnifier controls from moving off screen"
        );
        Ok(Self(window))
    }
}
impl Drop for MovementGuard {
    fn drop(&mut self) {
        unsafe {
            RemoveWindowSubclass(self.0, Some(constrain_movement), SUBCLASS_ID);
        }
    }
}

pub(super) fn constrain_rect(rect: RECT, work: RECT) -> RECT {
    let width = (rect.right - rect.left).min(work.right - work.left).max(1);
    let height = (rect.bottom - rect.top).min(work.bottom - work.top).max(1);
    let left = rect.left.clamp(work.left, work.right - width);
    let top = rect.top.clamp(work.top, work.bottom - height);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

unsafe extern "system" fn constrain_movement(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if matches!(
        message,
        WM_WINDOWPOSCHANGED | WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE
    ) {
        super::input::geometry_changed();
    }
    if (message == WM_MOVING || message == WM_SIZING) && lparam.0 != 0 {
        let (x, y) = crate::capture::cursor_position();
        // Follow the pointer's monitor so the viewer can cross display boundaries.
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let proposed = &mut *(lparam.0 as *mut RECT);
            *proposed = constrain_rect(*proposed, info.rcWork);
            return LRESULT(1);
        }
    }
    DefSubclassProc(window, message, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_stays_visible_with_negative_monitor_origin_and_top_taskbar() {
        let work = RECT {
            left: -1920,
            top: 48,
            right: 0,
            bottom: 1080,
        };
        for rect in [
            RECT {
                left: -2100,
                top: -300,
                right: -1300,
                bottom: 300,
            },
            RECT {
                left: -400,
                top: 900,
                right: 400,
                bottom: 1500,
            },
            RECT {
                left: -3000,
                top: -1000,
                right: 3000,
                bottom: 2000,
            },
        ] {
            let bounded = constrain_rect(rect, work);
            assert!(bounded.left >= work.left && bounded.top >= work.top);
            assert!(bounded.right <= work.right && bounded.bottom <= work.bottom);
            assert!(bounded.right > bounded.left && bounded.bottom > bounded.top);
        }
    }
}
