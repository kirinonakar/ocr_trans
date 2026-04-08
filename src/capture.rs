use xcap::Monitor;
use image::{RgbaImage, GenericImageView};
use anyhow::{Result, Context};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn capture_area(rect: &CaptureRect, monitors: &Option<Vec<Monitor>>) -> Result<RgbaImage> {
    let local_monitors;
    let monitors_ref = if let Some(m) = monitors {
        m
    } else {
        local_monitors = Monitor::all().context("Failed to get monitors")?;
        &local_monitors
    };
    
    // Find monitor that contains the top-left point of the rect
    let monitor = monitors_ref.iter().find(|m| {
        rect.x >= m.x() && rect.x < m.x() + m.width() as i32 &&
        rect.y >= m.y() && rect.y < m.y() + m.height() as i32
    }).unwrap_or(&monitors_ref[0]);
    
    let img = monitor.capture_image().context("Failed to capture monitor")?;
    
    // Normalize coordinates relative to the monitor
    let local_x = (rect.x - monitor.x()).max(0) as u32;
    let local_y = (rect.y - monitor.y()).max(0) as u32;
    let w = (rect.width as u32).min(img.width() - local_x);
    let h = (rect.height as u32).min(img.height() - local_y);
    
    if w == 0 || h == 0 {
        return Ok(RgbaImage::new(1, 1));
    }

    let cropped = img.view(local_x, local_y, w, h).to_image();
    Ok(cropped)
}

/// Captures the full primary monitor.
pub fn capture_full_screen() -> Result<RgbaImage> {
    let monitors = Monitor::all().context("Failed to get monitors")?;
    if monitors.is_empty() {
        return Err(anyhow::anyhow!("No monitors found"));
    }
    // Using the first monitor as primary for selection
    let img = monitors[0].capture_image().context("Failed to capture monitor")?;
    Ok(img)
}

/// Comparison logic to check if the screen changed enough to trigger API.
pub fn is_changed(prev: &Option<RgbaImage>, curr: &RgbaImage, _threshold: f32) -> bool {
    let prev_img = match prev {
        Some(p) => p,
        None => return true,
    };
    
    if prev_img.dimensions() != curr.dimensions() {
        return true;
    }
    
    let mut diff_sum = 0u64;
    let mut total_pixels = 0u64;

    // Sample every 4th pixel (stride=2 in both x and y) to avoid burning CPU
    // on large captures every second. 25% sampling is more than enough for
    // detecting subtitle / dialog changes.
    let (width, height) = prev_img.dimensions();
    let stride = 2u32; // step every 2 pixels in each axis → ~25% sampled
    let mut y = 0u32;
    while y < height {
        let mut x = 0u32;
        while x < width {
            let p = prev_img.get_pixel(x, y);
            let c = curr.get_pixel(x, y);
            let diff = (p[0] as i32 - c[0] as i32).abs()
                + (p[1] as i32 - c[1] as i32).abs()
                + (p[2] as i32 - c[2] as i32).abs();
            if diff > 80 {
                diff_sum += 1;
            }
            total_pixels += 1;
            x += stride;
        }
        y += stride;
    }

    if total_pixels == 0 { return false; }
    (diff_sum as f32 / total_pixels as f32) >= 0.01
}
