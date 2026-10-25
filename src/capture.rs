//! Capture entry points shared by the UI and background workers.
//! Implementations live in responsibility-specific modules under `capture/`.
mod change_detection;
mod desktop;
mod output;
mod recording;
mod scrolling;
mod window;

pub use change_detection::is_changed;
pub use desktop::{
    capture_area, capture_full_screen, capture_monitor_at_point, capture_virtual_desktop,
    cursor_position, monitor_rect_at_point, native_monitor_rect_at_point, sample_pixel_at_point,
    virtual_desktop_capture_rect,
};
pub use output::{
    copy_text_to_clipboard, output_directory, save_png_and_copy_to, unique_output_path_in,
};
pub use recording::ScreenRecorder;
pub use scrolling::scrolling_capture;
pub use window::{capture_window, window_target_at_point};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl CaptureRect {
    pub fn valid(self) -> bool {
        self.width > 1 && self.height > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowTarget {
    pub bounds: CaptureRect,
    /// Native HWND on Windows. It is zero on platforms without Win32.
    pub handle: isize,
}
