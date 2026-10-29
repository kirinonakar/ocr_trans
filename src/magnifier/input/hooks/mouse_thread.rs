//! Own the low-level hook on a thread independent of Slint/rendering.
//! A thread-message barrier publishes events only AFTER the hook has returned.
//! See PowerToys' deferred-injection rationale:
//! https://github.com/microsoft/PowerToys/blob/main/doc/devdocs/modules/mouseutils/mousebuttonlock.md#architecture
use super::{queue_pending, HardwareEvent};
use anyhow::{Context, Result};
use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::*,
};

const RELAY: u32 = WM_APP + 0x521;
pub(super) const DISPATCH: u32 = WM_APP + 0x522;
static NEXT_GENERATION: AtomicUsize = AtomicUsize::new(1);
thread_local! {
    static UI_ROUTE: RefCell<Option<Arc<Route>>> = const { RefCell::new(None) };
    static HOOK_ROUTE: RefCell<Option<Arc<Route>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Pending {
    incoming: VecDeque<HardwareEvent>,
    ready: VecDeque<HardwareEvent>,
    relay_posted: bool,
    dispatch_posted: bool,
}
impl Pending {
    fn enqueue(&mut self, event: HardwareEvent) -> bool {
        queue_pending(&mut self.incoming, event);
        !std::mem::replace(&mut self.relay_posted, true)
    }
    fn relay(&mut self) -> bool {
        self.relay_posted = false;
        for event in self.incoming.drain(..) {
            queue_pending(&mut self.ready, event);
        }
        !self.ready.is_empty() && !std::mem::replace(&mut self.dispatch_posted, true)
    }
    fn take_ready(&mut self) -> VecDeque<HardwareEvent> {
        self.dispatch_posted = false;
        std::mem::take(&mut self.ready)
    }
    fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Default)]
struct CaptureGate(AtomicU8);
impl CaptureGate {
    fn capturing(&self) -> bool {
        self.0.load(Ordering::Acquire) != 0
    }
    fn begin_entry(&self) {
        let _ = self
            .0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }
    fn acknowledge(&self, active: bool) {
        self.0.store(if active { 2 } else { 0 }, Ordering::Release);
    }
}

struct Route {
    viewer: isize,
    generation: usize,
    thread: AtomicU32,
    gate: CaptureGate,
    closed: AtomicBool,
    failed: AtomicBool,
    preview: Mutex<Option<RECT>>,
    pending: Mutex<Pending>,
}
impl Route {
    fn enqueue(&self, event: HardwareEvent) {
        let relay = self.pending.lock().unwrap().enqueue(event);
        if relay {
            // Posting to OUR OWN thread, not directly to the UI, is essential:
            // the UI could run before this callback returns on another CPU.
            if unsafe {
                PostThreadMessageW(
                    self.thread.load(Ordering::Acquire),
                    RELAY,
                    WPARAM(0),
                    LPARAM(0),
                )
            }
            .is_err()
            {
                self.pending.lock().unwrap().relay_posted = false;
                self.failed.store(true, Ordering::Release);
            }
        }
    }
    fn relay(&self) {
        let dispatch = self.pending.lock().unwrap().relay();
        if dispatch {
            let result = unsafe {
                PostMessageW(
                    Some(HWND(self.viewer as _)),
                    DISPATCH,
                    WPARAM(self.generation),
                    LPARAM(0),
                )
            };
            if result.is_err() {
                self.pending.lock().unwrap().dispatch_posted = false;
                self.failed.store(true, Ordering::Release);
            }
        }
    }
}

pub(super) struct MouseThread {
    route: Arc<Route>,
}
impl MouseThread {
    pub fn start(viewer: HWND) -> Result<Self> {
        let route = Arc::new(Route {
            viewer: viewer.0 as isize,
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            thread: AtomicU32::new(0),
            gate: CaptureGate::default(),
            closed: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            preview: Mutex::new(None),
            pending: Mutex::new(Pending::default()),
        });
        let worker = route.clone();
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("magnifier-mouse-hook".into())
            .spawn(move || unsafe {
                // This dedicated thread uses physical coordinates for its
                // entire lifetime; don't switch DPI context on every mouse tick.
                let _dpi = super::super::TargetDpi::physical();
                worker.thread.store(GetCurrentThreadId(), Ordering::Release);
                let mut message = MSG::default();
                let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                HOOK_ROUTE.with(|slot| *slot.borrow_mut() = Some(worker.clone()));
                let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) {
                    Ok(hook) => hook,
                    Err(error) => {
                        let _ = send.send(Err(error.to_string()));
                        return;
                    }
                };
                struct Unhook(HHOOK);
                impl Drop for Unhook {
                    fn drop(&mut self) {
                        unsafe {
                            let _ = UnhookWindowsHookEx(self.0);
                        }
                    }
                }
                let _unhook = Unhook(hook);
                if send.send(Ok(())).is_err() {
                    return;
                }
                loop {
                    let result = GetMessageW(&mut message, None, 0, 0).0;
                    if result <= 0 {
                        if result < 0 {
                            worker.failed.store(true, Ordering::Release);
                        }
                        break;
                    }
                    if message.message == RELAY {
                        worker.relay();
                    } else {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                HOOK_ROUTE.with(|slot| slot.borrow_mut().take());
            })
            .context("Unable to start mouse hook thread")?;
        receive
            .recv()
            .context("Mouse hook thread stopped during startup")?
            .map_err(anyhow::Error::msg)?;
        UI_ROUTE.with(|slot| *slot.borrow_mut() = Some(route.clone()));
        Ok(Self { route })
    }
    pub fn failed(&self) -> bool {
        self.route.failed.load(Ordering::Acquire)
    }
}
impl Drop for MouseThread {
    fn drop(&mut self) {
        self.route.closed.store(true, Ordering::Release);
        self.route.gate.acknowledge(false);
        self.route.pending.lock().unwrap().clear();
        UI_ROUTE.with(|slot| {
            let current = slot
                .borrow()
                .as_ref()
                .is_some_and(|route| route.generation == self.route.generation);
            if current {
                slot.borrow_mut().take();
            }
        });
        // Never join a hook thread from the UI: another application's hook may
        // still be on the chain. The thread owns/unhooks its handle on exit.
        unsafe {
            let _ = PostThreadMessageW(
                self.route.thread.load(Ordering::Acquire),
                WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

pub(super) fn set_active(active: bool) {
    UI_ROUTE.with(|slot| {
        if let Some(route) = slot.borrow().as_ref() {
            route.gate.acknowledge(active);
        }
    });
}
pub(super) fn set_preview(preview: Option<RECT>) {
    UI_ROUTE.with(|slot| {
        if let Some(route) = slot.borrow().as_ref() {
            *route.preview.lock().unwrap() = preview;
        }
    });
}
pub(super) fn matches_generation(generation: usize) -> bool {
    UI_ROUTE.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|route| route.generation == generation)
    })
}
pub(super) fn take_pending() -> VecDeque<HardwareEvent> {
    UI_ROUTE.with(|slot| {
        slot.borrow().as_ref().map_or_else(VecDeque::new, |route| {
            route.pending.lock().unwrap().take_ready()
        })
    })
}
pub(super) fn clear_pending() {
    UI_ROUTE.with(|slot| {
        if let Some(route) = slot.borrow().as_ref() {
            route.pending.lock().unwrap().clear();
        }
    });
}

fn preview_candidate(rect: Option<RECT>, point: POINT) -> bool {
    rect.is_some_and(|rect| {
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    })
}
fn is_press(message: u32) -> bool {
    matches!(
        message,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    )
}
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 || lparam.0 == 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let message = wparam.0 as u32;
    if !matches!(
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
    ) {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let data = *(lparam.0 as *const MSLLHOOKSTRUCT);
    if data.dwExtraInfo == super::super::native::INPUT_TAG {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let handled = HOOK_ROUTE.with(|slot| {
        let slot = slot.borrow();
        let Some(route) = slot.as_ref() else {
            return false;
        };
        if route.closed.load(Ordering::Acquire) || route.failed.load(Ordering::Acquire) {
            return false;
        }
        let active = route.gate.capturing();
        if !active && !preview_candidate(*route.preview.lock().unwrap(), data.pt) {
            return false;
        }
        // Only a possible first press needs a cheap native hit check. No remote
        // geometry, focus/style changes, SendInput or Slint work in this hook.
        let intercept =
            active || (is_press(message) && WindowFromPoint(data.pt) == HWND(route.viewer as _));
        if !active && message != WM_MOUSEMOVE && !intercept {
            return false;
        }
        if !active && intercept {
            // The matching physical up must also be swallowed while the UI is
            // still entering native mode; otherwise up could precede injected down.
            route.gate.begin_entry();
        }
        let mut origin = data.pt;
        if active && message == WM_MOUSEMOVE && GetCursorPos(&mut origin).is_err() {
            return false;
        }
        route.enqueue(HardwareEvent {
            message,
            data,
            origin,
            native: active,
            intercepted: intercept,
        });
        intercept
    });
    if handled {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(message: u32, time: u32) -> HardwareEvent {
        HardwareEvent {
            message,
            native: true,
            intercepted: true,
            data: MSLLHOOKSTRUCT {
                time,
                ..Default::default()
            },
            origin: POINT::default(),
        }
    }
    #[test]
    fn first_press_gate_also_captures_release_before_ui_can_enter_native_mode() {
        let gate = CaptureGate::default();
        assert!(!gate.capturing());
        gate.begin_entry();
        assert!(gate.capturing()); // Matching up cannot leak before injected down.
        gate.acknowledge(true);
        gate.begin_entry(); // Late worker entry cannot overwrite an active UI ack.
        assert_eq!(gate.0.load(Ordering::Acquire), 2);
        gate.acknowledge(false);
        assert!(!gate.capturing());
        gate.begin_entry();
        gate.acknowledge(false); // Entry failure.
        assert!(!gate.capturing());
    }

    #[test]
    fn concurrent_relay_and_ui_draining_keep_every_button_in_order() {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let producer = pending.clone();
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let worker = std::thread::spawn(move || {
            for time in 1..=512 {
                producer.lock().unwrap().enqueue(event(
                    if time % 2 == 1 {
                        WM_LBUTTONDOWN
                    } else {
                        WM_LBUTTONUP
                    },
                    time,
                ));
                producer.lock().unwrap().relay();
                std::thread::yield_now();
            }
            finished.store(true, Ordering::Release);
        });
        let mut events = Vec::new();
        while !done.load(Ordering::Acquire) {
            events.extend(pending.lock().unwrap().take_ready());
            std::thread::yield_now();
        }
        worker.join().unwrap();
        events.extend(pending.lock().unwrap().take_ready());
        assert_eq!(
            events.iter().map(|e| e.data.time).collect::<Vec<_>>(),
            (1..=512).collect::<Vec<_>>()
        );
    }

    #[test]
    fn timer_cannot_consume_an_event_before_the_hook_returns() {
        let mut queue = Pending::default();
        assert!(queue.enqueue(event(WM_LBUTTONDOWN, 1000)));
        assert!(queue.take_ready().is_empty());
        assert!(!queue.enqueue(event(WM_LBUTTONUP, 1030)));
        assert!(queue.relay());
        assert_eq!(
            queue
                .take_ready()
                .iter()
                .map(|e| (e.message, e.data.time))
                .collect::<Vec<_>>(),
            [(WM_LBUTTONDOWN, 1000), (WM_LBUTTONUP, 1030)]
        );
    }
    #[test]
    fn double_click_order_survives_relay_and_ui_render_delays() {
        let mut queue = Pending::default();
        queue.enqueue(event(WM_LBUTTONDOWN, 1000));
        queue.relay();
        queue.enqueue(event(WM_LBUTTONUP, 1030));
        queue.relay();
        queue.enqueue(event(WM_LBUTTONDOWN, 1200));
        queue.enqueue(event(WM_LBUTTONUP, 1230));
        queue.relay();
        assert_eq!(
            queue
                .take_ready()
                .iter()
                .map(|e| (e.message, e.data.time))
                .collect::<Vec<_>>(),
            [
                (WM_LBUTTONDOWN, 1000),
                (WM_LBUTTONUP, 1030),
                (WM_LBUTTONDOWN, 1200),
                (WM_LBUTTONUP, 1230)
            ]
        );
        assert!(!queue.relay());
    }
    #[test]
    fn incoming_events_wait_for_their_own_barrier_and_close_cancels_both_queues() {
        let mut queue = Pending::default();
        queue.enqueue(event(WM_LBUTTONDOWN, 1000));
        queue.relay();
        queue.enqueue(event(WM_LBUTTONUP, 1030));
        assert_eq!(queue.take_ready().len(), 1);
        assert!(queue.take_ready().is_empty());
        assert!(queue.relay());
        assert_eq!(queue.take_ready().front().unwrap().message, WM_LBUTTONUP);
        queue.enqueue(event(WM_MOUSEWHEEL, 1300));
        queue.relay();
        queue.enqueue(event(WM_MOUSEWHEEL, 1400));
        queue.clear();
        assert!(queue.take_ready().is_empty());
        assert!(!queue.relay());
    }
}
