use anyhow::{Context, Result};

pub(crate) fn image_dimensions(width: u32, height: u32) -> (u32, u32) {
    let pixels = width as u64 * height as u64;
    if pixels <= 1_000_000 {
        return (width, height);
    }
    let scale = (1_000_000.0 / pixels as f64).sqrt();
    (
        (width as f64 * scale).floor().max(1.0) as u32,
        (height as f64 * scale).floor().max(1.0) as u32,
    )
}

pub(crate) fn prepare_image(img: &image::RgbaImage) -> Result<Vec<u8>> {
    anyhow::ensure!(img.width() > 0 && img.height() > 0, "The image is empty");
    let (width, height) = image_dimensions(img.width(), img.height());
    let rgb = if (width, height) == img.dimensions() {
        image::DynamicImage::ImageRgba8(img.clone()).to_rgb8()
    } else {
        image::DynamicImage::ImageRgba8(image::imageops::resize(
            img,
            width,
            height,
            image::imageops::FilterType::Lanczos3,
        ))
        .to_rgb8()
    };
    let mut buffer = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, 92)
        .encode_image(&rgb)
        .context("Failed to encode image to JPEG")?;
    Ok(buffer)
}
