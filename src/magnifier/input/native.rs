//! Keep the OS pointer over the real source, and display a separate pointer in
//! the preview. Posting WM_MOUSEMOVE cannot update GetCursorPos, hit testing,
//! TrackMouseEvent, DirectInput or the native button state.
use super::*;
use windows::Win32::UI::{
    Input::KeyboardAndMouse::*,
    Magnification::{MagInitialize, MagShowSystemCursor, MagUninitialize},
};

pub(super) const INPUT_TAG: usize = 0x4f43524d;

fn advance_virtual(cursor: &mut (f64, f64), point: POINT, origin: POINT) {
    cursor.0 += (i64::from(point.x) - i64::from(origin.x)) as f64;
    cursor.1 += (i64::from(point.y) - i64::from(origin.y)) as f64;
}

fn update_virtual_sample(
    cursor: &mut (f64, f64),
    event: hooks::HardwareEvent,
    viewer_origin: POINT,
) {
    if !event.native {
        *cursor = viewer_local_point(event.data.pt, viewer_origin);
    } else if event.message == WM_MOUSEMOVE {
        advance_virtual(cursor, event.data.pt, event.origin);
    }
}

pub(super) struct NativePointer {
    active: bool,
    added_style: i32,
    held: u32,
}

// Use pixel centers, not endpoints: SendInput divides the virtual desktop into
// 65536 equal bins. This also works for monitors left/above the primary screen.
fn absolute_axis(value: i32, origin: i32, extent: i32) -> i32 {
    let extent = i64::from(extent.max(1));
    let offset = (i64::from(value) - i64::from(origin)).clamp(0, extent - 1);
    ((offset * 65536 + 32768) / extent).clamp(0, 65535) as i32
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, data: u32, point: Option<POINT>, desktop: RECT) -> INPUT {
    let mut mi = MOUSEINPUT {
        dwFlags: flags,
        mouseData: data,
        dwExtraInfo: INPUT_TAG,
        ..Default::default()
    };
    if let Some(point) = point {
        mi.dx = absolute_axis(point.x, desktop.left, desktop.right - desktop.left);
        mi.dy = absolute_axis(point.y, desktop.top, desktop.bottom - desktop.top);
        mi.dwFlags |= MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    }
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi },
    }
}

struct InputBatch {
    events: [INPUT; 2],
    len: usize,
}

impl InputBatch {
    fn from_hardware(
        screen: POINT,
        hook_origin: POINT,
        desktop: RECT,
        current_cursor: impl FnOnce() -> Option<POINT>,
    ) -> Self {
        // The hook origin belongs to the hardware delta calculation, not to
        // injection deduplication. A game may have warped the cursor since that
        // sample, or queued events may already have injected another position.
        Self::new(screen, current_cursor().unwrap_or(hook_origin), desktop)
    }

    fn new(screen: POINT, actual: POINT, desktop: RECT) -> Self {
        let mut batch = Self {
            events: [INPUT::default(); 2],
            len: 0,
        };
        if (screen.x, screen.y) != (actual.x, actual.y) {
            batch.push(mouse_input(MOUSE_EVENT_FLAGS(0), 0, Some(screen), desktop));
        }
        batch
    }
    fn push(&mut self, input: INPUT) {
        self.events[self.len] = input;
        self.len += 1;
    }
    fn as_slice(&self) -> &[INPUT] {
        &self.events[..self.len]
    }
    fn with_time(mut self, time: u32) -> Self {
        for event in &mut self.events[..self.len] {
            // Preserve the hardware clock across deferred/re-entrant dispatch.
            // A zero timestamp would replace the user's double-click interval
            // with our processing interval. Cursor + button share one timestamp.
            event.Anonymous.mi.time = time;
        }
        self
    }
}

fn inject(events: &[INPUT]) -> bool {
    unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) == events.len() as u32 }
}

fn button_event(message: u32, data: u32) -> Option<(u32, bool, MOUSE_EVENT_FLAGS, u32)> {
    Some(match message {
        WM_LBUTTONDOWN => (1, true, MOUSEEVENTF_LEFTDOWN, 0),
        WM_LBUTTONUP => (1, false, MOUSEEVENTF_LEFTUP, 0),
        WM_RBUTTONDOWN => (2, true, MOUSEEVENTF_RIGHTDOWN, 0),
        WM_RBUTTONUP => (2, false, MOUSEEVENTF_RIGHTUP, 0),
        WM_MBUTTONDOWN => (16, true, MOUSEEVENTF_MIDDLEDOWN, 0),
        WM_MBUTTONUP => (16, false, MOUSEEVENTF_MIDDLEUP, 0),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let side = data >> 16;
            (
                if side == 1 { 32 } else { 64 },
                message == WM_XBUTTONDOWN,
                if message == WM_XBUTTONDOWN {
                    MOUSEEVENTF_XDOWN
                } else {
                    MOUSEEVENTF_XUP
                },
                side,
            )
        }
        _ => return None,
    })
}

fn entry_blocked_by_buttons(pressed: u32, message: u32, data: u32) -> bool {
    // Dispatch runs after the physical callback, so the first press itself can
    // already be visible to GetAsyncKeyState. Only OTHER held buttons block it.
    let initiating = button_event(message, data)
        .filter(|(_, down, _, _)| *down)
        .map_or(0, |(mask, _, _, _)| mask);
    pressed & !initiating != 0
}

fn wheel_event(message: u32, data: u32) -> Option<(MOUSE_EVENT_FLAGS, u32)> {
    let flags = match message {
        WM_MOUSEWHEEL => MOUSEEVENTF_WHEEL,
        WM_MOUSEHWHEEL => MOUSEEVENTF_HWHEEL,
        _ => return None,
    };
    // Preserve high-resolution deltas and their sign, without scaling by zoom.
    Some((flags, ((data >> 16) as i16 as i32) as u32))
}

impl NativePointer {
    pub fn new() -> Result<Self> {
        anyhow::ensure!(
            unsafe { MagInitialize() }.as_bool(),
            "Unable to initialize the preview cursor"
        );
        Ok(Self {
            active: false,
            added_style: 0,
            held: 0,
        })
    }

    pub fn active(&self) -> bool {
        self.active
    }

    fn enter(&mut self, bridge: &mut InputBridge) -> Result<()> {
        // A layered transparent window passes input across process/thread
        // boundaries. HTTRANSPARENT alone only searches the same UI thread.
        unsafe {
            let style = GetWindowLongW(bridge.viewer, GWL_EXSTYLE);
            self.added_style = (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as i32 & !style;
            SetWindowLongW(bridge.viewer, GWL_EXSTYLE, style | self.added_style);
            let passthrough = (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as i32;
            if GetWindowLongW(bridge.viewer, GWL_EXSTYLE) & passthrough != passthrough {
                self.restore_style(bridge.viewer);
                anyhow::bail!("Unable to enable mouse interaction through the preview");
            }
            // Match Winit's set_cursor_hittest(false) path. Setting layered
            // opacity here would replace the viewer's composition rendering.
            if !MagShowSystemCursor(false).as_bool() {
                self.restore_style(bridge.viewer);
                anyhow::bail!("Unable to hide the source cursor");
            }
            let _ = ClipCursor(None);
        }
        self.active = true;
        hooks::native_active(true);
        bridge.activate();
        Ok(())
    }

    fn restore_style(&mut self, viewer: HWND) {
        unsafe {
            let style = GetWindowLongW(viewer, GWL_EXSTYLE);
            SetWindowLongW(viewer, GWL_EXSTYLE, style & !self.added_style);
        }
        self.added_style = 0;
    }

    pub fn leave(&mut self, bridge: &InputBridge, restore_position: bool) {
        if !self.active {
            return;
        }
        // Always release synthetic buttons, including on Alt+Tab or close.
        let releases: Vec<_> = [
            WM_LBUTTONUP,
            WM_RBUTTONUP,
            WM_MBUTTONUP,
            WM_XBUTTONUP,
            WM_XBUTTONUP,
        ]
        .into_iter()
        .zip([0, 0, 0, 1 << 16, 2 << 16])
        .filter_map(|(message, data)| button_event(message, data))
        .filter(|(mask, _, _, _)| self.held & mask != 0)
        .map(|(_, _, flags, data)| mouse_input(flags, data, None, RECT::default()))
        .collect();
        if !releases.is_empty() {
            inject(&releases);
            // ReleaseCapture affects only our own thread. Also cancel the real
            // recipient's drag when focus loss changes native input routing.
            unsafe {
                let thread = GetWindowThreadProcessId(bridge.source.window, None);
                let mut info = GUITHREADINFO {
                    cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                    ..Default::default()
                };
                if GetGUIThreadInfo(thread, &mut info).is_ok() && !info.hwndCapture.is_invalid() {
                    let _ =
                        PostMessageW(Some(info.hwndCapture), WM_CANCELMODE, WPARAM(0), LPARAM(0));
                }
                let _ = PostMessageW(
                    Some(bridge.source.window),
                    WM_CANCELMODE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        self.held = 0;
        self.active = false;
        hooks::clear_pending();
        hooks::native_active(false);
        self.restore_style(bridge.viewer);
        unsafe {
            let _ = MagShowSystemCursor(true);
        }
        if restore_position {
            let mut origin = POINT::default();
            let _dpi = TargetDpi::physical();
            if unsafe { ClientToScreen(bridge.viewer, &mut origin) }.as_bool() {
                inject(&[mouse_input(
                    MOUSE_EVENT_FLAGS(0),
                    0,
                    Some(POINT {
                        x: origin.x.saturating_add(bridge.cursor.0.round() as i32),
                        y: origin.y.saturating_add(bridge.cursor.1.round() as i32),
                    }),
                    desktop_bounds(),
                )]);
            }
        }
    }

    pub fn process(&mut self, bridge: &mut InputBridge, event: hooks::HardwareEvent) -> bool {
        let hooks::HardwareEvent {
            message,
            data,
            origin,
            ..
        } = event;
        let _dpi = TargetDpi::physical();
        let wheel = matches!(message, WM_MOUSEWHEEL | WM_MOUSEHWHEEL);
        let foreground = unsafe { GetForegroundWindow() };
        let related = foreground == bridge.viewer
            || foreground == bridge.source.window
            || unsafe { GetAncestor(foreground, GA_ROOTOWNER) } == bridge.source.window;
        if !self.active && !related && message == WM_MOUSEMOVE {
            return false;
        }
        if !self.active
            && message != WM_MOUSEMOVE
            && !button_event(message, data.mouseData).is_some_and(|(_, down, _, _)| down)
        {
            return false;
        }
        if (message != WM_MOUSEMOVE && !bridge.source.valid()) || (self.active && !related) {
            let was_active = self.active;
            self.leave(bridge, true);
            return was_active;
        }
        if !self.active {
            if message != WM_MOUSEMOVE {
                bridge.refresh_geometry();
            }
            // A cheap cached bounds rejection keeps unrelated desktop motion
            // out of WindowFromPoint and the source's message queue.
            let Some(geometry) = bridge.geometry else {
                return false;
            };
            let cursor = viewer_local_point(data.pt, geometry.viewer_origin);
            bridge.cursor = cursor;
            if bridge.source_point(false).is_none() {
                return false;
            }
            if unsafe { WindowFromPoint(data.pt) } != bridge.viewer {
                return false;
            }
            let pressed = unsafe {
                [
                    (VK_LBUTTON, 1),
                    (VK_RBUTTON, 2),
                    (VK_MBUTTON, 16),
                    (VK_XBUTTON1, 32),
                    (VK_XBUTTON2, 64),
                ]
                .into_iter()
                .fold(0, |mask, (key, bit)| {
                    mask | if GetAsyncKeyState(key.0 as i32) < 0 {
                        bit
                    } else {
                        0
                    }
                })
            };
            if entry_blocked_by_buttons(pressed, message, data.mouseData) {
                return false;
            }
            if let Err(error) = self.enter(bridge) {
                log::warn!("Unable to start native magnifier input: {error:#}");
                bridge.error = Some(format!("Window interaction unavailable: {error}"));
                return false;
            }
            bridge.refresh_geometry();
        } else {
            if let Some(geometry) = bridge.geometry {
                // Samples queued before entry are absolute viewer positions,
                // including a first press after an earlier hover activated us.
                update_virtual_sample(&mut bridge.cursor, event, geometry.viewer_origin);
            }
            if message != WM_MOUSEMOVE {
                bridge.refresh_geometry();
                // Overlay order is maintained by UI/window events. Wheel bursts
                // must not walk/reorder the entire topmost window band.
                if !wheel {
                    bridge.set_overlay_rects(crate::win_utils::set_preview_topmost(bridge.viewer));
                }
            }
        }
        // The system cursor is over the source, but overlay controls are at the
        // displayed pointer position. Hand input back when that pointer reaches
        // one of our visible overlays so its close button/textbox stays usable.
        if let Some(geometry) = bridge.geometry {
            let virtual_screen = POINT {
                x: geometry
                    .viewer_origin
                    .x
                    .saturating_add(bridge.cursor.0.round() as i32),
                y: geometry
                    .viewer_origin
                    .y
                    .saturating_add(bridge.cursor.1.round() as i32),
            };
            if bridge
                .overlay_rects
                .iter()
                .any(|rect| contains(*rect, virtual_screen))
            {
                let hit = unsafe { GetAncestor(WindowFromPoint(virtual_screen), GA_ROOT) };
                let mut pid = 0;
                unsafe {
                    GetWindowThreadProcessId(hit, Some(&mut pid));
                }
                if hit != bridge.viewer
                    && pid == std::process::id()
                    && unsafe { GetWindowLongW(hit, GWL_EXSTYLE) } & WS_EX_TOPMOST.0 as i32 != 0
                {
                    self.leave(bridge, true);
                    return true;
                }
            }
        }
        let Some(screen) = bridge.source_point(self.held != 0) else {
            // Reaching the header or leaving the image restores normal UI input.
            self.leave(bridge, true);
            return true;
        };
        // Never send a native press to an exposed caption/outside the source.
        // Owned popup windows (menus/tooltips) retain Windows' own hit testing.
        if message != WM_MOUSEMOVE && self.held == 0 && !bridge.source_client_contains(screen) {
            self.leave(bridge, true);
            return true;
        }
        if message != WM_MOUSEMOVE && self.held == 0 {
            let hit = unsafe { WindowFromPoint(screen) };
            if unsafe { GetAncestor(hit, GA_ROOT) } != bridge.source.window
                && unsafe { GetAncestor(hit, GA_ROOTOWNER) } != bridge.source.window
            {
                self.leave(bridge, true);
                return true;
            }
        }
        let Some(geometry) = bridge.geometry else {
            self.leave(bridge, true);
            return true;
        };
        // Suppress every handled hardware event and inject mapped input once.
        // Mixing original hardware clicks with synthesized movement breaks the
        // source-pointer contract used by games with native cursor tracking.
        let mut events = InputBatch::from_hardware(screen, origin, geometry.desktop, || {
            let mut current = POINT::default();
            unsafe { GetCursorPos(&mut current) }.ok().map(|_| current)
        });
        if let Some((mask, down, flags, mouse_data)) = button_event(message, data.mouseData) {
            events.push(mouse_input(flags, mouse_data, None, geometry.desktop));
            if inject(events.with_time(data.time).as_slice()) {
                if down {
                    self.held |= mask;
                } else {
                    self.held &= !mask;
                }
            } else {
                log::warn!("Windows rejected magnifier input (the target may be elevated)");
                bridge.error = Some("Windows blocked mouse input to the selected window".into());
                self.leave(bridge, true);
            }
        } else {
            if let Some((flags, delta)) = wheel_event(message, data.mouseData) {
                events.push(mouse_input(flags, delta, None, geometry.desktop));
            }
            if events.len != 0 && !inject(events.with_time(data.time).as_slice()) {
                bridge.error = Some("Windows blocked mouse input to the selected window".into());
                self.leave(bridge, true);
            }
        }
        true
    }

    pub fn maintain(&mut self, bridge: &InputBridge) {
        let _dpi = TargetDpi::physical();
        let Some(screen) = bridge.source_point(self.held != 0) else {
            self.leave(bridge, true);
            return;
        };
        let mut actual = POINT::default();
        // Source movement and game cursor warps must not leave the actual
        // cursor at a different point from the stationary preview pointer.
        if unsafe { GetCursorPos(&mut actual) }.is_ok()
            && (actual.x, actual.y) != (screen.x, screen.y)
        {
            let hit = unsafe { WindowFromPoint(screen) };
            if unsafe { GetAncestor(hit, GA_ROOT) } != bridge.source.window
                && unsafe { GetAncestor(hit, GA_ROOTOWNER) } != bridge.source.window
            {
                self.leave(bridge, true);
                return;
            }
            if !inject(&[mouse_input(
                MOUSE_EVENT_FLAGS(0),
                0,
                Some(screen),
                bridge.geometry.unwrap().desktop,
            )]) {
                self.leave(bridge, true);
            }
        }
    }
}

impl Drop for NativePointer {
    fn drop(&mut self) {
        unsafe {
            if self.active {
                let _ = MagShowSystemCursor(true);
            }
            let _ = MagUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_first_press_keeps_viewer_coordinates_after_hover_activates_native_mode() {
        let viewer = POINT { x: -1300, y: 300 };
        let mut cursor = (190.0, 140.0);
        let mut event = hooks::HardwareEvent {
            message: WM_LBUTTONDOWN,
            data: MSLLHOOKSTRUCT {
                pt: POINT { x: -1100, y: 450 },
                ..Default::default()
            },
            origin: POINT::default(),
            native: false,
            intercepted: true,
        };
        update_virtual_sample(&mut cursor, event, viewer);
        assert_eq!(cursor, (200.0, 150.0));
        event.native = true;
        event.message = WM_MOUSEMOVE;
        event.origin = POINT { x: -500, y: 400 };
        event.data.pt = POINT { x: -496, y: 402 };
        update_virtual_sample(&mut cursor, event, viewer);
        assert_eq!(cursor, (204.0, 152.0));
        event.message = WM_LBUTTONUP;
        event.data.pt = POINT { x: 800, y: 900 };
        update_virtual_sample(&mut cursor, event, viewer);
        assert_eq!(cursor, (204.0, 152.0));
    }

    #[test]
    fn deferred_first_press_ignores_its_own_button_but_preserves_drag_guard() {
        for (message, data, mask) in [
            (WM_LBUTTONDOWN, 0, 1),
            (WM_RBUTTONDOWN, 0, 2),
            (WM_MBUTTONDOWN, 0, 16),
            (WM_XBUTTONDOWN, 1 << 16, 32),
            (WM_XBUTTONDOWN, 2 << 16, 64),
        ] {
            assert!(!entry_blocked_by_buttons(mask, message, data));
            let other = if mask == 1 { 2 } else { 1 };
            assert!(entry_blocked_by_buttons(mask | other, message, data));
            assert!(entry_blocked_by_buttons(mask, WM_MOUSEMOVE, 0));
        }
    }

    #[test]
    fn game_cursor_warp_after_hook_sample_is_repaired_before_button_injection() {
        let desktop = RECT {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let target = POINT { x: -500, y: 400 };
        let game_center = POINT { x: -900, y: 500 };
        // The old hook sample equals the target, but the game moved the real
        // cursor before dispatch. Skipping the move would click at game_center.
        let mut batch = InputBatch::from_hardware(target, target, desktop, || Some(game_center));
        batch.push(mouse_input(MOUSEEVENTF_LEFTDOWN, 0, None, desktop));
        let batch = batch.with_time(1234);
        assert_eq!(batch.len, 2);
        let movement = unsafe { batch.as_slice()[0].Anonymous.mi };
        assert_ne!(movement.dwFlags & MOUSEEVENTF_MOVE, MOUSE_EVENT_FLAGS(0));
        assert_eq!(
            movement.dx,
            absolute_axis(target.x, desktop.left, desktop.right - desktop.left)
        );
        assert_eq!(
            movement.dy,
            absolute_axis(target.y, desktop.top, desktop.bottom - desktop.top)
        );
        assert_eq!(
            unsafe { batch.as_slice()[1].Anonymous.mi }.dwFlags,
            MOUSEEVENTF_LEFTDOWN
        );
        // Already aligned cursors still avoid extra movement during double-clicks.
        assert_eq!(
            InputBatch::from_hardware(target, game_center, desktop, || Some(target)).len,
            0
        );
        assert_eq!(
            InputBatch::from_hardware(target, target, desktop, || None).len,
            0
        );
    }

    #[test]
    fn queued_hardware_delta_uses_saved_origin_while_injection_uses_live_position() {
        let origin = POINT { x: -500, y: 400 };
        let data = POINT { x: -496, y: 402 };
        let warped = POINT { x: -900, y: 500 };
        let mut cursor = (200.5, 100.25);
        advance_virtual(&mut cursor, data, origin);
        assert_eq!(cursor, (204.5, 102.25));
        let desktop = RECT {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let batch = InputBatch::from_hardware(data, data, desktop, || Some(warped));
        assert_eq!(batch.len, 1);
        let movement = unsafe { batch.as_slice()[0].Anonymous.mi };
        assert_eq!(
            movement.dx,
            absolute_axis(data.x, desktop.left, desktop.right - desktop.left)
        );
        assert_eq!(
            movement.dy,
            absolute_axis(data.y, desktop.top, desktop.bottom - desktop.top)
        );
    }

    #[test]
    fn double_click_packets_preserve_hardware_times_without_extra_cursor_moves() {
        let desktop = RECT {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let point = POINT { x: -500, y: 400 };
        for times in [
            [1000, 1030, 1200, 1230],
            [u32::MAX - 100, u32::MAX - 70, 50, 80],
        ] {
            let mut forwarded = Vec::new();
            for (message, time) in [WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDOWN, WM_LBUTTONUP]
                .into_iter()
                .zip(times)
            {
                let (_, _, flags, data) = button_event(message, 0).unwrap();
                let mut batch = InputBatch::new(point, point, desktop);
                batch.push(mouse_input(flags, data, None, desktop));
                let batch = batch.with_time(time);
                assert_eq!(batch.len, 1);
                forwarded.push(unsafe { batch.as_slice()[0].Anonymous.mi }.time);
            }
            assert_eq!(forwarded, times);
            assert_eq!(
                forwarded[2].wrapping_sub(forwarded[0]),
                times[2].wrapping_sub(times[0])
            );
        }
        // A necessary reposition preceding a click must carry the same clock.
        let mut moved = InputBatch::new(point, POINT { x: 0, y: 0 }, desktop);
        moved.push(mouse_input(MOUSEEVENTF_LEFTDOWN, 0, None, desktop));
        let moved = moved.with_time(5678);
        assert_eq!(moved.len, 2);
        assert!(moved
            .as_slice()
            .iter()
            .all(|event| unsafe { event.Anonymous.mi }.time == 5678));
    }

    #[test]
    fn high_resolution_vertical_and_horizontal_wheel_keep_signed_delta_and_time() {
        for (message, flags) in [
            (WM_MOUSEWHEEL, MOUSEEVENTF_WHEEL),
            (WM_MOUSEHWHEEL, MOUSEEVENTF_HWHEEL),
        ] {
            for delta in [-240i16, -120, -15, 1, 15, 120, 240] {
                let (actual_flags, payload) =
                    wheel_event(message, (delta as u16 as u32) << 16).unwrap();
                let mut batch =
                    InputBatch::new(POINT::default(), POINT::default(), RECT::default());
                batch.push(mouse_input(actual_flags, payload, None, RECT::default()));
                let batch = batch.with_time(1234);
                assert_eq!(batch.len, 1);
                let packet = unsafe { batch.as_slice()[0].Anonymous.mi };
                assert_eq!(packet.dwFlags, flags);
                assert_eq!(packet.mouseData as i32, i32::from(delta));
                assert_eq!(packet.time, 1234);
            }
        }
    }

    #[test]
    fn stationary_clicks_and_wheel_are_injected_without_an_extra_move() {
        let desktop = RECT {
            left: -1920,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let point = POINT { x: -500, y: 400 };
        for (flags, data) in [
            (MOUSEEVENTF_LEFTDOWN, 0),
            (MOUSEEVENTF_LEFTUP, 0),
            (MOUSEEVENTF_WHEEL, (-120i32) as u32),
        ] {
            let mut batch = InputBatch::new(point, point, desktop);
            batch.push(mouse_input(flags, data, None, desktop));
            assert_eq!(batch.len, 1);
            let mi = unsafe { batch.as_slice()[0].Anonymous.mi };
            assert_eq!(mi.dwFlags, flags);
            assert_eq!(mi.mouseData, data);
        }
        let mut batch = InputBatch::new(point, POINT { x: 0, y: 0 }, desktop);
        batch.push(mouse_input(MOUSEEVENTF_LEFTDOWN, 0, None, desktop));
        assert_eq!(batch.len, 2);
        assert_ne!(
            unsafe { batch.as_slice()[0].Anonymous.mi }.dwFlags & MOUSEEVENTF_MOVE,
            MOUSE_EVENT_FLAGS(0)
        );
        assert_eq!(
            unsafe { batch.as_slice()[1].Anonymous.mi }.dwFlags,
            MOUSEEVENTF_LEFTDOWN
        );
    }

    #[test]
    fn fourfold_zoom_avoids_duplicate_native_pixel_moves() {
        let desktop = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let view = crate::capture::CaptureRect {
            x: 0,
            y: 0,
            width: 4000,
            height: 2000,
        };
        let source = RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 500,
        };
        let mut actual = POINT::default();
        let mut injected = 0;
        for x in 0..=1000 {
            let screen = map_point(view, source, (x as f64, 0.0), false).unwrap();
            let batch = InputBatch::new(screen, actual, desktop);
            if batch.len != 0 {
                injected += 1;
                actual = screen;
            }
        }
        assert_eq!(injected, 250);
        assert_eq!(actual.x, 250);
    }
    #[test]
    fn virtual_motion_uses_each_hardware_origin_even_when_native_moves_intervene() {
        let mut cursor = (200.5, 100.25);
        // Both events arrived while the UI was busy and the real cursor stayed
        // on the source. Replaying the first event injects a DIFFERENT position;
        // that must not distort the second event's original four-pixel delta.
        for _ in 0..2 {
            advance_virtual(
                &mut cursor,
                POINT { x: -1496, y: 502 },
                POINT { x: -1500, y: 500 },
            );
        }
        assert_eq!(cursor, (208.5, 104.25));
        advance_virtual(
            &mut cursor,
            POINT { x: -1598, y: 698 },
            POINT { x: -1600, y: 700 },
        );
        assert_eq!(cursor, (210.5, 102.25));
    }
    #[test]
    fn absolute_input_round_trips_every_pixel_on_negative_and_large_desktops() {
        for (origin, extent) in [
            (-3840, 7680),
            (-1080, 3240),
            (0, 1920),
            (200, 1),
            (-8000, 16000),
        ] {
            for offset in 0..extent {
                let encoded = absolute_axis(origin + offset, origin, extent);
                let decoded = (i64::from(encoded) * i64::from(extent) / 65536) as i32;
                assert_eq!(decoded, offset, "{origin} {extent} {offset}");
            }
        }
    }
    #[test]
    fn all_five_buttons_have_independent_native_press_and_release_pairs() {
        for (down, up, data) in [
            (WM_LBUTTONDOWN, WM_LBUTTONUP, 0),
            (WM_RBUTTONDOWN, WM_RBUTTONUP, 0),
            (WM_MBUTTONDOWN, WM_MBUTTONUP, 0),
            (WM_XBUTTONDOWN, WM_XBUTTONUP, 1 << 16),
            (WM_XBUTTONDOWN, WM_XBUTTONUP, 2 << 16),
        ] {
            let (mask, pressed, _, payload) = button_event(down, data).unwrap();
            let (up_mask, up_pressed, _, up_payload) = button_event(up, data).unwrap();
            assert!(pressed && !up_pressed);
            assert_eq!((mask, payload), (up_mask, up_payload));
        }
    }
}
