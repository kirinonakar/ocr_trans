use anyhow::{Context, Result};

/// Uses the Windows language-pack OCR engine, matching AIMediaWorker's OCR button behavior.
/// The public function is synchronous so callers can run it on a blocking worker thread and keep
/// Slint's UI responsive while Windows.Media.Ocr is processing the bitmap.
#[cfg(target_os = "windows")]
pub fn recognize_text(bgra_pixels: &[u8], width: u32, height: u32) -> Result<String> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

    if width == 0 || height == 0 || bgra_pixels.len() < (width as usize * height as usize * 4) {
        anyhow::bail!("The OCR image buffer is invalid");
    }

    let ro_initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED).is_ok() };
    let result = (|| {
        let buffer = CryptographicBuffer::CreateFromByteArray(bgra_pixels)
            .context("Failed to create the Windows OCR buffer")?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
        )
        .context("Failed to create the Windows OCR bitmap")?;
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .context("No Windows OCR language pack is available")?;
        let ocr_result = engine
            .RecognizeAsync(&bitmap)
            .context("Failed to start Windows OCR")?
            .get()
            .context("Windows OCR failed")?;
        Ok(ocr_result
            .Text()
            .context("Windows OCR returned no text")?
            .to_string())
    })();
    if ro_initialized {
        unsafe {
            RoUninitialize();
        }
    }
    result
}

#[cfg(not(target_os = "windows"))]
pub fn recognize_text(_bgra_pixels: &[u8], _width: u32, _height: u32) -> Result<String> {
    anyhow::bail!("Windows OCR is available on Windows only")
}
