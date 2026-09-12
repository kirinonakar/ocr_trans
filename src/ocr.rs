use anyhow::{Context, Result};

pub(crate) const LANGUAGE_OPTIONS: &[(&str, &str)] = &[
    ("", "OCR: Windows"),
    ("ko-KR", "OCR: 한국어"),
    ("ja-JP", "OCR: 日本語"),
    ("en-US", "OCR: English"),
    ("zh-Hans", "OCR: 简体中文"),
    ("zh-Hant", "OCR: 繁體中文"),
];

pub(crate) fn language_label(tag: &str) -> &str {
    LANGUAGE_OPTIONS
        .iter()
        .find(|(code, _)| *code == tag)
        .map(|(_, label)| *label)
        .unwrap_or(tag)
}

pub(crate) fn language_tag(label: &str) -> &str {
    LANGUAGE_OPTIONS
        .iter()
        .find(|(_, name)| *name == label)
        .map(|(tag, _)| *tag)
        .unwrap_or(label)
}

/// An empty language tag uses Windows' preferred OCR language (not image language detection).
/// The public function is synchronous so callers can run it on a blocking worker thread and keep
/// Slint's UI responsive while Windows.Media.Ocr is processing the bitmap.
#[cfg(target_os = "windows")]
pub fn recognize_text(
    bgra_pixels: &[u8],
    width: u32,
    height: u32,
    language_tag: &str,
) -> Result<String> {
    use windows::core::HSTRING;
    use windows::Globalization::Language;
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
        // Never silently substitute another language: Japanese OCR can turn Hangul into
        // plausible-looking but incorrect characters. Windows profile selection is a default,
        // not automatic detection of the language in the image.
        let engine = if language_tag.trim().is_empty() {
            OcrEngine::TryCreateFromUserProfileLanguages().context(
                "No OCR pack matches your Windows languages. Select an OCR language and install its OCR pack in Windows Settings",
            )?
        } else {
            let tag = language_tag.trim();
            let language = Language::CreateLanguage(&HSTRING::from(tag))
                .with_context(|| format!("Invalid OCR language: {tag}"))?;
            OcrEngine::TryCreateFromLanguage(&language).with_context(|| {
                format!("OCR language {tag} is unavailable. Install its OCR language pack in Windows Settings")
            })?
        };
        if let Ok(language) = engine
            .RecognizerLanguage()
            .and_then(|lang| lang.LanguageTag())
        {
            log::debug!("Windows OCR language: {language}");
        }
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
pub fn recognize_text(
    _bgra_pixels: &[u8],
    _width: u32,
    _height: u32,
    _language_tag: &str,
) -> Result<String> {
    anyhow::bail!("Windows OCR is available on Windows only")
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    fn korean_fixture() -> (Vec<u8>, u32, u32) {
        let image = image::load_from_memory(include_bytes!("../tests/fixtures/korean.png"))
            .unwrap()
            .to_rgba8();
        let mut pixels = image.as_raw().clone();
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        (pixels, image.width(), image.height())
    }

    #[test]
    fn unavailable_language_does_not_fall_back_to_another_engine() {
        let error = recognize_text(&vec![255; 100 * 40 * 4], 100, 40, "zz-ZZ")
            .expect_err("An unavailable language must not silently use Japanese or the profile");
        assert!(error.to_string().contains("zz-ZZ"), "{error:#}");
    }

    #[test]
    #[ignore = "Requires the Windows Korean OCR language pack"]
    fn korean_image_preserves_hangul_and_line_breaks() {
        let (pixels, width, height) = korean_fixture();
        let text = recognize_text(&pixels, width, height, "ko-KR").unwrap();
        let compact = |value: &str| {
            value
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
        };
        assert_eq!(
            compact(&text),
            compact("한국어 문장을 정확하게 인식합니다.\n화면의 글자를 복사합니다."),
            "Recognized text: {text}"
        );
        assert_eq!(text.lines().count(), 2, "{text}");
    }

    #[test]
    #[ignore = "Requires a Windows OCR pack matching the user profile"]
    fn default_language_matches_windows_profile() {
        use windows::Media::Ocr::OcrEngine;
        use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

        // Keep the profile query's apartment alive until all its consumers have finished.
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { RoUninitialize() };
            }
        }
        unsafe { RoInitialize(RO_INIT_MULTITHREADED).unwrap() };
        let _apartment = Apartment;
        let profile = OcrEngine::TryCreateFromUserProfileLanguages()
            .and_then(|engine| engine.RecognizerLanguage())
            .and_then(|language| language.LanguageTag());
        let tag = profile.unwrap().to_string();
        let (pixels, width, height) = korean_fixture();
        assert_eq!(
            recognize_text(&pixels, width, height, "").unwrap(),
            recognize_text(&pixels, width, height, &tag).unwrap(),
            "Default OCR must use the Windows profile language {tag}"
        );
    }

    #[test]
    #[ignore = "Requires a Windows OCR pack matching the user profile"]
    fn default_language_can_be_used_repeatedly_on_a_worker() {
        let (pixels, width, height) = korean_fixture();
        let first = recognize_text(&pixels, width, height, "").unwrap();
        let second = recognize_text(&pixels, width, height, "").unwrap();
        assert_eq!(first, second);
    }
}
