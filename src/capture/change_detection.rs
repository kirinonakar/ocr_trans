//! Frame comparison used to decide when OCR/translation needs another pass.
use image::RgbaImage;

/// Comparison logic to check if the screen changed enough to trigger the original VLM worker.
pub fn is_changed(prev: &Option<RgbaImage>, curr: &RgbaImage, threshold: f32) -> bool {
    let prev_img = match prev {
        Some(p) => p,
        None => return true,
    };
    if prev_img.dimensions() != curr.dimensions() {
        return true;
    }

    let mut diff_sum = 0u64;
    let mut total_pixels = 0u64;
    let (width, height) = prev_img.dimensions();
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let p = prev_img.get_pixel(x, y);
            let c = curr.get_pixel(x, y);
            let diff = (p[0] as i32 - c[0] as i32).unsigned_abs()
                + (p[1] as i32 - c[1] as i32).unsigned_abs()
                + (p[2] as i32 - c[2] as i32).unsigned_abs();
            if diff > 80 {
                diff_sum += 1;
            }
            total_pixels += 1;
        }
    }
    total_pixels != 0 && diff_sum as f32 / total_pixels as f32 > threshold.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    #[test]
    fn pixel_change_threshold_is_respected() {
        let original = RgbaImage::from_pixel(20, 20, Rgba([0, 0, 0, 255]));
        let mut changed = original.clone();
        for y in 0..2 {
            for x in 0..20 {
                changed.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        assert!(super::is_changed(&Some(original.clone()), &changed, 0.05));
        assert!(!super::is_changed(&Some(original.clone()), &changed, 0.2));
        assert!(!super::is_changed(&Some(original.clone()), &original, 0.0));
        assert!(super::is_changed(&None, &original, 0.5));
    }
}
