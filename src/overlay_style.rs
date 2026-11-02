use crate::{MainWindow, OverlayWindow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceColors {
    pub(crate) background: [u8; 3],
    pub(crate) text: [u8; 3],
}

#[derive(Clone, Copy, Default)]
struct ColorBin {
    count: u32,
    sum: [u64; 3],
}

impl ColorBin {
    fn color(self) -> [u8; 3] {
        self.sum.map(|sum| (sum / self.count as u64) as u8)
    }
}

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a as i32 - b as i32).pow(2) as u32)
        .sum()
}

/// Estimate one background/ink pair from the unmodified capture. Quantized bins merge
/// compression noise; contrast and population favor solid ink over antialiased edges
/// and isolated specks. Sampling is bounded without resizing away thin glyph strokes.
pub(crate) fn source_colors(image: &image::RgbaImage) -> Option<SourceColors> {
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    let mut bins = vec![ColorBin::default(); 4096];
    let step_x = image.width().div_ceil(256);
    let step_y = image.height().div_ceil(256);
    let mut samples = 0;
    for y in (0..image.height()).step_by(step_y as usize) {
        // Shift both axes to avoid repeatedly missing narrow glyph strokes.
        let offset = (y / step_y) % step_x;
        for x in (offset..image.width()).step_by(step_x as usize) {
            let sample_y = (y + (x / step_x) % step_y).min(image.height() - 1);
            let pixel = image.get_pixel(x, sample_y).0;
            if pixel[3] < 128 {
                continue;
            }
            let index = ((pixel[0] as usize >> 4) << 8)
                | ((pixel[1] as usize >> 4) << 4)
                | (pixel[2] as usize >> 4);
            let bin = &mut bins[index];
            bin.count += 1;
            for (sum, value) in bin.sum.iter_mut().zip(pixel) {
                *sum += value as u64;
            }
            samples += 1;
        }
    }
    let background = bins.iter().max_by_key(|bin| bin.count)?;
    if background.count == 0 {
        return None;
    }
    let background = background.color();
    let minimum_count = (samples / 1000).max(2);
    let text = bins
        .iter()
        .filter(|bin| bin.count >= minimum_count)
        .filter(|bin| distance(background, bin.color()) >= 30 * 30)
        .max_by(|a, b| {
            let score = |bin: &ColorBin| {
                distance(background, bin.color()) as f64 * (bin.count as f64).sqrt()
            };
            score(a).total_cmp(&score(b))
        })
        .map(|bin| bin.color())
        // A flat/empty source has no measurable ink. Keep its background and a readable fallback.
        .unwrap_or_else(|| {
            if background[0] as u32 * 299 + background[1] as u32 * 587 + background[2] as u32 * 114
                >= 128_000
            {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
    Some(SourceColors { background, text })
}

pub(crate) fn remember_source(overlay: &OverlayWindow, colors: SourceColors) {
    let [r, g, b] = colors.background;
    overlay.set_source_bg_color(slint::Color::from_rgb_u8(r, g, b));
    let [r, g, b] = colors.text;
    overlay.set_source_text_color(slint::Color::from_rgb_u8(r, g, b));
    overlay.set_has_source_colors(true);
}

/// Manual swatches remain intact so disabling Auto restores the user's previous style.
/// This deliberately touches only the overlay, never the result textbox.
pub(crate) fn apply(main: &MainWindow, overlay: &OverlayWindow) {
    let automatic = main.get_auto_style() && overlay.get_has_source_colors();
    overlay.set_bg_color(if automatic {
        overlay.get_source_bg_color()
    } else {
        main.get_overlay_bg_color()
    });
    overlay.set_text_color(if automatic {
        overlay.get_source_text_color()
    } else {
        main.get_overlay_text_color()
    });
    overlay.set_bg_opacity(if main.get_use_textbox() {
        0.1
    } else {
        main.get_overlay_bg_opacity()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn auto_updates_only_overlay_and_manual_colors_are_restored() {
        // Use a dedicated thread so this platform also works with serial test execution.
        std::thread::spawn(|| {
            use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
            use std::rc::Rc;

            struct Headless;
            impl slint::platform::Platform for Headless {
                fn create_window_adapter(
                    &self,
                ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError>
                {
                    Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
                }
            }
            slint::platform::set_platform(Box::new(Headless)).unwrap();
            let main = MainWindow::new().unwrap();
            let overlay = OverlayWindow::new().unwrap();
            let textbox = crate::TextboxWindow::new().unwrap();
            let manual_bg = slint::Color::from_rgb_u8(255, 240, 190);
            let manual_text = slint::Color::from_rgb_u8(180, 30, 50);
            main.set_overlay_bg_color(manual_bg);
            main.set_overlay_text_color(manual_text);
            main.set_overlay_bg_opacity(0.65);
            textbox.set_text_color(manual_text);
            main.set_auto_style(true);
            apply(&main, &overlay);
            // Auto before the first capture keeps the configured colors.
            assert_eq!(overlay.get_bg_color(), manual_bg);
            assert_eq!(overlay.get_bg_opacity(), 0.65);

            remember_source(
                &overlay,
                SourceColors {
                    background: [20, 24, 30],
                    text: [245, 245, 245],
                },
            );
            for dark in [false, true] {
                main.set_dark_theme(dark);
                textbox.set_dark_theme(dark);
                apply(&main, &overlay);
                assert_eq!(
                    overlay.get_bg_color(),
                    slint::Color::from_rgb_u8(20, 24, 30)
                );
                assert_eq!(
                    overlay.get_text_color(),
                    slint::Color::from_rgb_u8(245, 245, 245)
                );
                assert_eq!(overlay.get_bg_opacity(), 0.65);
                assert_eq!(textbox.get_text_color(), manual_text);
                assert_eq!(textbox.get_dark_theme(), dark);
                assert_eq!(main.get_overlay_bg_color(), manual_bg);
                assert_eq!(main.get_overlay_text_color(), manual_text);
            }
            main.set_use_textbox(true);
            apply(&main, &overlay);
            assert_eq!(overlay.get_bg_opacity(), 0.1);
            assert_eq!(textbox.get_text_color(), manual_text);
            main.set_use_textbox(false);
            apply(&main, &overlay);
            assert_eq!(overlay.get_bg_opacity(), 0.65);
            // Opacity stays adjustable while Auto continues to supply the colors.
            main.set_overlay_bg_opacity(0.35);
            apply(&main, &overlay);
            assert_eq!(overlay.get_bg_opacity(), 0.35);
            assert_eq!(
                overlay.get_bg_color(),
                slint::Color::from_rgb_u8(20, 24, 30)
            );
            assert_eq!(
                overlay.get_text_color(),
                slint::Color::from_rgb_u8(245, 245, 245)
            );
            main.set_auto_style(false);
            apply(&main, &overlay);
            assert_eq!(overlay.get_bg_color(), manual_bg);
            assert_eq!(overlay.get_text_color(), manual_text);
            assert_eq!(overlay.get_bg_opacity(), 0.35);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn detects_light_dark_and_colored_backgrounds_and_ink() {
        for (background, text) in [
            ([255, 255, 255], [0, 0, 0]),
            ([20, 24, 30], [245, 245, 245]),
            ([255, 240, 190], [180, 30, 50]),
            ([30, 60, 120], [255, 220, 40]),
        ] {
            let mut image = RgbaImage::from_pixel(
                200,
                80,
                Rgba([background[0], background[1], background[2], 255]),
            );
            for y in 20..60 {
                for x in (20..180).filter(|x| x % 10 < 3) {
                    image.put_pixel(x, y, Rgba([text[0], text[1], text[2], 255]));
                }
            }
            assert_eq!(
                source_colors(&image),
                Some(SourceColors { background, text })
            );
        }
    }

    #[test]
    fn solid_ink_beats_antialiasing_and_isolated_noise() {
        let image = RgbaImage::from_fn(200, 80, |x, y| {
            let color = if x == 0 && y == 0 {
                [0, 0, 0]
            } else if (20..60).contains(&y) && (20..180).contains(&x) && x % 10 < 2 {
                [180, 30, 50]
            } else if (20..60).contains(&y) && (20..180).contains(&x) && x % 10 < 5 {
                [220, 140, 150]
            } else {
                [255, 255, 255]
            };
            Rgba([color[0], color[1], color[2], 255])
        });
        assert_eq!(source_colors(&image).unwrap().text, [180, 30, 50]);
    }

    #[test]
    fn large_captures_keep_thin_strokes_and_empty_images_are_safe() {
        let image = RgbaImage::from_fn(2048, 1024, |x, y| {
            if (100..900).contains(&y) && x % 16 == 1 {
                Rgba([255, 220, 40, 255])
            } else {
                Rgba([30, 60, 120, 255])
            }
        });
        assert_eq!(source_colors(&image).unwrap().text, [255, 220, 40]);
        let image = RgbaImage::from_fn(1024, 2048, |_, y| {
            if y % 16 == 1 {
                Rgba([255, 220, 40, 255])
            } else {
                Rgba([30, 60, 120, 255])
            }
        });
        assert_eq!(source_colors(&image).unwrap().text, [255, 220, 40]);
        assert_eq!(source_colors(&RgbaImage::new(0, 0)), None);
        assert_eq!(source_colors(&RgbaImage::new(20, 20)), None);
        assert_eq!(
            source_colors(&RgbaImage::from_pixel(1, 1, Rgba([255; 4])))
                .unwrap()
                .text,
            [0, 0, 0]
        );
    }
}
