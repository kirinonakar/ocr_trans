//! Native input for the live window preview. The physical cursor stays on the
//! source so Windows hit testing, hover tracking and game cursor queries agree.
use super::source_window::SourceWindow;
use anyhow::Result;
use std::time::{Duration, Instant};
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
pub(super) use hooks::{
    geometry_changed, geometry_version, present_frame, request_update, set_owner,
    take_close_requested,
};

pub(super) struct InputBridge {
    native: Option<native::NativePointer>,
    hooks: Option<hooks::NativeHooks>,
    source: SourceWindow,
    viewer: HWND,
    no_activate_was_set: bool,
    view: Option<crate::capture::CaptureRect>,
    frame_size: (u32, u32),
    frame_crop: Option<(u32, u32, u32, u32)>,
    cursor: (f64, f64),
    error: Option<String>,
    geometry: Option<InputGeometry>,
    geometry_version: u64,
    overlay_rects: Vec<RECT>,
}

#[derive(Clone, Copy)]
struct InputGeometry {
    source: RECT,
    viewer_origin: POINT,
    desktop: RECT,
    client: Option<ClientHit>,
}

#[derive(Clone, Copy)]
struct ClientHit {
    origin: POINT,
    rect: RECT,
    read_at: Instant,
}
impl ClientHit {
    fn contains(self, logical_point: POINT) -> bool {
        contains(
            self.rect,
            POINT {
                x: logical_point.x.saturating_sub(self.origin.x),
                y: logical_point.y.saturating_sub(self.origin.y),
            },
        )
    }
}

fn cached_client_hit(
    cached: Option<ClientHit>,
    now: Instant,
    convert: impl FnOnce() -> Option<POINT>,
    live: impl FnOnce() -> bool,
) -> bool {
    let cached = cached.filter(|client| {
        now.saturating_duration_since(client.read_at) <= Duration::from_millis(250)
    });
    match cached.and_then(|client| convert().map(|point| client.contains(point))) {
        Some(inside) => inside,
        None => live(),
    }
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

fn intersect_client(frame: RECT, client: RECT) -> Option<(u32, u32, u32, u32)> {
    let left = frame.left.max(client.left);
    let top = frame.top.max(client.top);
    let right = frame.right.min(client.right);
    let bottom = frame.bottom.min(client.bottom);
    (right > left && bottom > top).then_some((
        (left - frame.left) as u32,
        (top - frame.top) as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// Locate client pixels in the captured frame, using physical coordinates for
/// both WGC's visible-frame capture and DWM's full-window fallback.
pub(super) fn client_crop(window: HWND, size: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let _dpi = TargetDpi::physical();
    let frame = capture_bounds(window, size)?;
    let mut client = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetClientRect(window, &mut client).ok()?;
        if !ClientToScreen(window, &mut origin).as_bool() {
            return None;
        }
    }
    client.left += origin.x;
    client.right += origin.x;
    client.top += origin.y;
    client.bottom += origin.y;
    intersect_client(frame, client)
}

fn query_client_hit(window: HWND) -> Option<ClientHit> {
    // Cache the SAME logical coordinate system as the legacy per-click check.
    // No inverse/corner conversion; a failed optional probe must never disable
    // the independently valid source-to-view mapping (notably in games).
    let _dpi = TargetDpi::physical();
    let mut origin = POINT::default();
    let mut rect = RECT::default();
    unsafe {
        if !ClientToScreen(window, &mut origin).as_bool()
            || !PhysicalToLogicalPointForPerMonitorDPI(Some(window), &mut origin).as_bool()
        {
            return None;
        }
        let _target_dpi = TargetDpi::enter(window);
        GetClientRect(window, &mut rect).ok()?;
    }
    (rect.right > rect.left && rect.bottom > rect.top).then_some(ClientHit {
        origin,
        rect,
        read_at: Instant::now(),
    })
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
        let hooks = hooks::NativeHooks::install(source, viewer)?;
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
            frame_crop: None,
            cursor: (-1.0, -1.0),
            error: None,
            geometry: None,
            geometry_version: 0,
            overlay_rects: Vec::new(),
        };
        this.activate();
        Ok(this)
    }

    pub fn set_view(
        &mut self,
        view: crate::capture::CaptureRect,
        frame_size: (u32, u32),
        frame_crop: Option<(u32, u32, u32, u32)>,
    ) {
        let changed = self.frame_size != frame_size
            || self.frame_crop != frame_crop
            || self.view.is_some_and(|old| {
                (old.x, old.y, old.width, old.height) != (view.x, view.y, view.width, view.height)
            });
        if changed {
            self.suspend_mouse();
        }
        self.view = Some(view);
        self.frame_size = frame_size;
        self.frame_crop = frame_crop;
        if changed || self.geometry.is_none() {
            self.geometry_version = hooks::geometry_version().wrapping_sub(1);
            self.refresh_geometry();
        }
        self.publish_hook_preview();
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
        let version = hooks::geometry_version();
        if self.geometry_version == version {
            return;
        }
        self.geometry_version = version;
        let _dpi = TargetDpi::physical();
        self.geometry = (|| {
            let mut source = capture_bounds(self.source.window, self.frame_size)?;
            if let Some((x, y, width, height)) = self.frame_crop {
                source.left += x as i32;
                source.top += y as i32;
                source.right = source.left + width as i32;
                source.bottom = source.top + height as i32;
            }
            let mut viewer_origin = POINT::default();
            if !unsafe { ClientToScreen(self.viewer, &mut viewer_origin) }.as_bool() {
                return None;
            }
            Some(InputGeometry {
                source,
                viewer_origin,
                desktop: desktop_bounds(),
                client: query_client_hit(self.source.window),
            })
        })();
        self.publish_hook_preview();
    }

    fn publish_hook_preview(&self) {
        let preview = self.geometry.zip(self.view).map(|(geometry, view)| RECT {
            left: geometry.viewer_origin.x.saturating_add(view.x),
            top: geometry.viewer_origin.y.saturating_add(view.y),
            right: geometry
                .viewer_origin
                .x
                .saturating_add(view.x)
                .saturating_add(view.width),
            bottom: geometry
                .viewer_origin
                .y
                .saturating_add(view.y)
                .saturating_add(view.height),
        });
        hooks::set_preview(preview);
    }

    fn source_client_contains(&self, point: POINT) -> bool {
        let cached = self.geometry.and_then(|geometry| geometry.client);
        cached_client_hit(
            cached,
            Instant::now(),
            || {
                let mut logical = point;
                unsafe {
                    PhysicalToLogicalPointForPerMonitorDPI(Some(self.source.window), &mut logical)
                }
                .as_bool()
                .then_some(logical)
            },
            || self.source_client_contains_live(point),
        )
    }

    // Preserve the previously working DPI-aware check as a fallback when the
    // optional cache/probe is unavailable. Never gate all pointer geometry on it.
    fn source_client_contains_live(&self, mut point: POINT) -> bool {
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
        if event.intercepted && !native.active() {
            // A rejected first press must release the hook's pending-entry gate.
            hooks::native_active(false);
        }
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
        if self
            .hooks
            .as_ref()
            .is_some_and(|hooks| hooks.mouse_failed())
        {
            self.suspend_mouse();
            self.error = Some("Mouse hook stopped; close and reopen the magnifier to retry".into());
            return;
        }
        if self
            .hooks
            .as_ref()
            .is_some_and(|hooks| !hooks.tracks_geometry())
        {
            self.geometry_version = hooks::geometry_version().wrapping_sub(1);
        }
        self.refresh_geometry();
        if let Some(geometry) = self.geometry.as_mut() {
            if geometry
                .client
                .is_none_or(|client| client.read_at.elapsed() >= Duration::from_millis(100))
            {
                // Repair missed client-only layout/DPI notifications and failed
                // optional probes without repeating remote queries per click.
                geometry.client = query_client_hit(self.source.window);
            }
        }
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
        self.flush_pending();
    }

    pub fn flush_pending(&mut self) {
        for event in hooks::take_pending() {
            if event.native && self.preview_cursor().is_none() {
                // Source-relative events from a completed leave are no longer
                // valid viewer coordinates. Never re-enter using those points.
                continue;
            }
            self.native_mouse(event);
            // Inactive hover is observational: don't discard a subsequent first
            // press just because the preceding hover did not activate the source.
        }
    }

    pub fn suspend_mouse(&mut self) {
        hooks::clear_pending();
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
    fn hidden_titlebar_maps_client_corners_at_each_dpi_and_with_negative_coordinates() {
        for dpi in [1, 2, 3] {
            let frame = RECT {
                left: -1800,
                top: -100,
                right: -1800 + 816 * dpi,
                bottom: -100 + 640 * dpi,
            };
            let client = RECT {
                left: frame.left + 8 * dpi,
                top: frame.top + 32 * dpi,
                right: frame.right - 8 * dpi,
                bottom: frame.bottom - 8 * dpi,
            };
            let crop = intersect_client(frame, client).unwrap();
            assert_eq!(
                crop,
                (
                    (8 * dpi) as u32,
                    (32 * dpi) as u32,
                    (800 * dpi) as u32,
                    (600 * dpi) as u32
                )
            );
            let view = crate::capture::CaptureRect {
                x: 6,
                y: 42,
                width: 1600,
                height: 1200,
            };
            let first = map_point(view, client, (6.0, 42.0), false).unwrap();
            let last = map_point(view, client, (1605.0, 1241.0), false).unwrap();
            assert_eq!((first.x, first.y), (client.left, client.top));
            assert_eq!((last.x, last.y), (client.right - 1, client.bottom - 1));
            assert_eq!(
                intersect_client(client, client),
                Some((0, 0, (800 * dpi) as u32, (600 * dpi) as u32))
            );
        }
        assert!(intersect_client(
            RECT {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100
            },
            RECT {
                left: 0,
                top: 100,
                right: 100,
                bottom: 200
            }
        )
        .is_none());
    }

    #[test]
    fn logical_client_cache_keeps_caption_boundary_and_falls_back_on_probe_failure() {
        let now = Instant::now();
        let cached = ClientHit {
            origin: POINT { x: -1500, y: 200 },
            rect: RECT {
                left: 0,
                top: 0,
                right: 800,
                bottom: 600,
            },
            read_at: now,
        };
        for (point, inside) in [
            (POINT { x: -1500, y: 200 }, true),
            (POINT { x: -701, y: 799 }, true),
            (POINT { x: -1500, y: 199 }, false),
            (POINT { x: -700, y: 200 }, false),
        ] {
            assert_eq!(
                cached_client_hit(
                    Some(cached),
                    now,
                    || Some(point),
                    || panic!("live query on cache hit")
                ),
                inside
            );
        }
        assert!(cached_client_hit(
            None,
            now,
            || panic!("convert without cache"),
            || true
        ));
        assert!(cached_client_hit(Some(cached), now, || None, || true));
        assert!(!cached_client_hit(Some(cached), now, || None, || false));
        let stale = ClientHit {
            read_at: now - Duration::from_secs(1),
            ..cached
        };
        assert!(cached_client_hit(
            Some(stale),
            now,
            || panic!("stale coordinate conversion"),
            || true
        ));
    }

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
