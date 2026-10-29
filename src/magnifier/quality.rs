//! Event-driven, worker-owned capture and alpha-aware Lanczos scaling.
//! Only a single latest result and one pending UI notification are retained.
use anyhow::{Context, Result};
use fast_image_resize::{
    images::{Image, ImageRef},
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
};
use image::RgbaImage;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::Duration,
};

mod popups;
mod window_capture;

type PixelBuffer = slint::SharedPixelBuffer<slint::Rgba8Pixel>;

/// A latched wakeup: events arriving during processing must not be lost.
#[derive(Default)]
pub(super) struct Wake {
    pending: Mutex<bool>,
    ready: Condvar,
}
impl Wake {
    pub fn signal(&self) {
        *self.pending.lock().unwrap() = true;
        self.ready.notify_one();
    }
    fn wait(&self) {
        let pending = self.pending.lock().unwrap();
        // The timeout only maintains popup discovery/device health on static windows.
        let (mut pending, _) = self
            .ready
            .wait_timeout_while(pending, Duration::from_millis(100), |pending| !*pending)
            .unwrap();
        *pending = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FrameKey {
    pub source: (u32, u32),
    pub output: (u32, u32),
}

pub(super) struct ProcessedFrame {
    pub key: FrameKey,
    pub buffer: PixelBuffer,
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
    wake: Arc<Wake>,
}

impl QualitySession {
    pub fn start(source: isize, window: slint::Weak<crate::MagnifierWindow>) -> Result<Self> {
        let this = Self {
            stop: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Mutex::new(Shared::default())),
            wake: Arc::new(Wake::default()),
        };
        let stop = this.stop.clone();
        let shared = this.shared.clone();
        let wake = this.wake.clone();
        std::thread::Builder::new()
            .name("magnifier-quality".into())
            .spawn(move || {
                let queued = Arc::new(AtomicBool::new(false));
                let notify = || {
                    if queued.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let queued = queued.clone();
                    let stop = stop.clone();
                    let _ = window.upgrade_in_event_loop(move |_| {
                        queued.store(false, Ordering::Release);
                        // An old session must never publish into a reopened viewer.
                        if !stop.load(Ordering::Acquire) {
                            super::input::present_frame();
                        }
                    });
                };
                if let Err(error) = run(source, &stop, &shared, wake, &notify) {
                    log::warn!("High-quality window preview unavailable; using DWM: {error:#}");
                    shared.lock().unwrap().error = Some(error.to_string());
                    notify();
                }
            })
            .context("Unable to start window preview worker")?;
        Ok(this)
    }

    pub fn source_size(&self) -> Option<(u32, u32)> {
        self.shared.lock().unwrap().source
    }

    pub fn request(&self, output: Option<(u32, u32)>) {
        let mut state = self.shared.lock().unwrap();
        if state.output != output {
            state.output = output;
            drop(state);
            self.wake.signal();
        }
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
        self.stop.store(true, Ordering::Release);
        self.wake.signal();
    }
}

fn resize_pool() -> Result<rayon::ThreadPool> {
    // Leave capacity for the source application, input hooks and compositor.
    let threads =
        std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).clamp(1, 4));
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|n| format!("magnifier-resize-{n}"))
        .build()
        .context("Unable to start preview resize pool")
}

fn run(
    source: isize,
    stop: &AtomicBool,
    shared: &Mutex<Shared>,
    wake: Arc<Wake>,
    notify: &impl Fn(),
) -> Result<()> {
    let _apartment = window_capture::Apartment::new()?;
    let mut capture = window_capture::WindowCapture::new(source, wake.clone())?;
    let mut popups = popups::Popups::new(source);
    let pool = resize_pool()?;
    let mut resizer = Resizer::new();
    let mut original: Option<RgbaImage> = None;
    let mut rendered: Option<FrameKey> = None;
    let mut composed: Option<RgbaImage> = None;
    while !stop.load(Ordering::Acquire) {
        if shared.lock().unwrap().output.is_none() {
            wake.wait();
            continue;
        }
        let mut source_changed = false;
        if let Some(frame) = capture.next_frame()? {
            let size = frame.dimensions();
            let changed_size = shared.lock().unwrap().source.replace(size) != Some(size);
            if changed_size {
                notify();
            }
            // Static windows can deliver duplicates. Avoid convolution/upload.
            if original.as_ref() != Some(&frame) {
                original = Some(frame);
                rendered = None;
                source_changed = true;
            }
        }
        let output = shared.lock().unwrap().output;
        if let Some(original) = &original {
            match popups.composite(original, &capture, source_changed) {
                popups::CompositeUpdate::Unchanged => {}
                popups::CompositeUpdate::Clear => {
                    if composed.take().is_some() {
                        rendered = None;
                    }
                }
                popups::CompositeUpdate::Frame(pixels) => {
                    composed = Some(pixels);
                    rendered = None;
                }
            }
        }
        if let (Some(original), Some(output)) = (composed.as_ref().or(original.as_ref()), output) {
            let key = FrameKey {
                source: original.dimensions(),
                output,
            };
            if rendered != Some(key) {
                let buffer = pool.install(|| resize_buffer(&mut resizer, original, output))?;
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let mut state = shared.lock().unwrap();
                if state.output == Some(output) {
                    state.latest = Some(ProcessedFrame { key, buffer });
                    rendered = Some(key);
                    drop(state);
                    notify();
                }
            }
        }
        wake.wait();
    }
    Ok(())
}

fn resize_buffer(
    resizer: &mut Resizer,
    original: &RgbaImage,
    output: (u32, u32),
) -> Result<PixelBuffer> {
    anyhow::ensure!(output.0 > 0 && output.1 > 0, "Invalid preview size");
    let mut buffer = PixelBuffer::new(output.0, output.1);
    // 100% does exactly one copy into the same shared storage used for display/save.
    if original.dimensions() == output {
        buffer.make_mut_bytes().copy_from_slice(original.as_raw());
        return Ok(buffer);
    }
    let source = ImageRef::new(
        original.width(),
        original.height(),
        original.as_raw(),
        PixelType::U8x4,
    )?;
    let mut destination =
        Image::from_slice_u8(output.0, output.1, buffer.make_mut_bytes(), PixelType::U8x4)?;
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    // Alpha-aware filtering prevents colored halos on transparent edges.
    resizer.resize(&source, &mut destination, &options)?;
    Ok(buffer)
}

#[cfg(test)]
pub(super) fn resize_original(
    resizer: &mut Resizer,
    original: &RgbaImage,
    output: (u32, u32),
) -> Result<RgbaImage> {
    let buffer = resize_buffer(resizer, original, output)?;
    RgbaImage::from_raw(output.0, output.1, buffer.as_bytes().to_vec())
        .context("Invalid resized frame")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::time::Instant;

    #[test]
    fn repeated_frames_keep_the_same_quality_after_motion() {
        let source = RgbaImage::from_fn(37, 21, |x, y| {
            Rgba([if (x + y) % 3 == 0 { 0 } else { 255 }, 60, 90, 255])
        });
        let mut resizer = Resizer::new();
        let expected = resize_original(&mut resizer, &source, (63, 36)).unwrap();
        for shift in 1..=8 {
            let moving = RgbaImage::from_fn(37, 21, |x, y| *source.get_pixel((x + shift) % 37, y));
            resize_original(&mut resizer, &moving, (63, 36)).unwrap();
            // Returning to the same content must not change sharpness, even
            // after the resizer processed motion or another output size.
            resize_original(&mut resizer, &moving, (74, 42)).unwrap();
            assert_eq!(
                resize_original(&mut resizer, &source, (63, 36)).unwrap(),
                expected
            );
        }
    }

    #[test]
    #[ignore = "Measures CPU Lanczos resizing on synthetic frames; does not operate desktop windows"]
    fn measure_high_quality_preview() {
        for (source_size, output) in [
            ((800, 600), (1600, 1200)),
            ((1920, 1080), (2560, 1440)),
            ((3840, 2160), (1920, 1080)),
        ] {
            let original = RgbaImage::from_fn(source_size.0, source_size.1, |x, y| {
                let value = if (x / 3 + y / 17) % 7 == 0 { 30 } else { 245 };
                Rgba([value, value, value, 255])
            });
            for threads in [1, resize_pool().unwrap().current_num_threads()] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                let mut resizer = Resizer::new();
                pool.install(|| resize_buffer(&mut resizer, &original, output))
                    .unwrap();
                let mut timings = Vec::new();
                for _ in 0..9 {
                    let started = Instant::now();
                    std::hint::black_box(
                        pool.install(|| resize_buffer(&mut resizer, &original, output))
                            .unwrap(),
                    );
                    timings.push(started.elapsed());
                }
                timings.sort();
                println!(
                    "preview {:?}->{:?}: Lanczos ({threads} threads, shared buffer) {:.3} ms",
                    source_size,
                    output,
                    timings[timings.len() / 2].as_secs_f64() * 1000.0
                );
            }
        }
    }

    #[test]
    fn parallel_filter_matches_serial_and_shared_snapshot_survives_next_frame() {
        let serial = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let parallel = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let original = RgbaImage::from_fn(257, 193, |x, y| {
            Rgba([(x % 251) as u8, (y % 253) as u8, 77, ((x + y) % 256) as u8])
        });
        let expected = serial
            .install(|| resize_buffer(&mut Resizer::new(), &original, (521, 389)))
            .unwrap();
        let actual = parallel
            .install(|| resize_buffer(&mut Resizer::new(), &original, (521, 389)))
            .unwrap();
        assert_eq!(actual.as_bytes(), expected.as_bytes());
        let saved = actual.clone();
        assert_eq!(saved.as_bytes().as_ptr(), actual.as_bytes().as_ptr());
        let next = RgbaImage::from_pixel(257, 193, Rgba([10, 20, 30, 255]));
        parallel
            .install(|| resize_buffer(&mut Resizer::new(), &next, (521, 389)))
            .unwrap();
        assert_eq!(saved.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn output_changes_and_stop_wake_the_worker_without_losing_early_signals() {
        let session = QualitySession {
            stop: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Mutex::new(Shared::default())),
            wake: Arc::new(Wake::default()),
        };
        session.request(Some((80, 60)));
        assert!(*session.wake.pending.lock().unwrap());
        session.wake.wait(); // Signal arrived before wait; must be consumed.
        assert!(!*session.wake.pending.lock().unwrap());
        session.request(Some((80, 60)));
        assert!(!*session.wake.pending.lock().unwrap());
        session.request(None);
        assert!(*session.wake.pending.lock().unwrap());
        session.wake.wait();
        let wake = session.wake.clone();
        let stop = session.stop.clone();
        drop(session);
        assert!(stop.load(Ordering::Acquire));
        assert!(*wake.pending.lock().unwrap());
    }

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
        {
            let scaled = resize_original(&mut Resizer::new(), &source, (31, 31)).unwrap();
            for pixel in scaled
                .pixels()
                .filter(|pixel| pixel[3] > 16 && pixel[3] < 239)
            {
                assert!(pixel[0].abs_diff(pixel[1]) <= 2 && pixel[1].abs_diff(pixel[2]) <= 2);
            }
        }
    }
}
