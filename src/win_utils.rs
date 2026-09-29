use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMWA_CAPTION_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_TEXT_COLOR, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SendMessageW, SetLayeredWindowAttributes, SetWindowLongW, GWL_EXSTYLE,
    HTCAPTION, LWA_ALPHA, WM_NCLBUTTONDOWN, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

/// Shows an OS-owned menu, which is not clipped to the Slint toolbar's client area.
/// `screen_x`/`screen_y` locate the menu's top-left corner in screen coordinates.
#[cfg(target_os = "windows")]
pub fn choose_popup_item(
    hwnd: HWND,
    labels: &[String],
    selected: i32,
    screen_x: i32,
    screen_y: i32,
) -> windows::core::Result<Option<usize>> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, PostMessageW, SetForegroundWindow,
        TrackPopupMenu, HMENU, MF_CHECKED, MF_STRING, TPM_NONOTIFY, TPM_RETURNCMD, WM_NULL,
    };

    struct Menu(HMENU);
    impl Drop for Menu {
        fn drop(&mut self) {
            unsafe { let _ = DestroyMenu(self.0); }
        }
    }

    unsafe {
        let menu = Menu(CreatePopupMenu()?);
        for (index, label) in labels.iter().enumerate() {
            let label: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
            let flags = if index as i32 == selected { MF_STRING | MF_CHECKED } else { MF_STRING };
            AppendMenuW(menu.0, flags, index + 1, PCWSTR(label.as_ptr()))?;
        }
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(menu.0, TPM_RETURNCMD | TPM_NONOTIFY,
            screen_x, screen_y, 0, hwnd, None).0;
        // Ensure a subsequent opening also dismisses correctly on an outside click.
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        Ok((command > 0).then(|| command as usize - 1).filter(|index| *index < labels.len()))
    }
}

/// Sets the window to be click-through by applying WS_EX_TRANSPARENT and WS_EX_LAYERED styles.
#[allow(dead_code)]
pub fn set_click_through(hwnd: HWND, enable: bool) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        if enable {
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ex_style | (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as i32,
            );
            let _ = SetLayeredWindowAttributes(
                hwnd,
                windows::Win32::Foundation::COLORREF(0),
                255,
                LWA_ALPHA,
            );
        } else {
            SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ex_style & !(WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as i32,
            );
        }
    }
}

/// Sets the window to be a tool window (hides from taskbar).
/// If can_focus is false, it also adds WS_EX_NOACTIVATE to prevent focus stealing.
pub fn set_tool_window(hwnd: HWND, can_focus: bool) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        };
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        // WS_EX_TOOLWINDOW hides from taskbar
        let mut new_style = (ex_style | WS_EX_TOOLWINDOW.0 as i32) & !(WS_EX_APPWINDOW.0 as i32);

        if can_focus {
            new_style &= !(WS_EX_NOACTIVATE.0 as i32);
        } else {
            new_style |= WS_EX_NOACTIVATE.0 as i32;
        }

        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, new_style);
    }
}

/// Set the owner window to hide the child window from taskbar.
pub fn set_window_owner(child: HWND, owner: HWND) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, GWLP_HWNDPARENT};
        let _ = SetWindowLongPtrW(child, GWLP_HWNDPARENT, owner.0 as isize);
    }
}

/// Sets the window to be layered (essential for alpha transparency on Windows).
pub fn set_layered(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED.0 as i32);
        let _ = SetLayeredWindowAttributes(
            hwnd,
            windows::Win32::Foundation::COLORREF(0),
            255,
            LWA_ALPHA,
        );
    }
}

/// Re-applies the topmost band to the window without resizing it, moving it, or stealing focus.
///
/// An application running in fullscreen can push an always-on-top overlay behind itself, so the
/// OCR overlay re-applies HWND_TOPMOST while such an application owns the foreground.
pub fn set_topmost(hwnd: HWND) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
        };
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

/// Returns true when the foreground window of another process covers its whole monitor, which is
/// how a fullscreen application presents itself (browser, game, video player, ...). This app's own
/// windows are ignored so switching between them never triggers the overlay guard, and merely
/// maximized windows are excluded via IsZoomed.
pub fn foreground_app_is_fullscreen() -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsZoomed,
    };

    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return false;
        }
        let mut process_id = 0u32;
        let _ = GetWindowThreadProcessId(foreground, Some(&mut process_id));
        if process_id == std::process::id() || IsZoomed(foreground).as_bool() {
            return false;
        }
        let mut window_rect = RECT::default();
        if GetWindowRect(foreground, &mut window_rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            return false;
        }
        let monitor_rect = monitor_info.rcMonitor;
        window_rect.left <= monitor_rect.left
            && window_rect.top <= monitor_rect.top
            && window_rect.right >= monitor_rect.right
            && window_rect.bottom >= monitor_rect.bottom
    }
}

/// Applies the Mica backdrop effect (Windows 11).
pub fn set_mica_backdrop(hwnd: HWND) {
    unsafe {
        let value = DWMSBT_MAINWINDOW.0 as i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &value as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Applies the requested theme to the native title bar of the OCR settings window.
///
/// Slint's `dark_theme` property only changes the client area. The non-client title bar is
/// owned by Windows, so it needs the DWM attributes as well to avoid a light title bar above a
/// dark OCR UI (and vice versa).
pub fn set_title_bar_theme(hwnd: HWND, dark: bool) {
    unsafe {
        let dark_mode = if dark { 1i32 } else { 0i32 };
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );

        // COLORREF stores RGB values in Windows' 0x00BBGGRR layout.
        // Dark caption matches the capture toolbar (#2b2b31), not the old navy client theme.
        let caption_color = COLORREF(if dark { 0x00312b2b } else { 0x00fcfaf8 });
        let text_color = COLORREF(if dark { 0x00fcfaf8 } else { 0x001b1811 });
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &caption_color as *const _ as *const _,
            std::mem::size_of::<COLORREF>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR,
            &text_color as *const _ as *const _,
            std::mem::size_of::<COLORREF>() as u32,
        );
    }
}

/// Excludes the window from any desktop capture.
pub fn set_exclude_from_capture(hwnd: windows::Win32::Foundation::HWND) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
        };
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    }
}

/// Disables window transitions (animations like fade/slide) for the given window.
pub fn disable_window_transitions(hwnd: HWND) {
    unsafe {
        let value = 1i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &value as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Starts the standard Win32 title-bar drag behavior for a frameless toolbar.
/// Sending WM_NCLBUTTONDOWN keeps the interaction identical to a native compact overlay.
pub fn begin_window_drag(hwnd: HWND) {
    unsafe {
        let _ = ReleaseCapture();
        let _ = SendMessageW(
            hwnd,
            WM_NCLBUTTONDOWN,
            WPARAM(HTCAPTION as usize),
            LPARAM(0),
        );
    }
}

/// Opens the native Windows folder picker used for the capture/record destination.
#[cfg(target_os = "windows")]
pub fn pick_folder(owner: Option<isize>) -> Option<std::path::PathBuf> {
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::OsStringExt;
    use std::ptr;

    #[repr(C)]
    struct BrowseInfoW {
        hwnd_owner: isize,
        pidl_root: *mut c_void,
        display_name: *mut u16,
        title: *const u16,
        flags: u32,
        callback: Option<unsafe extern "system" fn(isize, u32, isize, isize) -> i32>,
        lparam: isize,
        image: i32,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHBrowseForFolderW(info: *const BrowseInfoW) -> *mut c_void;
        fn SHGetPathFromIDListW(item: *const c_void, path: *mut u16) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(memory: *mut c_void);
    }

    // BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE
    let title: Vec<u16> = "Select capture folder\0".encode_utf16().collect();
    let mut display_name = [0u16; 260];
    let info = BrowseInfoW {
        hwnd_owner: owner.unwrap_or(0),
        pidl_root: ptr::null_mut(),
        display_name: display_name.as_mut_ptr(),
        title: title.as_ptr(),
        flags: 0x0001 | 0x0040,
        callback: None,
        lparam: 0,
        image: 0,
    };

    let item = unsafe { SHBrowseForFolderW(&info) };
    if item.is_null() {
        return None;
    }

    let mut path = [0u16; 32_768];
    let success = unsafe { SHGetPathFromIDListW(item, path.as_mut_ptr()) != 0 };
    unsafe {
        CoTaskMemFree(item);
    }
    if !success {
        return None;
    }

    let length = path
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(path.len());
    if length == 0 {
        return None;
    }
    Some(std::path::PathBuf::from(OsString::from_wide(
        &path[..length],
    )))
}

#[cfg(not(target_os = "windows"))]
pub fn pick_folder(_owner: Option<isize>) -> Option<std::path::PathBuf> {
    None
}
