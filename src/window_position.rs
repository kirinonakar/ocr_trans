use i_slint_backend_winit::{EventResult, WinitWindowAccessor};
use winit::event::WindowEvent;

/// Restore before the first show, then persist only normal, visible window positions.
/// Both APIs use physical outer-window coordinates, including on mixed-DPI desktops.
pub(crate) fn track(
    window: &slint::Window,
    saved: Option<(i32, i32)>,
    mut save: impl FnMut((i32, i32)) + 'static,
) {
    if let Some(position) = saved {
        let (x, y) = reachable_position(position);
        window.set_position(slint::PhysicalPosition::new(x, y));
    }
    let mut last_position = saved;
    window.on_winit_window_event(move |window, event| {
        if matches!(event, WindowEvent::Moved(_) | WindowEvent::CloseRequested)
            && window.is_visible()
        {
            let position = window
                .with_winit_window(|native| {
                    if native.is_minimized() == Some(true) || native.is_maximized() {
                        return None;
                    }
                    native.outer_position().ok().map(|p| (p.x, p.y))
                })
                .flatten();
            if let Some(position) = position {
                if last_position != Some(position) {
                    last_position = Some(position);
                    save(position);
                }
            }
        }
        EventResult::Propagate
    });
}

#[cfg(target_os = "windows")]
fn reachable_position(position: (i32, i32)) -> (i32, i32) {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                // Sample inside the title bar: the invisible outer resize border may
                // extend onto the neighboring monitor, especially at higher DPI.
                x: position.0.saturating_add(80),
                y: position.1.saturating_add(16),
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        let work = info.rcWork;
        keep_title_visible(position, (work.left, work.top, work.right, work.bottom))
    } else {
        position
    }
}

#[cfg(not(target_os = "windows"))]
fn reachable_position(position: (i32, i32)) -> (i32, i32) {
    position
}

#[cfg(any(target_os = "windows", test))]
fn keep_title_visible(
    (x, y): (i32, i32),
    (left, top, right, bottom): (i32, i32, i32, i32),
) -> (i32, i32) {
    // Retain normal placements, including the invisible resize border outside a work area.
    // Leave enough title bar visible to drag a window back after a monitor is disconnected.
    (
        x.clamp(left.saturating_sub(16), right.saturating_sub(160).max(left)),
        y.clamp(top.saturating_sub(16), bottom.saturating_sub(40).max(top)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_monitor_positions_and_resize_borders_are_preserved() {
        let work = (-1920, 0, 0, 1040);
        assert_eq!(keep_title_visible((-1700, 120), work), (-1700, 120));
        assert_eq!(keep_title_visible((-1928, 0), work), (-1928, 0));
        assert_eq!(keep_title_visible((-1932, -12), work), (-1932, -12));
    }

    #[test]
    fn disconnected_monitor_and_taskbar_positions_keep_a_draggable_title() {
        let work = (0, 48, 1920, 1080);
        assert_eq!(keep_title_visible((-1800, 100), work), (-16, 100));
        assert_eq!(keep_title_visible((2500, -500), work), (1760, 32));
        assert_eq!(keep_title_visible((1900, 1200), work), (1760, 1040));
        assert_eq!(keep_title_visible((i32::MAX, i32::MIN), work), (1760, 32));
    }
}
