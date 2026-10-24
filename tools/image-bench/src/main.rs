#[path = "../../../src/image_processing.rs"]
mod image_processing;

fn main() {
    for (width, height) in [(3840, 2160), (1920, 80), (320, 160)] {
        // Deterministic fine detail exercises resizing and JPEG instead of compressing a blank image.
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            let noise = x
                .wrapping_mul(1664525)
                .wrapping_add(y.wrapping_mul(1013904223));
            image::Rgba([noise as u8, (noise >> 8) as u8, (noise >> 16) as u8, 255])
        });
        std::hint::black_box(image_processing::prepare_image(&image).unwrap());
        let mut samples = Vec::new();
        for _ in 0..11 {
            let started = std::time::Instant::now();
            std::hint::black_box(
                image_processing::prepare_image(std::hint::black_box(&image)).unwrap(),
            );
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{width}x{height}: median {:.3} ms",
            samples[samples.len() / 2]
        );
    }
}
