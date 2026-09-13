use anyhow::{Context, Result};

#[derive(Debug)]
pub(crate) struct OcrLanguage {
    pub(crate) tag: String,
    pub(crate) label: String,
}

#[derive(Debug, Default)]
pub(crate) struct LanguageMenu {
    pub(crate) languages: Vec<OcrLanguage>,
    pub(crate) selected: Option<usize>,
}

fn compact_language_label(tag: &str) -> String {
    match tag
        .split('-')
        .next()
        .unwrap_or(tag)
        .to_ascii_lowercase()
        .as_str()
    {
        "ja" => "JP".to_string(),
        code => code.to_ascii_uppercase(),
    }
}

fn language_menu(tags: Vec<String>, saved: Option<&str>, system: Option<&str>) -> LanguageMenu {
    let labels: Vec<String> = tags.iter().map(|tag| compact_language_label(tag)).collect();
    let selected = saved
        .into_iter()
        .chain(system)
        .find_map(|preferred| {
            tags.iter()
                .position(|tag| tag.eq_ignore_ascii_case(preferred))
        })
        .or_else(|| (!tags.is_empty()).then_some(0));
    let languages = tags
        .iter()
        .zip(&labels)
        .map(|(tag, label)| OcrLanguage {
            tag: tag.clone(),
            // Distinguish regional/script packs when more than one shares a short label.
            label: if labels.iter().filter(|other| *other == label).count() > 1 {
                tag.to_ascii_uppercase()
            } else {
                label.clone()
            },
        })
        .collect();
    LanguageMenu {
        languages,
        selected,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn enter_mta_apartment() {
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    pin_process_mta();
    thread_local! {
        // Joining the apartment is intentionally never undone. COM tears down the process MTA when
        // the last thread uninitializes it, and that teardown unloads the WinRT implementation
        // DLLs behind activation factories already cached by the `windows` crate. Later OCR calls
        // then dereference an unmapped vtable and crash.
        static APARTMENT: () = {
            let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        };
    }
    APARTMENT.with(|_| ());
}

/// Pins an MTA for the whole process lifetime. `CoIncrementMTAUsage` only ever increments the
/// usage count, so the MTA and the DLLs hosting cached activation factories stay alive even
/// after the thread that created them exits. Must not run on Winit's main thread before its
/// `OleInitialize` call, which would fail with `RPC_E_CHANGED_MODE`.
#[cfg(target_os = "windows")]
fn pin_process_mta() {
    use windows::Win32::System::Com::CoIncrementMTAUsage;

    static PINNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    PINNED.get_or_init(|| {
        // The cookie is never decremented: reusing the process MTA is what keeps the cached
        // factory vtables mapped.
        let _ = unsafe { CoIncrementMTAUsage() };
    });
}

#[cfg(target_os = "windows")]
pub(crate) fn installed_language_menu(saved_tag: &str) -> Result<LanguageMenu> {
    use windows::{core::HSTRING, Globalization::Language, Media::Ocr::OcrEngine};

    enter_mta_apartment();
    (|| {
        let available = OcrEngine::AvailableRecognizerLanguages()
            .context("Unable to read installed Windows OCR languages")?;
        let mut tags = Vec::new();
        for index in 0..available.Size()? {
            tags.push(available.GetAt(index)?.LanguageTag()?.to_string());
        }
        let system = OcrEngine::TryCreateFromUserProfileLanguages()
            .and_then(|engine| engine.RecognizerLanguage())
            .and_then(|language| language.LanguageTag())
            .ok()
            .map(|tag| tag.to_string());
        // Resolve previously saved regional aliases (such as ko-KR -> ko) through Windows.
        let saved = if saved_tag.trim().is_empty() {
            None
        } else {
            Language::CreateLanguage(&HSTRING::from(saved_tag.trim()))
                .and_then(|language| OcrEngine::TryCreateFromLanguage(&language))
                .and_then(|engine| engine.RecognizerLanguage())
                .and_then(|language| language.LanguageTag())
                .ok()
                .map(|tag| tag.to_string())
        };
        Ok(language_menu(tags, saved.as_deref(), system.as_deref()))
    })()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn installed_language_menu(_saved_tag: &str) -> Result<LanguageMenu> {
    Ok(LanguageMenu::default())
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
    use windows::Foundation::AsyncStatus;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;

    if width == 0 || height == 0 || bgra_pixels.len() < (width as usize * height as usize * 4) {
        anyhow::bail!("The OCR image buffer is invalid");
    }

    enter_mta_apartment();
    (|| {
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
        let operation = engine
            .RecognizeAsync(&bitmap)
            .context("Failed to start Windows OCR")?;
        // Wait by polling the operation status instead of registering a completion delegate.
        // `IAsyncOperation::get()` installs a `SetCompleted` handler that the Windows OCR /
        // MediaFrame pipeline (RTMediaFrame.dll) invokes from its own worker thread; the app
        // crashed with an access violation inside that callback (ocr_trans.exe+0xD471E,
        // called from RTMediaFrame.dll+0xD87D), so no completion callback is registered here.
        let started = std::time::Instant::now();
        let ocr_result = loop {
            let status = operation
                .Status()
                .context("Failed to query Windows OCR status")?;
            if status == AsyncStatus::Completed {
                break operation.GetResults().context("Windows OCR failed")?;
            }
            if status == AsyncStatus::Canceled {
                anyhow::bail!("Windows OCR was canceled");
            }
            if status == AsyncStatus::Error {
                let code = operation.ErrorCode().map(|code| code.0 as u32).unwrap_or(0);
                anyhow::bail!("Windows OCR failed (0x{code:08X})");
            }
            if started.elapsed() > std::time::Duration::from_secs(30) {
                let _ = operation.Cancel();
                anyhow::bail!("Windows OCR timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
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
    })()
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