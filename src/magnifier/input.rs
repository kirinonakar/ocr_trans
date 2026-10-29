//! Native input for the live window preview. The physical cursor stays on the
//! source so Windows hit testing, hover tracking and game cursor queries agree.
use super::source_window::SourceWindow;
use anyhow::Result;
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::ClientToScreen,
    },
    UI::{
        HiDpi::{
            GetWindowDpiAwarenessContext, PhysicalToLogicalPointForPerMonitorDPI,
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
        WindowsAndMessaging::*,
    },
};
mod hooks;
mod native;
pub(super) use hooks::{set_owner, take_close_requested};

pub(super) struct InputBridge {
    native: Option<native::NativePointer>,
    hooks: Option<hooks::NativeHooks>,
    source: SourceWindow,
    viewer: HWND,
    no_activate_was_set: bool,
    view: Option<crate::capture::CaptureRect>,
    frame_size: (u32, u32),
    cursor: (f64, f64),
    error: Option<String>,
    geometry: Option<InputGeometry>,
    overlay_rects: Vec<RECT>,
}

#[derive(Clone, Copy)]
struct InputGeometry {
    source: RECT,
    viewer_origin: POINT,
    desktop: RECT,
}

fn contains(rect: RECT, point: POINT) -> bool {
    point.x >= rect.left && point.y >= rect.top && point.x < rect.right && point.y < rect.bottom
}

fn desktop_bounds() -> RECT {
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        RECT {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

struct TargetDpi(DPI_AWARENESS_CONTEXT);
impl TargetDpi {
    fn enter(window: HWND) -> Self {
        unsafe {
            Self(SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(
                window,
            )))
        }
    }

    fn physical() -> Self {
        unsafe {
            Self(SetThreadDpiAwarenessContext(
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            ))
        }
    }
}

impl Drop for TargetDpi {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
}

fn captured_bounds(size: (u32, u32), candidates: [RECT; 3]) -> Option<RECT> {
    candidates.into_iter().find(|rect| {
        rect.right > rect.left
            && rect.bottom > rect.top
            && (rect.right - rect.left) as u32 == size.0
            && (rect.bottom - rect.top) as u32 == size.1
    })
}

pub(super) fn capture_bounds(window: HWND, size: (u32, u32)) -> Option<RECT> {
    let _dpi = TargetDpi::physical();
    let mut visible = RECT::default();
    let mut outer = RECT::default();
    let mut client = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        let _ = DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut visible as *mut _ as _,
            std::mem::size_of::<RECT>() as u32,
        );
        GetWindowRect(window, &mut outer).ok()?;
        GetClientRect(window, &mut client).ok()?;
        if !ClientToScreen(window, &mut origin).as_bool() {
            return None;
        }
    }
    client.left += origin.x;
    client.right += origin.x;
    client.top += origin.y;
    client.bottom += origin.y;
    captured_bounds(size, [visible, outer, client])
}

fn viewer_local_point(screen: POINT, origin: POINT) -> (f64, f64) {
    (
        (i64::from(screen.x) - i64::from(origin.x)) as f64,
        (i64::from(screen.y) - i64::from(origin.y)) as f64,
    )
}

fn map_point(
    view: crate::capture::CaptureRect,
    source: RECT,
    point: (f64, f64),
    dragging: bool,
) -> Option<POINT> {
    if view.width <= 1
        || view.height <= 1
        || source.right <= source.left
        || source.bottom <= source.top
    {
        return None;
    }
    let (x, y) = (point.0 - view.x as f64, point.1 - view.y as f64);
    if !dragging && (x < 0.0 || y < 0.0 || x >= view.width as f64 || y >= view.height as f64) {
        return None;
    }
    let x = x.clamp(0.0, (view.width - 1) as f64) / (view.width - 1) as f64;
    let y = y.clamp(0.0, (view.height - 1) as f64) / (view.height - 1) as f64;
    Some(POINT {
        x: source.left + (x * (source.right - source.left - 1) as f64).round() as i32,
        y: source.top + (y * (source.bottom - source.top - 1) as f64).round() as i32,
    })
}

impl InputBridge {
    pub fn new(source: HWND, viewer: HWND) -> Result<Self> {
        let native = native::NativePointer::new()?;
        let hooks = hooks::NativeHooks::install(viewer)?;
        let source = SourceWindow::park(source, viewer)?;
        let style = unsafe { GetWindowLongW(viewer, GWL_EXSTYLE) };
        unsafe {
            SetWindowLongW(viewer, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as i32);
        }
        let mut this = Self {
            native: Some(native),
            hooks: Some(hooks),
            source,
            viewer,
            no_activate_was_set: style & WS_EX_NOACTIVATE.0 as i32 != 0,
            view: None,
            frame_size: (0, 0),
            cursor: (-1.0, -1.0),
            error: None,
            geometry: None,
            overlay_rects: Vec::new(),
        };
        this.activate();
        Ok(this)
    }

    pub fn set_view(&mut self, view: crate::capture::CaptureRect, frame_size: (u32, u32)) {
        let changed = self.frame_size != frame_size
            || self.view.is_some_and(|old| {
                (old.x, old.y, old.width, old.height) != (view.x, view.y, view.width, view.height)
            });
        if changed {
            self.suspend_mouse();
        }
        self.view = Some(view);
        self.frame_size = frame_size;
        if changed {
            self.refresh_geometry();
        }
    }

    pub fn source_covered(&self) -> bool {
        self.source.covered()
    }
    pub fn take_capture_restart(&mut self) -> bool {
        self.source.take_capture_restart()
    }

    fn source_point(&self, dragging: bool) -> Option<POINT> {
        map_point(self.view?, self.geometry?.source, self.cursor, dragging)
    }

    fn refresh_geometry(&mut self) {
        let _dpi = TargetDpi::physical();
        self.geometry = (|| {
            let source = capture_bounds(self.source.window, self.frame_size)?;
            let mut viewer_origin = POINT::default();
            if !unsafe { ClientToScreen(self.viewer, &mut viewer_origin) }.as_bool() {
                return None;
            }
            Some(InputGeometry {
                source,
                viewer_origin,
                desktop: desktop_bounds(),
            })
        })();
    }

    // Native Windows input selects child controls itself. Only button/wheel
    // events need this live client check; traversing every child on every move
    // adds cross-process work without affecting native routing.
    fn source_client_contains(&self, mut point: POINT) -> bool {
        let window = self.source.window;
        let mut origin = POINT::default();
        unsafe {
            let _dpi = TargetDpi::physical();
            if !ClientToScreen(window, &mut origin).as_bool()
                || !PhysicalToLogicalPointForPerMonitorDPI(Some(window), &mut point).as_bool()
                || !PhysicalToLogicalPointForPerMonitorDPI(Some(window), &mut origin).as_bool()
            {
                return false;
            }
            point.x -= origin.x;
            point.y -= origin.y;
            let _target_dpi = TargetDpi::enter(window);
            let mut client = RECT::default();
            GetClientRect(window, &mut client).is_ok() && contains(client, point)
        }
    }

    pub fn set_overlay_rects(&mut self, rects: Vec<RECT>) {
        self.overlay_rects = rects;
    }

    fn native_mouse(&mut self, event: hooks::HardwareEvent) -> bool {
        let Some(mut native) = self.native.take() else {
            return false;
        };
        let handled = native.process(self, event);
        self.native = Some(native);
        handled
    }

    pub fn preview_cursor(&self) -> Option<(f64, f64)> {
        self.native
            .as_ref()
            .filter(|native| native.active())
            .map(|_| self.cursor)
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    fn activate(&mut self) {
        unsafe {
            if GetForegroundWindow() != self.source.window {
                let _ = SetForegroundWindow(self.source.window);
            }
        }
        self.overlay_rects = crate::win_utils::set_preview_topmost(self.viewer);
    }

    pub fn flush(&mut self) {
        self.refresh_geometry();
        if let Some(mut native) = self.native.take() {
            let foreground = unsafe { GetForegroundWindow() };
            let related = foreground == self.viewer
                || foreground == self.source.window
                || unsafe { GetAncestor(foreground, GA_ROOTOWNER) } == self.source.window;
            if !self.source.valid() || !related {
                native.leave(self, true);
            }
            if native.active() && related {
                unsafe {
                    let mut clip = RECT::default();
                    if let Some(geometry) = self.geometry {
                        if GetClipCursor(&mut clip).is_ok()
                            && (clip.left, clip.top, clip.right, clip.bottom)
                                != (
                                    geometry.desktop.left,
                                    geometry.desktop.top,
                                    geometry.desktop.right,
                                    geometry.desktop.bottom,
                                )
                        {
                            let _ = ClipCursor(None);
                        }
                    }
                }
                native.maintain(self);
            }
            if !native.active() {
                self.source.keep_parked();
            }
            self.native = Some(native);
        }
        for event in hooks::take_pending() {
            self.native_mouse(event);
            if self.preview_cursor().is_none() {
                break;
            }
        }
    }

    pub fn suspend_mouse(&mut self) {
        hooks::take_pending();
        if let Some(mut native) = self.native.take() {
            let foreground = unsafe { GetForegroundWindow() };
            native.leave(
                self,
                foreground == self.viewer || foreground == self.source.window,
            );
            self.native = Some(native);
        }
        if self.source.valid() && unsafe { GetForegroundWindow() } == self.source.window {
            unsafe {
                let _ = ClipCursor(None);
            }
        }
    }
}

impl Drop for InputBridge {
    fn drop(&mut self) {
        self.suspend_mouse();
        self.hooks.take();
        unsafe {
            if !self.no_activate_was_set {
                let style = GetWindowLongW(self.viewer, GWL_EXSTYLE);
                SetWindowLongW(
                    self.viewer,
                    GWL_EXSTYLE,
                    style & !(WS_EX_NOACTIVATE.0 as i32),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_mapping_preserves_title_and_invisible_border_offsets_at_high_dpi() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let outer = RECT {
                left: -1600,
                top: 100,
                right: -1600 + (816.0 * dpi) as i32,
                bottom: 100 + (640.0 * dpi) as i32,
            };
            let visible = RECT {
                left: outer.left + (8.0 * dpi) as i32,
                top: outer.top,
                right: outer.right - (8.0 * dpi) as i32,
                bottom: outer.bottom - (8.0 * dpi) as i32,
            };
            let client = RECT {
                left: visible.left,
                top: outer.top + (32.0 * dpi) as i32,
                right: visible.right,
                bottom: visible.bottom,
            };
            for captured in [outer, visible, client] {
                let size = (
                    (captured.right - captured.left) as u32,
                    (captured.bottom - captured.top) as u32,
                );
                let selected = captured_bounds(size, [visible, outer, client]).unwrap();
                assert_eq!((selected.left, selected.top), (captured.left, captured.top));
                let view = crate::capture::CaptureRect {
                    x: 6,
                    y: 42,
                    width: size.0 as i32,
                    height: size.1 as i32,
                };
                // The first client pixel stays (0,0); the caption isn't scaled
                // into the client or subtracted a second time.
                let point = map_point(
                    view,
                    selected,
                    (
                        (6 + client.left - captured.left) as f64,
                        (42 + client.top - captured.top) as f64,
                    ),
                    false,
                )
                .unwrap();
                assert_eq!((point.x - client.left, point.y - client.top), (0, 0));
            }
            assert!(captured_bounds((13, 17), [visible, outer, client]).is_none());
        }
    }

    #[test]
    fn cursor_mapping_tracks_moved_viewer_on_negative_monitors() {
        let screen = POINT { x: -1100, y: 450 };
        let view = crate::capture::CaptureRect {
            x: 6,
            y: 42,
            width: 400,
            height: 300,
        };
        let source = RECT {
            left: 100,
            top: 200,
            right: 900,
            bottom: 800,
        };
        let before = viewer_local_point(screen, POINT { x: -1300, y: 300 });
        let after = viewer_local_point(screen, POINT { x: -1250, y: 280 });
        assert_eq!(before, (200.0, 150.0));
        assert_eq!(after, (150.0, 170.0));
        let first = map_point(view, source, before, false).unwrap();
        let moved = map_point(view, source, after, false).unwrap();
        assert!(moved.x < first.x);
        assert!(moved.y > first.y);
    }

    #[test]
    fn mouse_mapping_keeps_first_last_and_center_pixels_at_fractional_scales() {
        let source = RECT {
            left: -2000,
            top: 100,
            right: -800,
            bottom: 700,
        };
        for (width, height) in [(360, 180), (660, 330), (1860, 930)] {
            let view = crate::capture::CaptureRect {
                x: 6,
                y: 42,
                width,
                height,
            };
            let first = map_point(view, source, (6.0, 42.0), false).unwrap();
            assert_eq!((first.x, first.y), (-2000, 100));
            let last = map_point(
                view,
                source,
                ((6 + width - 1) as f64, (42 + height - 1) as f64),
                false,
            )
            .unwrap();
            assert_eq!((last.x, last.y), (-801, 699));
            let center = map_point(
                view,
                source,
                (
                    6.0 + (width - 1) as f64 / 2.0,
                    42.0 + (height - 1) as f64 / 2.0,
                ),
                false,
            )
            .unwrap();
            assert_eq!((center.x, center.y), (-1400, 400));
            assert!(map_point(view, source, (5.0, 41.0), false).is_none());
            let clamped = map_point(view, source, (5.0, 41.0), true).unwrap();
            assert_eq!((clamped.x, clamped.y), (-2000, 100));
        }
    }
}
