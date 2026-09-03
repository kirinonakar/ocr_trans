use anyhow::{Context, Result};

/// Uses the Windows language-pack OCR engine, matching AIMediaWorker's OCR button behavior.
/// The public function is synchronous so callers can run it on a blocking worker thread and keep
/// Slint's UI responsive while Windows.Media.Ocr is processing the bitmap.
#[cfg(target_os = "windows")]
pub fn recognize_text(bgra_pixels: &[u8], width: u32, height: u32) -> Result<String> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Globalization::Language;
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    use windows::core::HSTRING;

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
        // Prefer the Japanese OCR pack when it is installed, then fall back to the user's
        // configured languages so English and other existing OCR workflows keep working.
        let japanese_language = Language::CreateLanguage(&HSTRING::from("ja-JP"));
        let engine = japanese_language
            .ok()
            .and_then(|language| OcrEngine::TryCreateFromLanguage(&language).ok())
            .or_else(|| OcrEngine::TryCreateFromUserProfileLanguages().ok())
            .context(
                "No Windows OCR language pack is available; install Japanese OCR support for ja-JP text",
            )?;
        let ocr_result = engine
            .RecognizeAsync(&bitmap)
            .context("Failed to start Windows OCR")?
            .get()
            .context("Windows OCR failed")?;
        let fallback_text = ocr_result
            .Text()
            .context("Windows OCR returned no text")?
            .to_string();

        // Build the result from OcrLine instead of relying only on OcrResult::Text. The line
        // collection carries the layout boundaries explicitly, so Japanese multi-line text is
        // returned with the same line breaks that the recognizer detected.
        let Ok(lines) = ocr_result.Lines() else {
            return Ok(fallback_text);
        };
        let Ok(line_count) = lines.Size() else {
            return Ok(fallback_text);
        };
        let mut text = String::new();
        for index in 0..line_count {
            let Ok(line) = lines.GetAt(index) else {
                continue;
            };
            let Ok(line_text) = line.Text() else {
                continue;
            };
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&line_text.to_string());
        }
        if text.is_empty() {
            Ok(fallback_text)
        } else {
            Ok(text)
        }
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
