//! About window opened from the OCR mode heading.

use crate::{AboutWindow, MainWindow};
use i_slint_backend_winit::WinitWindowAccessor;
use slint::ComponentHandle;
use std::time::Duration;

pub(crate) fn register_callbacks(main: &MainWindow, about: &AboutWindow) {
    let main_weak = main.as_weak();
    let about_weak = about.as_weak();
    main.on_about_clicked(move || {
        let main_weak = main_weak.clone();
        let about_weak = about_weak.clone();
        // Create/activate native windows after Slint has finished dispatching the click.
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            let (Some(main), Some(about)) = (main_weak.upgrade(), about_weak.upgrade()) else {
                return;
            };
            about.set_dark_theme(main.get_dark_theme());
            if let Err(error) = about.show() {
                log::error!("Unable to show About window: {error}");
                return;
            }
            about.window().with_winit_window(|native| {
                native.set_minimized(false);
                native.focus_window();
                #[cfg(target_os = "windows")]
                {
                    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = native.window_handle() {
                        if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                            let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as _);
                            crate::win_utils::set_title_bar_theme(hwnd, about.get_dark_theme());
                            unsafe {
                                let _ =
                                    windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                                        hwnd,
                                    );
                            }
                        }
                    }
                }
            });
        });
    });

    let about_weak = about.as_weak();
    about.on_close_clicked(move || {
        let about_weak = about_weak.clone();
        slint::Timer::single_shot(Duration::from_millis(1), move || {
            if let Some(about) = about_weak.upgrade() {
                let _ = about.hide();
            }
        });
    });
    about
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
}
