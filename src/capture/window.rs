//! Window hit testing, frame cropping, and native window shape masks.
use super::{capture_area, CaptureRect, WindowTarget};
use anyhow::Result;
use image::{Rgba, RgbaImage};

/// Returns the top-level external window under a physical screen point.
/// The current process is ignored so the selector/toolbar cannot select itself.
#[cfg(target_os = "windows")]
pub fn window_target_at_point(x: i32, y: i32) -> Option<WindowTarget> {
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
            parameter: isize,
        ) -> i32;
        fn GetWindowRect(window: isize, rect: *mut Rect) -> i32;
        fn IsWindowVisible(window: isize) -> i32;
        fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmGetWindowAttribute(
            window: isize,
            attribute: u32,
            value: *mut Rect,
            value_size: u32,
        ) -> i32;
    }

    fn visible_window_rect(window: isize, fallback: Rect) -> Rect {
        // DWMWA_EXTENDED_FRAME_BOUNDS (9) excludes the invisible resize border that
        // GetWindowRect includes on modern Windows. That border was the source of the margin
        // around window captures. Keep GetWindowRect as a fallback for classic/non-DWM windows.
        let mut visible = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let result = unsafe {
            DwmGetWindowAttribute(window, 9, &mut visible, std::mem::size_of::<Rect>() as u32)
        };
        if result == 0 && visible.right > visible.left && visible.bottom > visible.top {
            visible
        } else {
            fallback
        }
    }

    #[repr(C)]
    struct SearchContext {
        x: i32,
        y: i32,
        current_process: u32,
        window: isize,
        rect: Rect,
    }

    unsafe extern "system" fn enum_window_proc(window: isize, parameter: isize) -> i32 {
        let context = &mut *(parameter as *mut SearchContext);
        if window == 0 || IsWindowVisible(window) == 0 {
            return 1;
        }
        let mut process_id = 0u32;
        GetWindowThreadProcessId(window, &mut process_id);
        if process_id == context.current_process {
            return 1;
        }
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(window, &mut rect) == 0 {
            return 1;
        }
        rect = visible_window_rect(window, rect);
        if context.x >= rect.left
            && context.x < rect.right
            && context.y >= rect.top
            && context.y < rect.bottom
            && rect.right - rect.left > 1
            && rect.bottom - rect.top > 1
        {
            context.window = window;
            context.rect = rect;
            return 0;
        }
        1
    }

    let mut context = SearchContext {
        x,
        y,
        current_process: std::process::id(),
        window: 0,
        rect: Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    };
    unsafe {
        EnumWindows(
            Some(enum_window_proc),
            &mut context as *mut SearchContext as isize,
        );
    }
    if context.window == 0 {
        return None;
    }
    let bounds = CaptureRect {
        x: context.rect.left,
        y: context.rect.top,
        width: context.rect.right - context.rect.left,
        height: context.rect.bottom - context.rect.top,
    };
    bounds.valid().then_some(WindowTarget {
        bounds,
        handle: context.window,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn window_target_at_point(_x: i32, _y: i32) -> Option<WindowTarget> {
    None
}

const WINDOW_CAPTURE_MARGIN: u32 = 3;

pub fn capture_window(target: WindowTarget) -> Result<RgbaImage> {
    let mut image = capture_area(&target.bounds, &None)?;
    #[cfg(target_os = "windows")]
    apply_window_shape(&mut image, target);
    let crop_width = image.width().saturating_sub(WINDOW_CAPTURE_MARGIN * 2);
    let crop_height = image.height().saturating_sub(WINDOW_CAPTURE_MARGIN * 2);
    if crop_width == 0 || crop_height == 0 {
        anyhow::bail!("The selected window is too small to crop");
    }
    Ok(image::imageops::crop_imm(
        &image,
        WINDOW_CAPTURE_MARGIN,
        WINDOW_CAPTURE_MARGIN,
        crop_width,
        crop_height,
    )
    .to_image())
}

/// Mask before cropping so the existing frame inset does not shift the corner centers.
/// Use the full window dimensions even when capture_area clips at a monitor boundary.
fn mask_rounded_corners(image: &mut RgbaImage, width: u32, height: u32, radius: f64) {
    let radius = radius.min(width as f64 / 2.0).min(height as f64 / 2.0);
    if radius <= 0.0 {
        return;
    }
    let inner_radius = (radius - WINDOW_CAPTURE_MARGIN as f64).max(0.0);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let px = x as f64 + 0.5;
        let py = y as f64 + 0.5;
        let dx = (radius - px.min(width as f64 - px)).max(0.0);
        let dy = (radius - py.min(height as f64 - py)).max(0.0);
        if dx == 0.0 || dy == 0.0 {
            continue;
        }
        // Screen pixels already contain the DWM border and its background-blended edge.
        // Merely reducing their alpha leaves a grey halo. Inset the curved edge by the
        // same amount as the straight frame crop, retaining the original corner centers.
        let coverage = (inner_radius + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0);
        pixel[3] = (pixel[3] as f64 * coverage).round() as u8;
        if pixel[3] == 0 {
            *pixel = Rgba([0, 0, 0, 0]);
        }
    }
}

#[cfg(target_os = "windows")]
fn apply_window_shape(image: &mut RgbaImage, target: WindowTarget) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE};
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, GetWindowRgn, PtInRegion};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, GetWindowRect, IsZoomed, GWL_STYLE, WS_CAPTION, WS_THICKFRAME,
    };

    #[link(name = "user32")]
    extern "system" {
        fn GetDpiForWindow(window: isize) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *const std::ffi::c_void;
    }

    let window = HWND(target.handle as *mut std::ffi::c_void);
    if target.handle == 0 {
        return;
    }
    unsafe {
        // Explicit regions describe custom-shaped windows in GetWindowRect coordinates,
        // including its invisible resize border. DWM's rounded corners are not in this region.
        let region = CreateRectRgn(0, 0, 0, 0);
        if !region.0.is_null() {
            let kind = GetWindowRgn(window, region);
            let mut rect = RECT::default();
            let has_region = kind.0 != 0 && GetWindowRect(window, &mut rect).is_ok();
            if has_region {
                for (x, y, pixel) in image.enumerate_pixels_mut() {
                    if !PtInRegion(
                        region,
                        target.bounds.x - rect.left + x as i32,
                        target.bounds.y - rect.top + y as i32,
                    )
                    .as_bool()
                    {
                        *pixel = Rgba([0, 0, 0, 0]);
                    }
                }
            }
            let _ = DeleteObject(region);
            if has_region {
                return;
            }
        }

        let mut preference = 0u32;
        if DwmGetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &mut preference as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of_val(&preference) as u32,
        )
        .is_err()
            || preference == 1 // DWMWCP_DONOTROUND; also leave pre-Windows 11 unchanged.
            || IsZoomed(window).as_bool()
        {
            return;
        }
        // Resolve dynamically: older User32 versions do not export IsWindowArranged.
        let user32: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let module = GetModuleHandleW(user32.as_ptr());
        if module != 0 {
            let address = GetProcAddress(module, b"IsWindowArranged\0".as_ptr());
            if !address.is_null() {
                let is_arranged: unsafe extern "system" fn(isize) -> i32 =
                    std::mem::transmute(address);
                if is_arranged(target.handle) != 0 {
                    return;
                }
            }
        }
        let style = GetWindowLongW(window, GWL_STYLE) as u32;
        if preference == 0 && style & (WS_CAPTION.0 | WS_THICKFRAME.0) == 0 {
            // Borderless windows need an explicit opt-in; default is only a DWM heuristic.
            return;
        }
        let logical_radius = match preference {
            0 | 2 => 8.0, // DWMWCP_DEFAULT / DWMWCP_ROUND
            3 => 4.0,     // DWMWCP_ROUNDSMALL
            _ => return,
        };
        let dpi = GetDpiForWindow(target.handle);
        let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
        mask_rounded_corners(
            image,
            target.bounds.width as u32,
            target.bounds.height as u32,
            logical_radius * scale,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{mask_rounded_corners, WINDOW_CAPTURE_MARGIN};
    use image::{Rgba, RgbaImage};

    #[test]
    fn window_corners_are_transparent_after_frame_crop_and_png_roundtrip() {
        let color = Rgba([40, 80, 120, 255]);
        let mut image = RgbaImage::from_pixel(40, 30, color);
        mask_rounded_corners(&mut image, 40, 30, 12.0);
        let margin = WINDOW_CAPTURE_MARGIN;
        let cropped =
            image::imageops::crop_imm(&image, margin, margin, 40 - margin * 2, 30 - margin * 2)
                .to_image();
        let mut png = std::io::Cursor::new(Vec::new());
        cropped.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let decoded = image::load_from_memory(png.get_ref()).unwrap().to_rgba8();
        let right = decoded.width() - 1;
        let bottom = decoded.height() - 1;
        for (x, y) in [(0, 0), (right, 0), (0, bottom), (right, bottom)] {
            assert_eq!(*decoded.get_pixel(x, y), Rgba([0, 0, 0, 0]));
        }
        assert_eq!(*decoded.get_pixel(18, 0), color);
        assert_eq!(*decoded.get_pixel(0, 13), color);
        assert_eq!(*decoded.get_pixel(18, 13), color);
        assert!(decoded.pixels().any(|p| p[3] > 0 && p[3] < 255));
    }

    #[test]
    fn window_mask_removes_opaque_border_along_the_curve() {
        let fill = Rgba([232, 232, 232, 255]);
        let mut image = RgbaImage::from_pixel(100, 80, fill);
        // Border locations observed in the Explorer capture at a 14px outer radius.
        let border = [(8, 3), (7, 4), (5, 5), (3, 8)];
        for (x, y) in border {
            for (x, y) in [(x, y), (99 - x, y), (x, 79 - y), (99 - x, 79 - y)] {
                image.put_pixel(x, y, Rgba([211, 211, 211, 255]));
            }
        }
        mask_rounded_corners(&mut image, 100, 80, 14.0);
        assert!(image.pixels().all(|p| p[3] == 0 || p.0[..3] == fill.0[..3]));
        assert!(image.get_pixel(9, 3)[3] > 0);
        assert_eq!(*image.get_pixel(50, 3), fill);
        assert_eq!(*image.get_pixel(3, 40), fill);
    }

    #[test]
    fn window_mask_respects_full_bounds_when_monitor_clips_capture() {
        let color = Rgba([40, 80, 120, 128]);
        let mut image = RgbaImage::from_pixel(20, 15, color);
        mask_rounded_corners(&mut image, 40, 30, 8.0);
        assert_eq!(*image.get_pixel(19, 0), color);
        assert_eq!(*image.get_pixel(0, 14), color);
        assert_eq!(*image.get_pixel(19, 14), color);
        assert_eq!(image.get_pixel(0, 0)[3], 0);
        assert!(image.pixels().all(|p| p[3] <= 128));
    }
}
