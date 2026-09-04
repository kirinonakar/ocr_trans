use crate::capture;

pub(crate) fn clean_text(text: &str) -> String {
    normalize_japanese_spacing(&text.replace("\r\n", "\n").replace('\r', "\n"))
        .trim()
        .to_string()
}

fn normalize_japanese_spacing(text: &str) -> String {
    text.split('\n')
        .map(normalize_japanese_line_spacing)
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_japanese_line_spacing(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut normalized = String::with_capacity(line.len());
    let mut previous_non_space = None;
    let mut index = 0;

    while index < chars.len() {
        if is_inline_space(chars[index]) {
            let start = index;
            while index < chars.len() && is_inline_space(chars[index]) {
                index += 1;
            }

            let next_non_space = chars.get(index).copied();
            if !previous_non_space
                .zip(next_non_space)
                .is_some_and(|(previous, next)| {
                    is_japanese_spacing_char(previous) && is_japanese_spacing_char(next)
                })
            {
                normalized.extend(chars[start..index].iter().copied());
            }
            continue;
        }

        normalized.push(chars[index]);
        previous_non_space = Some(chars[index]);
        index += 1;
    }

    normalized
}

fn is_inline_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\u{3000}')
}

fn is_japanese_spacing_char(character: char) -> bool {
    matches!(
        character as u32,
        0x3000..=0x303f // Japanese punctuation and iteration marks
            | 0x3040..=0x30ff // Hiragana and Katakana
            | 0x31f0..=0x31ff // Katakana extensions
            | 0x3400..=0x4dbf // CJK extension A
            | 0x4e00..=0x9fff // CJK unified ideographs
            | 0xf900..=0xfaff // CJK compatibility ideographs
            | 0xff01..=0xff65 // Full-width Japanese punctuation/forms
            | 0xff66..=0xff9f // Half-width Katakana
    )
}

pub(crate) fn calculate_font_size(text: &str, width: f32, height: f32, max_size: f32) -> f32 {
    if text.is_empty() {
        return max_size;
    }

    // 1. Dynamic padding based on window size to maximize space in small overlays
    let padding_v = if height < 120.0 {
        (height * 0.2).max(20.0)
    } else {
        48.0
    };
    let padding_h = if width < 120.0 {
        (width * 0.1).max(12.0)
    } else {
        32.0
    };

    let available_w = (width - padding_h).max(20.0);
    let available_h = (height - padding_v).max(20.0);

    // Responsive font size for Searching...
    if text.starts_with("Searching...") {
        return (max_size * 1.1).min(available_h).max(10.0);
    }

    // Helper closure to check if text fits at a given font size
    let fits = |size: f32| -> bool {
        let line_height_est = size * 1.35; // Slightly tighter line height for better fitting
        let mut total_height = 0.0;

        for line in text.lines() {
            let line_trimmed = line.trim();
            if line_trimmed.is_empty() {
                total_height += line_height_est;
            } else {
                let mut line_width = 0.0;
                for c in line_trimmed.chars() {
                    // CJK characters are essentially square (1.0 ratio)
                    // Latin/Numbers are roughly 0.55-0.6 ratio
                    // Spaces are narrower (0.3 ratio)
                    let char_w = if (c >= '\u{3000}' && c <= '\u{9FFF}')
                        || (c >= '\u{AC00}' && c <= '\u{D7AF}')
                    {
                        size
                    } else if c.is_whitespace() {
                        size * 0.3
                    } else {
                        size * 0.58
                    };
                    line_width += char_w;
                }
                let num_wrapped_lines = (line_width / available_w).ceil().max(1.0);
                total_height += num_wrapped_lines * line_height_est;
            }
            if total_height > available_h {
                return false;
            }
        }
        total_height <= available_h
    };

    // 2. Binary search for the best font size (8.0 to max_size)
    // This provides much better precision and performance than linear search.
    let mut low = 8.0;
    let mut high = max_size;
    let mut best_size = low;

    // Fast-path: check if max_size already fits
    if fits(max_size) {
        return max_size;
    }

    // Binary search for precision (8 iterations = ~0.25px precision for range 8-72)
    for _ in 0..8 {
        let mid = (low + high) / 2.0;
        if fits(mid) {
            best_size = mid;
            low = mid;
        } else {
            high = mid;
        }
    }

    // Round to 0.5 for stability and clean appearance
    (best_size * 2.0).round() / 2.0
}

pub(crate) fn rgba_to_slint_image(rgba: image::RgbaImage) -> slint::Image {
    let (width, height) = rgba.dimensions();
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        width,
        height,
    );
    slint::Image::from_rgba8(buffer)
}

pub(crate) fn format_color_values(color: &image::Rgba<u8>) -> (String, String) {
    (
        format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]),
        format!("({}, {}, {})", color[0], color[1], color[2]),
    )
}

pub(crate) fn color_toolbar_tooltip(color: &image::Rgba<u8>) -> String {
    let (hex, decimal) = format_color_values(color);
    format!("HEX: {hex} | DEC: {decimal}")
}

pub(crate) fn color_selection_tooltip(color: &image::Rgba<u8>) -> String {
    let (hex, decimal) = format_color_values(color);
    format!("HEX: {hex}\nDEC: {decimal}")
}

pub(crate) fn color_preview(color: &image::Rgba<u8>) -> slint::Color {
    slint::Color::from_rgb_u8(color[0], color[1], color[2])
}

fn ruler_corners(rect: capture::CaptureRect) -> ((i32, i32), (i32, i32), (i32, i32), (i32, i32)) {
    let right = rect.x + rect.width;
    let bottom = rect.y + rect.height;
    (
        (rect.x, rect.y),
        (right, rect.y),
        (right, bottom),
        (rect.x, bottom),
    )
}

pub(crate) fn ruler_toolbar_tooltip(rect: capture::CaptureRect) -> String {
    let (top_left, top_right, bottom_right, bottom_left) = ruler_corners(rect);
    format!(
        "TL ({},{})  TR ({},{})  BR ({},{})  BL ({},{})  |  W {}  H {}",
        top_left.0,
        top_left.1,
        top_right.0,
        top_right.1,
        bottom_right.0,
        bottom_right.1,
        bottom_left.0,
        bottom_left.1,
        rect.width,
        rect.height,
    )
}

pub(crate) fn ruler_selection_lines(rect: capture::CaptureRect) -> (String, String) {
    let (top_left, top_right, bottom_right, bottom_left) = ruler_corners(rect);
    (
        format!(
            "TL ({},{})  TR ({},{})",
            top_left.0, top_left.1, top_right.0, top_right.1
        ),
        format!(
            "BL ({},{})  BR ({},{})  |  W {}  H {}",
            bottom_left.0, bottom_left.1, bottom_right.0, bottom_right.1, rect.width, rect.height,
        ),
    )
}

pub(crate) fn ruler_clipboard_text(rect: capture::CaptureRect) -> String {
    let (top_left, top_right, bottom_right, bottom_left) = ruler_corners(rect);
    format!(
        "Top-left: ({}, {})\nTop-right: ({}, {})\nBottom-right: ({}, {})\nBottom-left: ({}, {})\nWidth: {}\nHeight: {}",
        top_left.0,
        top_left.1,
        top_right.0,
        top_right.1,
        bottom_right.0,
        bottom_right.1,
        bottom_left.0,
        bottom_left.1,
        rect.width,
        rect.height,
    )
}

#[cfg(test)]
mod text_tests {
    use super::{clean_text, ruler_clipboard_text, ruler_selection_lines, ruler_toolbar_tooltip};
    use crate::capture::CaptureRect;

    #[test]
    fn removes_spaces_inserted_between_japanese_characters() {
        assert_eq!(
            clean_text("コ ミ ュ ニ テ ィ 活 動 の 理 解 獲 得"),
            "コミュニティ活動の理解獲得"
        );
    }

    #[test]
    fn preserves_line_breaks_and_latin_word_spacing() {
        assert_eq!(
            clean_text("日 本 AI  tool\r\n次 の 行"),
            "日本 AI  tool\n次の行"
        );
    }

    #[test]
    fn formats_all_ruler_corners_and_dimensions() {
        let rect = CaptureRect {
            x: -120,
            y: 45,
            width: 320,
            height: 180,
        };

        assert_eq!(
            ruler_toolbar_tooltip(rect),
            "TL (-120,45)  TR (200,45)  BR (200,225)  BL (-120,225)  |  W 320  H 180"
        );
        assert_eq!(
            ruler_selection_lines(rect),
            (
                "TL (-120,45)  TR (200,45)".to_string(),
                "BL (-120,225)  BR (200,225)  |  W 320  H 180".to_string(),
            )
        );
        assert_eq!(
            ruler_clipboard_text(rect),
            "Top-left: (-120, 45)\nTop-right: (200, 45)\nBottom-right: (200, 225)\nBottom-left: (-120, 225)\nWidth: 320\nHeight: 180"
        );
    }
}
