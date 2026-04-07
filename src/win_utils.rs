use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SetWindowLongW, SetLayeredWindowAttributes, GWL_EXSTYLE, WS_EX_LAYERED, WS_EX_TRANSPARENT, LWA_ALPHA,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW,
};

/// Sets the window to be click-through by applying WS_EX_TRANSPARENT and WS_EX_LAYERED styles.
pub fn set_click_through(hwnd: HWND, enable: bool) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        if enable {
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as i32);
            let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
        } else {
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style & !(WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as i32);
        }
    }
}

/// Sets the window to be a tool window (hides from taskbar) and non-activatable.
pub fn set_tool_window(hwnd: HWND) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{WS_EX_TOOLWINDOW, WS_EX_NOACTIVATE};
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        // WS_EX_TOOLWINDOW hides from taskbar
        // WS_EX_NOACTIVATE prevents it from taking focus when shown
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as i32);
    }
}

/// Sets the window to be layered (essential for alpha transparency on Windows).
pub fn set_layered(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED.0 as i32);
        let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
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
