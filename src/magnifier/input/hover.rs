use super::*;
use std::time::Duration;

#[derive(Default)]
pub(super) struct Hover {
    target: Option<(MouseMessage, Instant, bool)>,
}

impl Hover {
    pub fn leave(&mut self) -> Option<MouseMessage> {
        self.target.take().map(|(mut message, _, _)| {
            message.message = WM_MOUSELEAVE;
            message.wparam = 0;
            message.point = POINT::default();
            message
        })
    }

    pub fn moved(&mut self, message: MouseMessage, now: Instant) -> Option<MouseMessage> {
        if let Some((old, since, sent)) = self.target.as_mut() {
            if old.window == message.window {
                // A new position rearms dwell, while repeated identical move
                // messages must not postpone it indefinitely.
                if (old.point.x, old.point.y) != (message.point.x, message.point.y) {
                    *since = now;
                    *sent = false;
                }
                *old = message;
                return None;
            }
        }
        let leave = self.leave();
        self.target = Some((message, now, false));
        leave
    }

    pub fn poll(&mut self, now: Instant, delay: Duration) -> Option<MouseMessage> {
        let (last, since, sent) = self.target.as_mut()?;
        if *sent || now.saturating_duration_since(*since) < delay {
            return None;
        }
        *sent = true;
        Some(MouseMessage {
            message: WM_MOUSEHOVER,
            activate: false,
            ..*last
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dwell_fires_once_and_rearms_on_motion_and_target_change() {
        let mut hover = Hover::default();
        let now = Instant::now();
        let delay = Duration::from_millis(400);
        let message = MouseMessage {
            window: HWND(1usize as _),
            message: WM_MOUSEMOVE,
            point: POINT { x: 20, y: 30 },
            wparam: 0,
            activate: false,
        };
        assert!(hover.moved(message, now).is_none());
        assert!(hover.poll(now + delay / 2, delay).is_none());
        hover.moved(message, now + delay / 2);
        assert_eq!(
            hover.poll(now + delay, delay).unwrap().message,
            WM_MOUSEHOVER
        );
        assert!(hover.poll(now + delay * 2, delay).is_none());
        let next = MouseMessage {
            window: HWND(2usize as _),
            ..message
        };
        assert_eq!(
            hover.moved(next, now + delay * 2).unwrap().window,
            message.window
        );
        assert!(hover.poll(now + delay * 2, delay).is_none());
        assert_eq!(
            hover.poll(now + delay * 3, delay).unwrap().window,
            next.window
        );
        assert_eq!(hover.leave().unwrap().message, WM_MOUSELEAVE);
        assert!(hover.poll(now + delay * 4, delay).is_none());
    }
}
