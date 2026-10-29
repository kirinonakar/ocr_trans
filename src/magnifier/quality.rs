//! Original-resolution window capture and one-pass, alpha-aware Lanczos scaling.
//! Only a single latest result is retained; the UI never waits for capture/resizing.
use anyhow::{Context, Result};
use fast_image_resize::{
    images::{Image, ImageRef},
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
};
use image::RgbaImage;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

mod popups;
mod window_capture;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FrameKey {
    pub source: (u32, u32),
    pub output: (u32, u32),
}

pub(super) struct ProcessedFrame {
    pub key: FrameKey,
    pub pixels: RgbaImage,
}

#[derive(Default)]
struct Shared {
    output: Option<(u32, u32)>,
    source: Option<(u32, u32)>,
    latest: Option<ProcessedFrame>,
    error: Option<String>,
}

pub(super) struct QualitySession {
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
}

impl QualitySession {
    pub fn start(source: isize) -> Result<Self> {
        let this = Self {
            stop: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Mutex::new(Shared::default())),
        };
        let stop = this.stop.clone();
        let shared = this.shared.clone();
        std::thread::Builder::new()
            .name("magnifier-quality".into())
            .spawn(move || {
                if let Err(error) = run(source, &stop, &shared) {
                    log::warn!("High-quality window preview unavailable; using DWM: {error:#}");
                    shared.lock().unwrap().error = Some(error.to_string());
                }
            })
            .context("Unable to start window preview worker")?;
        Ok(this)
    }

    pub fn source_size(&self) -> Option<(u32, u32)> {
        self.shared.lock().unwrap().source
    }

    pub fn request(&self, output: Option<(u32, u32)>) {
        self.shared.lock().unwrap().output = output;
    }

    pub fn take_frame(&self) -> Option<ProcessedFrame> {
        self.shared.lock().unwrap().latest.take()
    }

    pub fn failed(&self) -> bool {
        self.shared.lock().unwrap().error.is_some()
    }
}

impl Drop for QualitySession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(source: isize, stop: &AtomicBool, shared: &Mutex<Shared>) -> Result<()> {
    let _apartment = window_capture::Apartment::new()?;
    let mut capture = window_capture::WindowCapture::new(source)?;
    let mut popups = popups::Popups::new(source);
    let mut resizer = Resizer::new();
    let mut original: Option<RgbaImage> = None;
    let mut rendered: Option<FrameKey> = None;
    let mut composed: Option<RgbaImage> = None;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        if shared.lock().unwrap().output.is_none() {
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }
        if let Some(frame) = capture.next_frame()? {
            shared.lock().unwrap().source = Some(frame.dimensions());
            // Static windows often produce duplicate frames. Keep the last original
            // and avoid convolution/upload when neither pixels nor zoom has changed.
            if original.as_ref() != Some(&frame) {
                original = Some(frame);
                rendered = None;
            }
        }
        let output = shared.lock().unwrap().output;
        if let Some(original) = &original {
            let pixels = popups.composite(original);
            if composed != pixels {
                composed = pixels;
                rendered = None;
            }
        }
        if let (Some(original), Some(output)) = (composed.as_ref().or(original.as_ref()), output) {
            let key = FrameKey {
                source: original.dimensions(),
                output,
            };
            if rendered != Some(key) {
                let pixels = resize_original(&mut resizer, original, output)?;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                // A newer slider position supersedes work still in flight.
                let mut state = shared.lock().unwrap();
                if state.output == Some(output) {
                    state.latest = Some(ProcessedFrame { key, pixels });
                    rendered = Some(key);
                }
            }
        }
        if let Some(delay) = Duration::from_millis(16).checked_sub(started.elapsed()) {
            std::thread::sleep(delay);
        }
    }
    Ok(())
}

pub(super) fn resize_original(
    resizer: &mut Resizer,
    original: &RgbaImage,
    output: (u32, u32),
) -> Result<RgbaImage> {
    anyhow::ensure!(output.0 > 0 && output.1 > 0, "Invalid preview size");
    // Exact 100% preserves every original pixel, including font antialiasing.
    if original.dimensions() == output {
        return Ok(original.clone());
    }
    let source = ImageRef::new(
        original.width(),
        original.height(),
        original.as_raw(),
        PixelType::U8x4,
    )?;
    let mut destination = Image::new(output.0, output.1, PixelType::U8x4);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    // The resizer premultiplies alpha before filtering and unpremultiplies after,
    // keeping transparent window edges from acquiring a dark/colored halo.
    resizer.resize(&source, &mut destination, &options)?;
    RgbaImage::from_raw(output.0, output.1, destination.into_vec()).context("Invalid resized frame")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn one_to_one_keeps_text_pixels_byte_exact() {
        let source = RgbaImage::from_fn(17, 11, |x, y| {
            Rgba([(x * 13) as u8, (y * 21) as u8, 73, 255])
        });
        assert_eq!(
            resize_original(&mut Resizer::new(), &source, source.dimensions()).unwrap(),
            source
        );
    }

    #[test]
    fn fractional_enlargement_is_smooth_but_retains_stroke_contrast() {
        let source = RgbaImage::from_fn(40, 30, |x, y| {
            let ink = (10..14).contains(&x) || ((10..25).contains(&x) && (20..24).contains(&y));
            Rgba([if ink { 0 } else { 255 }; 4])
        });
        let mut source = source;
        for pixel in source.pixels_mut() {
            pixel[3] = 255;
        }
        let scaled = resize_original(&mut Resizer::new(), &source, (62, 47)).unwrap();
        assert!(scaled.pixels().any(|pixel| pixel[0] > 10 && pixel[0] < 245));
        assert!(scaled.pixels().any(|pixel| pixel[0] < 10));
        assert!(scaled.pixels().any(|pixel| pixel[0] > 245));
        for pixel in scaled.pixels() {
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            assert_eq!(pixel[3], 255);
        }
    }

    #[test]
    fn reduction_antialiases_thin_strokes_without_moire() {
        let source = RgbaImage::from_fn(120, 80, |x, _| {
            let value = if x % 2 == 0 { 0 } else { 255 };
            Rgba([value, value, value, 255])
        });
        let scaled = resize_original(&mut Resizer::new(), &source, (36, 24)).unwrap();
        for x in 4..32 {
            assert!((115..=140).contains(&scaled.get_pixel(x, 12)[0]));
        }
    }

    #[test]
    fn transparent_edges_do_not_bleed_hidden_rgb_into_text() {
        let source = RgbaImage::from_fn(20, 20, |x, _| {
            if x < 10 {
                Rgba([255, 0, 0, 0])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let scaled = resize_original(&mut Resizer::new(), &source, (31, 31)).unwrap();
        for pixel in scaled
            .pixels()
            .filter(|pixel| pixel[3] > 16 && pixel[3] < 239)
        {
            assert!(pixel[0].abs_diff(pixel[1]) <= 2 && pixel[1].abs_diff(pixel[2]) <= 2);
        }
    }
}
