//! Mouse messages use unscaled source client coordinates. Keyboard, IME and
//! physical gamepads keep their native route by keeping the source foreground.
use super::source_window::SourceWindow;
use anyhow::Result;
use std::{collections::VecDeque, time::Instant};
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::ClientToScreen,
    },
    UI::{
        Controls::{WM_MOUSEHOVER, WM_MOUSELEAVE},
        HiDpi::{
            GetWindowDpiAwarenessContext, PhysicalToLogicalPointForPerMonitorDPI,
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetDoubleClickTime, ReleaseCapture, SetCapture, VK_CONTROL, VK_SHIFT,
        },
        WindowsAndMessaging::*,
    },
};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
mod hooks;
mod hover;
pub(super) use hooks::{set_owner, take_close_requested};

enum PointerEvent<'a> {
    Moved(f64, f64),
    Button(&'a ElementState, &'a MouseButton),
    Wheel(&'a MouseScrollDelta),
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

#[derive(Clone, Copy)]
struct MouseMessage {
    window: HWND,
    message: u32,
    wparam: usize,
    point: POINT,
    activate: bool,
}

pub(super) struct InputBridge {
    hooks: Option<hooks::NativeHooks>,
    source: SourceWindow,
    viewer: HWND,
    no_activate_was_set: bool,
    view: Option<crate::capture::CaptureRect>,
    frame_size: (u32, u32),
    hover: hover::Hover,
    cursor: (f64, f64),
    buttons: u32,
    captured_child: Option<HWND>,
    queue: VecDeque<MouseMessage>,
    activate_on_flush: bool,
    last_click: Option<(MouseButton, HWND, POINT, Instant)>,
    skip_viewer_ups: u32,
    held_messages: Vec<MouseMessage>,
    input_targets: Vec<HWND>,
}

fn pack_point(point: POINT) -> LPARAM {
    LPARAM(((point.x as u16 as u32) | ((point.y as u16 as u32) << 16)) as isize)
}

// WGC includes different non-client extents depending on the window style.
// Match the actual frame instead of stretching it over an assumed DWM rect.
fn captured_bounds(size: (u32, u32), candidates: [RECT; 3]) -> Option<RECT> {
    candidates.into_iter().find(|rect| {
        rect.right > rect.left
            && rect.bottom > rect.top
            && (rect.right - rect.left) as u32 == size.0
            && (rect.bottom - rect.top) as u32 == size.1
    })
}

fn viewer_local_point(screen: POINT, origin: POINT) -> (f64, f64) {
    (
        (i64::from(screen.x) - i64::from(origin.x)) as f64,
        (i64::from(screen.y) - i64::from(origin.y)) as f64,
    )
}

/// Winit's image rectangle uses physical client pixels. Use the same units for
/// both current cursor queries and the physical points supplied by the hook.
fn viewer_point(viewer: HWND, screen: POINT) -> Option<(f64, f64)> {
    let _dpi = TargetDpi::physical();
    let mut origin = POINT::default();
    unsafe {
        ClientToScreen(viewer, &mut origin)
            .as_bool()
            .then(|| viewer_local_point(screen, origin))
    }
}

fn current_viewer_point(viewer: HWND) -> Option<(f64, f64)> {
    let _dpi = TargetDpi::physical();
    let mut screen = POINT::default();
    unsafe { GetCursorPos(&mut screen).ok()? };
    viewer_point(viewer, screen)
}

fn button_mask(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 16,
        MouseButton::Back => 32,
        MouseButton::Forward => 64,
        _ => 0,
    }
}

fn queue_mouse(queue: &mut VecDeque<MouseMessage>, message: MouseMessage) {
    if message.message == WM_MOUSEMOVE
        && queue
            .back()
            .is_some_and(|last| last.message == WM_MOUSEMOVE && last.window == message.window)
    {
        queue.pop_back();
    }
    queue.push_back(message);
}

fn release_for(mut message: MouseMessage) -> Option<(u32, MouseMessage)> {
    let (mask, up) = match message.message {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => (1, WM_LBUTTONUP),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => (2, WM_RBUTTONUP),
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => (16, WM_MBUTTONUP),
        WM_XBUTTONDOWN | WM_XBUTTONDBLCLK => (
            if message.wparam >> 16 == 1 { 32 } else { 64 },
            WM_XBUTTONUP,
        ),
        _ => return None,
    };
    message.message = up;
    message.wparam &= 0xffff0000;
    message.activate = false;
    Some((mask, message))
}

fn track_buttons(held: &mut Vec<MouseMessage>, message: MouseMessage) {
    if let Some((mask, _)) = release_for(message) {
        held.retain(|old| release_for(*old).is_none_or(|(bit, _)| bit != mask));
        held.push(message);
    } else if message.message == WM_MOUSEMOVE {
        for old in held.iter_mut().filter(|old| old.window == message.window) {
            old.point = message.point;
        }
    } else {
        held.retain(|old| {
            release_for(*old).is_none_or(|(_, up)| {
                up.message != message.message || (up.wparam >> 16) != (message.wparam >> 16)
            })
        });
    }
}

fn send_cleanup(message: MouseMessage) {
    unsafe {
        // Keep releases/cancellation behind earlier queued input. Synchronous
        // sends bypass game message pumps and tooltip relay filters.
        let _ = PostMessageW(
            Some(message.window),
            message.message,
            WPARAM(message.wparam),
            pack_point(message.point),
        );
    }
}

/// Pixel-center mapping preserves both endpoints at fractional zoom/DPI. Drag
/// endpoints are clamped only after a press has started within the image.
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
        let hooks = hooks::NativeHooks::install(viewer)?;
        let source = SourceWindow::park(source, viewer)?;
        let ex_style = unsafe { GetWindowLongW(viewer, GWL_EXSTYLE) };
        let no_activate_was_set = ex_style & WS_EX_NOACTIVATE.0 as i32 != 0;
        unsafe {
            SetWindowLongW(viewer, GWL_EXSTYLE, ex_style | WS_EX_NOACTIVATE.0 as i32);
        }
        let this = Self {
            hooks: Some(hooks),
            source,
            viewer,
            no_activate_was_set,
            view: None,
            frame_size: (0, 0),
            hover: hover::Hover::default(),
            cursor: (-1.0, -1.0),
            buttons: 0,
            captured_child: None,
            queue: VecDeque::new(),
            activate_on_flush: false,
            last_click: None,
            skip_viewer_ups: 0,
            held_messages: Vec::new(),
            input_targets: Vec::new(),
        };
        this.activate();
        Ok(this)
    }

    pub fn set_view(&mut self, view: crate::capture::CaptureRect, frame_size: (u32, u32)) {
        if self.frame_size != frame_size
            || self.view.is_some_and(|old| {
                (old.x, old.y, old.width, old.height) != (view.x, view.y, view.width, view.height)
            })
        {
            self.leave_hover();
        }
        self.view = Some(view);
        self.frame_size = frame_size;
    }

    pub fn source_covered(&self) -> bool {
        self.source.covered()
    }

    pub fn take_capture_restart(&mut self) -> bool {
        self.source.take_capture_restart()
    }

    fn source_point(&self, dragging: bool) -> Option<POINT> {
        let _dpi = TargetDpi::physical();
        let mut visible = RECT::default();
        let mut outer = RECT::default();
        let mut client = RECT::default();
        let mut origin = POINT::default();
        unsafe {
            let _ = DwmGetWindowAttribute(
                self.source.window,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut visible as *mut _ as _,
                std::mem::size_of::<RECT>() as u32,
            );
            GetWindowRect(self.source.window, &mut outer).ok()?;
            GetClientRect(self.source.window, &mut client).ok()?;
            if !ClientToScreen(self.source.window, &mut origin).as_bool() {
                return None;
            }
        }
        client.left += origin.x;
        client.right += origin.x;
        client.top += origin.y;
        client.bottom += origin.y;
        let rect = captured_bounds(self.frame_size, [visible, outer, client])?;
        map_point(self.view?, rect, self.cursor, dragging)
    }

    fn recipient(&self, screen: POINT) -> Option<(HWND, POINT)> {
        let mut child = self.captured_child.unwrap_or(self.source.window);
        for _ in 0..16 {
            let mut point = screen;
            let mut origin = POINT::default();
            unsafe {
                {
                    let _dpi = TargetDpi::physical();
                    if !ClientToScreen(child, &mut origin).as_bool() {
                        return None;
                    }
                }
                if !PhysicalToLogicalPointForPerMonitorDPI(Some(child), &mut point).as_bool() {
                    return None;
                }
                if !PhysicalToLogicalPointForPerMonitorDPI(Some(child), &mut origin).as_bool() {
                    return None;
                }
                point.x -= origin.x;
                point.y -= origin.y;
                let _dpi = TargetDpi::enter(child);
                if self.captured_child.is_some() {
                    return Some((child, point));
                }
                let next = ChildWindowFromPointEx(
                    child,
                    point,
                    CWP_SKIPDISABLED | CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT,
                );
                if next.0.is_null() || next == child {
                    let mut client = RECT::default();
                    GetClientRect(child, &mut client).ok()?;
                    if point.x < 0
                        || point.y < 0
                        || point.x >= client.right
                        || point.y >= client.bottom
                    {
                        return None;
                    }
                    return Some((child, point));
                }
                child = next;
            }
        }
        None
    }

    fn modifiers(&self) -> u32 {
        let mut flags = self.buttons;
        unsafe {
            if GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 {
                flags |= 4;
            }
            if GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 {
                flags |= 8;
            }
        }
        flags
    }

    fn enqueue(&mut self, message: MouseMessage) {
        if release_for(message).is_some() {
            self.leave_hover();
        }
        if message.message == WM_MOUSEMOVE && self.buttons == 0 {
            if let Some(leave) = self.hover.moved(message, Instant::now()) {
                queue_mouse(&mut self.queue, leave);
            }
        }
        track_buttons(&mut self.held_messages, message);
        if !self.input_targets.contains(&message.window) {
            self.input_targets.push(message.window);
        }
        queue_mouse(&mut self.queue, message);
    }

    fn leave_hover(&mut self) {
        if let Some(leave) = self.hover.leave() {
            queue_mouse(&mut self.queue, leave);
        }
    }

    fn release_button(&mut self, button: MouseButton) {
        // Mapping can fail after the source resizes or the pointer leaves it.
        // The original recipient still needs its release at the last valid point.
        let mask = button_mask(button);
        if let Some(index) = self
            .held_messages
            .iter()
            .position(|m| release_for(*m).is_some_and(|(bit, _)| bit == mask))
        {
            let held = self.held_messages.remove(index);
            if let Some((_, mut release)) = release_for(held) {
                release.wparam = ((self.buttons & !mask) as usize) | (release.wparam & 0xffff0000);
                queue_mouse(&mut self.queue, release);
            }
        }
        self.buttons &= !button_mask(button);
        if self.buttons == 0 {
            self.captured_child = None;
            if let Some(hooks) = self.hooks.as_mut() {
                let _ = hooks.set_dragging(false);
            }
            unsafe {
                let _ = ReleaseCapture();
            }
        }
    }

    pub fn handle(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::MouseInput {
            state: ElementState::Released,
            button,
            ..
        } = event
        {
            let mask = button_mask(*button);
            if self.skip_viewer_ups & mask != 0 {
                self.skip_viewer_ups &= !mask;
                return true;
            }
        }
        match event {
            WindowEvent::Focused(true) => {
                self.activate_on_flush = true;
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer(PointerEvent::Moved(position.x, position.y))
            }
            WindowEvent::CursorLeft { .. } => {
                self.leave_hover();
                false
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if *state == ElementState::Pressed {
                    // A native release can go elsewhere after a focus change.
                    self.skip_viewer_ups &= !button_mask(*button);
                }
                if let Some(point) = current_viewer_point(self.viewer) {
                    self.cursor = point;
                } else if *state == ElementState::Pressed {
                    return false;
                }
                self.pointer(PointerEvent::Button(state, button))
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some(point) = current_viewer_point(self.viewer) else {
                    return false;
                };
                self.cursor = point;
                self.pointer(PointerEvent::Wheel(delta))
            }
            _ => false,
        }
    }

    fn pointer(&mut self, event: PointerEvent<'_>) -> bool {
        if !self.source.valid() {
            return false;
        }
        match event {
            PointerEvent::Moved(x, y) => {
                self.cursor = (x, y);
                if let Some(screen) = self.source_point(self.buttons != 0) {
                    if let Some((window, point)) = self.recipient(screen) {
                        self.enqueue(MouseMessage {
                            window,
                            point,
                            message: WM_MOUSEMOVE,
                            wparam: self.modifiers() as usize,
                            activate: false,
                        });
                    } else {
                        self.leave_hover();
                    }
                    true
                } else {
                    self.leave_hover();
                    false
                }
            }
            PointerEvent::Button(state, button) => {
                self.leave_hover();
                let Some(screen) = self.source_point(self.buttons != 0) else {
                    if *state == ElementState::Released {
                        self.release_button(*button);
                    }
                    return false;
                };
                let Some((window, point)) = self.recipient(screen) else {
                    if *state == ElementState::Released {
                        self.release_button(*button);
                    }
                    return true;
                };
                let (mask, down, up, double, xbutton) = match button {
                    MouseButton::Left => (1, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, 0),
                    MouseButton::Right => (2, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_RBUTTONDBLCLK, 0),
                    MouseButton::Middle => (16, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MBUTTONDBLCLK, 0),
                    MouseButton::Back => (32, WM_XBUTTONDOWN, WM_XBUTTONUP, WM_XBUTTONDBLCLK, 1),
                    MouseButton::Forward => (64, WM_XBUTTONDOWN, WM_XBUTTONUP, WM_XBUTTONDBLCLK, 2),
                    _ => return false,
                };
                let pressed = *state == ElementState::Pressed;
                let mut message = if pressed { down } else { up };
                if pressed {
                    // Some controls use their most recent move to determine the
                    // hovered element, even when the button message has a point.
                    self.enqueue(MouseMessage {
                        window,
                        point,
                        message: WM_MOUSEMOVE,
                        wparam: self.modifiers() as usize,
                        activate: false,
                    });
                    if let Some(hooks) = self.hooks.as_mut() {
                        if let Err(error) = hooks.set_dragging(true) {
                            log::warn!("Unable to forward magnifier drag: {error}");
                            return true;
                        }
                    }
                    let first_button = self.buttons == 0;
                    self.buttons |= mask;
                    self.captured_child = Some(window);
                    if first_button {
                        unsafe {
                            SetCapture(self.viewer);
                        }
                    }
                    if let Some((last_button, last_window, last_point, last_time)) = self.last_click
                    {
                        if last_button == *button
                            && last_window == window
                            && last_time.elapsed().as_millis()
                                <= unsafe { GetDoubleClickTime() } as u128
                            && (point.x - last_point.x).abs()
                                <= unsafe { GetSystemMetrics(SM_CXDOUBLECLK) }
                            && (point.y - last_point.y).abs()
                                <= unsafe { GetSystemMetrics(SM_CYDOUBLECLK) }
                            && unsafe { GetClassLongW(window, GCL_STYLE) } & CS_DBLCLKS.0 != 0
                        {
                            message = double;
                        }
                    }
                    self.last_click = if message == double {
                        None
                    } else {
                        Some((*button, window, point, Instant::now()))
                    };
                } else {
                    self.buttons &= !mask;
                }
                if self.buttons == 0 {
                    if let Some(hooks) = self.hooks.as_mut() {
                        let _ = hooks.set_dragging(false);
                    }
                }
                self.enqueue(MouseMessage {
                    window,
                    point,
                    message,
                    wparam: self.modifiers() as usize | (xbutton << 16),
                    activate: pressed,
                });
                if self.buttons == 0 {
                    self.captured_child = None;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                }
                true
            }
            PointerEvent::Wheel(delta) => {
                let Some(screen) = self.source_point(false) else {
                    return false;
                };
                let Some((window, point)) = self.recipient(screen) else {
                    return true;
                };
                self.enqueue(MouseMessage {
                    window,
                    point,
                    message: WM_MOUSEMOVE,
                    wparam: self.modifiers() as usize,
                    activate: false,
                });
                let mut screen = screen;
                unsafe {
                    if !PhysicalToLogicalPointForPerMonitorDPI(Some(window), &mut screen).as_bool()
                    {
                        return true;
                    }
                }
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (*x as f64 * 120.0, *y as f64 * 120.0),
                    MouseScrollDelta::PixelDelta(point) => (point.x * 3.0, point.y * 3.0),
                };
                for (message, amount) in [(WM_MOUSEWHEEL, y), (WM_MOUSEHWHEEL, x)] {
                    let amount = amount.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16;
                    if amount != 0 {
                        self.enqueue(MouseMessage {
                            window,
                            point: screen,
                            message,
                            wparam: self.modifiers() as usize | ((amount as u16 as usize) << 16),
                            activate: true,
                        });
                    }
                }
                true
            }
        }
    }

    fn global_mouse(&mut self, message: u32, data: MSLLHOOKSTRUCT) -> bool {
        if self.buttons == 0 {
            return false;
        }
        let foreground = unsafe { GetForegroundWindow() };
        if foreground != self.viewer && foreground != self.source.window {
            return false;
        }
        let Some(cursor) = viewer_point(self.viewer, data.pt) else {
            return false;
        };
        self.cursor = cursor;
        if message == WM_MOUSEMOVE {
            return self.pointer(PointerEvent::Moved(self.cursor.0, self.cursor.1));
        }
        if message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
            let amount = (data.mouseData >> 16) as i16 as f32 / 120.0;
            let delta = if message == WM_MOUSEWHEEL {
                MouseScrollDelta::LineDelta(0.0, amount)
            } else {
                MouseScrollDelta::LineDelta(amount, 0.0)
            };
            return self.pointer(PointerEvent::Wheel(&delta));
        }
        let (button, released) = match message {
            WM_LBUTTONDOWN => (MouseButton::Left, false),
            WM_LBUTTONUP => (MouseButton::Left, true),
            WM_RBUTTONDOWN => (MouseButton::Right, false),
            WM_RBUTTONUP => (MouseButton::Right, true),
            WM_MBUTTONDOWN => (MouseButton::Middle, false),
            WM_MBUTTONUP => (MouseButton::Middle, true),
            WM_XBUTTONDOWN | WM_XBUTTONUP => (
                if data.mouseData >> 16 == 1 {
                    MouseButton::Back
                } else {
                    MouseButton::Forward
                },
                message == WM_XBUTTONUP,
            ),
            _ => return false,
        };
        let state = if released {
            ElementState::Released
        } else {
            ElementState::Pressed
        };
        let capture_target = self.captured_child.unwrap_or(self.source.window);
        let handled = self.pointer(PointerEvent::Button(&state, &button));
        if released && handled {
            // Deliver the mapped release before subsequent hardware movement
            // reaches a source control which may still own mouse capture.
            self.deliver_queued();
            // The hook MUST let the hardware release through: a posted message
            // cannot clear Windows' native/async button state. Consume the
            // resulting viewer event once, rather than synthesizing another up.
            self.skip_viewer_ups |= button_mask(button);
            if self.buttons == 0 {
                send_cleanup(MouseMessage {
                    window: capture_target,
                    message: WM_CANCELMODE,
                    wparam: 0,
                    point: POINT::default(),
                    activate: false,
                });
            }
        }
        handled
    }

    fn activate(&self) {
        unsafe {
            if GetForegroundWindow() != self.source.window {
                let _ = SetForegroundWindow(self.source.window);
            }
            // Source activation can raise a topmost game above the preview.
            let _ = SetWindowPos(
                self.viewer,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            // Let the target control process its mouse-down and select focus
            // itself. Attaching queues here resets held Ctrl/Shift key states.
        }
    }

    pub fn flush(&mut self) {
        if !self.source.valid() {
            self.queue.clear();
            return;
        }
        if self.buttons == 0 && self.queue.is_empty() {
            self.source.keep_parked();
        }
        for (message, data) in hooks::take_pending() {
            self.global_mouse(message, data);
        }
        let foreground = unsafe { GetForegroundWindow() };
        if foreground != self.viewer && foreground != self.source.window {
            self.leave_hover();
        } else if self.buttons == 0 {
            let mut delay = 400u32;
            unsafe {
                let _ = SystemParametersInfoW(
                    SPI_GETMOUSEHOVERTIME,
                    0,
                    Some(&mut delay as *mut _ as _),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                );
            }
            if let Some(message) = self.hover.poll(
                Instant::now(),
                std::time::Duration::from_millis(delay as u64),
            ) {
                queue_mouse(&mut self.queue, message);
            }
        }
        if self.buttons != 0
            && foreground != self.viewer
            && foreground != self.source.window
            && !self.queue.iter().any(|message| message.activate)
        {
            self.suspend_mouse();
        }
        if self.activate_on_flush {
            self.activate_on_flush = false;
            // Do not reclaim focus after the user switches to an unrelated app.
            if unsafe { GetForegroundWindow() } == self.viewer {
                self.activate();
            }
        }
        self.deliver_queued();
    }

    fn deliver_queued(&mut self) {
        while let Some(message) = self.queue.pop_front() {
            if message.activate {
                self.activate();
            }
            unsafe {
                // Games/tooltips often inspect retrieved MSGs before DispatchMessage.
                // Both ordinary input and cleanup use this ordered queue.
                if let Err(error) = PostMessageW(
                    Some(message.window),
                    message.message,
                    WPARAM(message.wparam),
                    pack_point(message.point),
                ) {
                    log::warn!("Unable to forward magnifier mouse input: {error}");
                }
            }
        }
    }

    pub fn suspend_mouse(&mut self) {
        if let Some(leave) = self.hover.leave() {
            send_cleanup(leave);
        }
        if let Some(hooks) = self.hooks.as_mut() {
            let _ = hooks.set_dragging(false);
        }
        hooks::take_pending();
        // Do not activate the game or start a queued click during shutdown.
        // Queued releases still belong to previously delivered presses.
        for message in self.queue.drain(..) {
            if matches!(
                message.message,
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP | WM_MOUSELEAVE
            ) {
                send_cleanup(message);
            }
        }
        for message in self.held_messages.drain(..) {
            if let Some((_, release)) = release_for(message) {
                send_cleanup(release);
            }
        }
        if self.source.valid() {
            unsafe {
                let thread = GetWindowThreadProcessId(self.source.window, None);
                let mut info = GUITHREADINFO {
                    cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                    ..Default::default()
                };
                if GetGUIThreadInfo(thread, &mut info).is_ok()
                    && !info.hwndCapture.0.is_null()
                    && !self.input_targets.contains(&info.hwndCapture)
                {
                    self.input_targets.push(info.hwndCapture);
                }
                if !self.input_targets.contains(&self.source.window) {
                    self.input_targets.push(self.source.window);
                }
            }
        }
        for window in self.input_targets.drain(..) {
            // ReleaseCapture only affects OUR thread. Ask the owning game/control
            // to cancel its capture before restoring the source placement.
            send_cleanup(MouseMessage {
                window,
                message: WM_CANCELMODE,
                wparam: 0,
                point: POINT::default(),
                activate: false,
            });
        }
        // A game can confine the system cursor independently of mouse capture.
        // Its old screen rectangle becomes stale when we restore its position.
        // Never change confinement while an unrelated app owns the foreground.
        if self.source.valid() && unsafe { GetForegroundWindow() } == self.source.window {
            unsafe {
                let _ = ClipCursor(None);
            }
        }
        self.buttons = 0;
        self.captured_child = None;
        self.last_click = None;
        self.activate_on_flush = false;
        unsafe {
            let _ = ReleaseCapture();
        }
    }
}

impl Drop for InputBridge {
    fn drop(&mut self) {
        self.hooks.take();
        self.suspend_mouse();
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
        // SourceWindow drops after this and restores the original placement.
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
    fn release_retains_recipient_and_last_valid_point_without_remapping() {
        let window = HWND(42usize as _);
        let mut held = Vec::new();
        let press = MouseMessage {
            window,
            message: WM_LBUTTONDOWN,
            wparam: 1,
            point: POINT { x: 10, y: 20 },
            activate: true,
        };
        track_buttons(&mut held, press);
        track_buttons(
            &mut held,
            MouseMessage {
                message: WM_MOUSEMOVE,
                point: POINT { x: -4, y: 150 },
                ..press
            },
        );
        let (mask, up) = release_for(held[0]).unwrap();
        assert_eq!(mask, 1);
        assert_eq!(up.window, window);
        assert_eq!((up.point.x, up.point.y), (-4, 150));
        assert_eq!(up.message, WM_LBUTTONUP);
        assert!(!up.activate);
        track_buttons(&mut held, up);
        assert!(held.is_empty());
    }

    #[test]
    fn side_buttons_and_native_releases_remain_independent() {
        let mut held = Vec::new();
        for id in [1, 2] {
            track_buttons(
                &mut held,
                MouseMessage {
                    window: HWND(42usize as _),
                    message: WM_XBUTTONDOWN,
                    wparam: id << 16,
                    point: POINT::default(),
                    activate: true,
                },
            );
        }
        let (_, up) = release_for(held[0]).unwrap();
        track_buttons(&mut held, up);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].wparam >> 16, 2);
        for message in [WM_LBUTTONUP, WM_RBUTTONUP, WM_MBUTTONUP, WM_XBUTTONUP] {
            assert!(hooks::is_button_release(message));
        }
        assert!(!hooks::is_button_release(WM_MOUSEMOVE));
        assert!(!hooks::is_button_release(WM_LBUTTONDOWN));
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
    fn high_frequency_moves_preserve_click_and_release_order() {
        let mut queue = VecDeque::new();
        let window = HWND(42usize as _);
        for (message, x) in [
            (WM_MOUSEMOVE, 1),
            (WM_MOUSEMOVE, 2),
            (WM_LBUTTONDOWN, 2),
            (WM_MOUSEMOVE, 3),
            (WM_MOUSEMOVE, 4),
            (WM_LBUTTONUP, 4),
            (WM_MOUSEMOVE, 5),
        ] {
            queue_mouse(
                &mut queue,
                MouseMessage {
                    window,
                    message,
                    point: POINT { x, y: 0 },
                    wparam: 0,
                    activate: false,
                },
            );
        }
        assert_eq!(
            queue
                .iter()
                .map(|event| (event.message, event.point.x))
                .collect::<Vec<_>>(),
            [
                (WM_MOUSEMOVE, 2),
                (WM_LBUTTONDOWN, 2),
                (WM_MOUSEMOVE, 4),
                (WM_LBUTTONUP, 4),
                (WM_MOUSEMOVE, 5)
            ]
        );
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

    #[test]
    fn mouse_message_coordinates_preserve_negative_signed_values() {
        let encoded = pack_point(POINT { x: -1200, y: -250 });
        assert_eq!((encoded.0 as u16) as i16, -1200);
        assert_eq!(((encoded.0 >> 16) as u16) as i16, -250);
    }
}
