//! Font families offered by the shared result-window font setting.

const PREFERRED_FONT: &str = "Noto Sans CJK JP";
const FALLBACK_FONT: &str = "Yu Gothic UI";

pub(crate) fn default_result_font() -> &'static str {
    if installed_families()
        .iter()
        .any(|name| name.eq_ignore_ascii_case(PREFERRED_FONT))
    {
        PREFERRED_FONT
    } else {
        FALLBACK_FONT
    }
}

pub(crate) fn result_font_family(value: &str) -> &str {
    let value = value.trim();
    if value.is_empty() {
        default_result_font()
    } else {
        value
    }
}

fn installed_families() -> &'static Vec<String> {
    static FAMILIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut families: Vec<String> = Vec::new();
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::Foundation::LPARAM;
            use windows::Win32::Graphics::Gdi::{
                EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW,
            };

            unsafe extern "system" fn collect_family(
                font: *const LOGFONTW,
                _metrics: *const TEXTMETRICW,
                _font_type: u32,
                data: LPARAM,
            ) -> i32 {
                // Enumeration is synchronous; `families` stays borrowed until it returns.
                let families = &mut *(data.0 as *mut Vec<String>);
                let name = &(*font).lfFaceName;
                let end = name.iter().position(|&ch| ch == 0).unwrap_or(name.len());
                let name = String::from_utf16_lossy(&name[..end]);
                // Skip vertical-writing aliases.
                if !name.is_empty() && !name.starts_with('@') {
                    families.push(name);
                }
                1
            }

            let dc = GetDC(None);
            if !dc.0.is_null() {
                let query = LOGFONTW {
                    lfCharSet: DEFAULT_CHARSET,
                    ..Default::default()
                };
                EnumFontFamiliesExW(
                    dc,
                    &query,
                    Some(collect_family),
                    LPARAM(&mut families as *mut Vec<String> as isize),
                    0,
                );
                ReleaseDC(None, dc);
            }
        }
        families.sort_by_cached_key(|name| name.to_lowercase());
        families.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        families
    })
}

pub(crate) fn font_menu(saved: &str) -> Vec<String> {
    let mut families = installed_families().clone();
    // Preserve a saved family even when it is temporarily unavailable on this machine.
    families.push(FALLBACK_FONT.to_string());
    families.push(result_font_family(saved).to_string());
    families.sort_by_cached_key(|name| name.to_lowercase());
    families.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    families
}
