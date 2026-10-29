//! HWND capture excludes Explorer tooltips/menus because they are separate
//! windows. Capture only intersecting popups associated with the selected HWND; enabling
//! IncludeSecondaryWindows would also capture our overlapping tool window and
//! recursively magnify the preview itself.
use super::window_capture::WindowCapture;
use image::RgbaImage;
use std::time::{Duration, Instant};
use windows::core::BOOL;
use windows::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
    UI::WindowsAndMessaging::*,
};

struct Popup {
    window: HWND,
    capture: WindowCapture,
    pixels: Option<RgbaImage>,
}

pub(super) struct Popups {
    source: HWND,
    windows: Vec<Popup>,
    last_scan: Instant,
}

fn bounds(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_err()
        {
            GetWindowRect(window, &mut rect).ok()?;
        }
    }
    Some(rect)
}

fn intersects(a: RECT, b: RECT) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

fn overlay_popup(result: &mut RgbaImage, pixels: &RgbaImage, source: RECT, popup: RECT) {
    image::imageops::overlay(
        result,
        pixels,
        i64::from(popup.left) - i64::from(source.left),
        i64::from(popup.top) - i64::from(source.top),
    );
}

#[derive(Clone, Copy)]
enum PopupOwner {
    Source,
    Unowned,
    Other,
}

fn include_popup(owner: PopupOwner, same_thread: bool, class: &str) -> bool {
    // Explorer's desktop and taskbar share its process and can have popup/tool
    // styles. Compositing them would replace the selected Explorer window with
    // the wallpaper at the same screen coordinates.
    if [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
    ]
    .iter()
    .any(|shell| class.eq_ignore_ascii_case(shell))
    {
        return false;
    }
    match owner {
        PopupOwner::Source => true,
        // Some native menus/tooltips have no owner. Only these known transient
        // classes on the source UI thread are safe to associate without an owner.
        PopupOwner::Unowned => {
            same_thread && (class.eq_ignore_ascii_case("tooltips_class32") || class == "#32768")
        }
        PopupOwner::Other => false,
    }
}

struct Search {
    source: HWND,
    pid: u32,
    thread: u32,
    rect: RECT,
    windows: Vec<HWND>,
}
unsafe extern "system" fn enumerate(window: HWND, param: LPARAM) -> BOOL {
    let search = &mut *(param.0 as *mut Search);
    let mut pid = 0;
    let thread = GetWindowThreadProcessId(window, Some(&mut pid));
    if window == search.source
        || pid != search.pid
        || !IsWindowVisible(window).as_bool()
        || IsIconic(window).as_bool()
    {
        return BOOL(1);
    }
    let style = GetWindowLongW(window, GWL_STYLE) as u32;
    let ex = GetWindowLongW(window, GWL_EXSTYLE) as u32;
    // Avoid overlaying another ordinary Explorer window from the same process.
    if style & WS_POPUP.0 == 0 && ex & WS_EX_TOOLWINDOW.0 == 0 {
        return BOOL(1);
    }
    let direct_owner = GetWindow(window, GW_OWNER).unwrap_or_default();
    let owner = if GetAncestor(window, GA_ROOTOWNER) == search.source
        || (!direct_owner.is_invalid() && GetAncestor(direct_owner, GA_ROOT) == search.source)
    {
        PopupOwner::Source
    } else if direct_owner.is_invalid() {
        PopupOwner::Unowned
    } else {
        PopupOwner::Other
    };
    let mut class = [0u16; 256];
    let length = GetClassNameW(window, &mut class) as usize;
    if !include_popup(
        owner,
        thread == search.thread,
        &String::from_utf16_lossy(&class[..length]),
    ) {
        return BOOL(1);
    }
    if bounds(window).is_some_and(|rect| intersects(rect, search.rect)) {
        search.windows.push(window);
    }
    BOOL((search.windows.len() < 8) as i32)
}

impl Popups {
    pub fn new(source: isize) -> Self {
        Self {
            source: HWND(source as _),
            windows: Vec::new(),
            last_scan: Instant::now() - Duration::from_secs(1),
        }
    }

    pub fn composite(&mut self, original: &RgbaImage) -> Option<RgbaImage> {
        // All worker coordinates are physical pixels, including DPI-unaware apps.
        let Some(source_rect) =
            super::super::input::capture_bounds(self.source, original.dimensions())
        else {
            return None;
        };
        if self.last_scan.elapsed() >= Duration::from_millis(100) {
            self.last_scan = Instant::now();
            let mut search = Search {
                source: self.source,
                pid: 0,
                thread: 0,
                rect: source_rect,
                windows: Vec::new(),
            };
            unsafe {
                search.thread = GetWindowThreadProcessId(self.source, Some(&mut search.pid));
                let _ = EnumWindows(Some(enumerate), LPARAM(&mut search as *mut _ as isize));
            }
            self.windows
                .retain(|popup| search.windows.contains(&popup.window));
            for window in &search.windows {
                if !self.windows.iter().any(|popup| popup.window == *window) {
                    match WindowCapture::new(window.0 as isize) {
                        Ok(capture) => self.windows.push(Popup {
                            window: *window,
                            capture,
                            pixels: None,
                        }),
                        Err(error) => log::debug!("Popup capture unavailable: {error:#}"),
                    }
                }
            }
            // EnumWindows is top-to-bottom; composite in the opposite order.
            self.windows.sort_by_key(|popup| {
                std::cmp::Reverse(
                    search
                        .windows
                        .iter()
                        .position(|window| *window == popup.window),
                )
            });
        }
        let mut result = None;
        for popup in &mut self.windows {
            if !unsafe { IsWindowVisible(popup.window) }.as_bool() {
                continue;
            }
            match popup.capture.next_frame() {
                Ok(Some(pixels)) => popup.pixels = Some(pixels),
                Ok(None) => {}
                Err(_) => {
                    popup.pixels = None;
                    continue;
                }
            }
            if let Some(pixels) = &popup.pixels {
                if let Some(rect) =
                    super::super::input::capture_bounds(popup.window, pixels.dimensions())
                {
                    // Overlay clips all four sides, including negative monitors.
                    overlay_popup(
                        result.get_or_insert_with(|| original.clone()),
                        pixels,
                        source_rect,
                        rect,
                    );
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    #[test]
    fn explorer_shell_surfaces_are_never_composited_over_the_selected_window() {
        for class in [
            "Progman",
            "WorkerW",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
        ] {
            // Sharing Explorer's process, UI thread and popup styles is not
            // enough to make its desktop/taskbar part of the selected window.
            assert!(!include_popup(PopupOwner::Unowned, true, class));
            assert!(!include_popup(PopupOwner::Source, true, class));
        }
        assert!(!include_popup(
            PopupOwner::Unowned,
            true,
            "UnrelatedToolWindow"
        ));
        assert!(!include_popup(PopupOwner::Other, true, "tooltips_class32"));
    }

    #[test]
    fn source_popups_and_unowned_native_tooltips_keep_their_association() {
        assert!(include_popup(PopupOwner::Source, false, "CustomTooltip"));
        for class in ["tooltips_class32", "TOOLTIPS_CLASS32", "#32768"] {
            assert!(include_popup(PopupOwner::Unowned, true, class));
            assert!(!include_popup(PopupOwner::Unowned, false, class));
        }
    }

    #[test]
    fn tooltip_composition_clips_negative_offsets_and_does_not_leave_ghosts() {
        let original = RgbaImage::from_pixel(4, 4, Rgba([20, 30, 40, 255]));
        let popup = RgbaImage::from_pixel(3, 2, Rgba([200, 100, 50, 255]));
        let mut composed = original.clone();
        let source_rect = RECT {
            left: -1920,
            top: -300,
            right: -1916,
            bottom: -296,
        };
        let popup_rect = RECT {
            left: -1921,
            top: -297,
            right: -1918,
            bottom: -295,
        };
        overlay_popup(&mut composed, &popup, source_rect, popup_rect);
        assert_eq!(*composed.get_pixel(0, 3), Rgba([200, 100, 50, 255]));
        assert_eq!(composed.get_pixel(2, 3), original.get_pixel(2, 3));
        assert_eq!(*original.get_pixel(0, 3), Rgba([20, 30, 40, 255]));
    }
}
