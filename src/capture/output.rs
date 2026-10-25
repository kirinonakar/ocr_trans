//! Capture file naming, output directories, and clipboard export.
use anyhow::{Context, Result};
use image::RgbaImage;
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Mutex;

static OUTPUT_PATH_STATE: Mutex<Option<(String, u64)>> = Mutex::new(None);

pub fn output_directory() -> PathBuf {
    let pictures = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|path| path.join("Pictures"));
    if let Some(path) = pictures.filter(|path| path.is_dir()) {
        return path;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn configured_output_directory(configured: Option<&str>) -> PathBuf {
    if let Some(folder) = configured
        .map(str::trim)
        .filter(|folder| !folder.is_empty())
    {
        let path = PathBuf::from(folder);
        if path.is_dir() {
            return path;
        }
    }
    output_directory()
}

#[cfg(target_os = "windows")]
fn current_date_for_filename() -> String {
    #[repr(C)]
    struct WindowsSystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(system_time: *mut WindowsSystemTime);
    }

    let mut time = WindowsSystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    unsafe { GetLocalTime(&mut time) };
    if (1..=9999).contains(&time.year)
        && (1..=12).contains(&time.month)
        && (1..=31).contains(&time.day)
    {
        format!("{:04}-{:02}-{:02}", time.year, time.month, time.day)
    } else {
        // GetLocalTime is not expected to fail, but keep the filename usable if it does.
        "0000-00-00".to_string()
    }
}

#[cfg(not(target_os = "windows"))]
fn current_date_for_filename() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    let (year, month, day) = civil_date_from_days(days as i64);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

#[cfg(not(target_os = "windows"))]
fn civil_date_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    // Howard Hinnant's proleptic Gregorian calendar conversion.
    let shifted = days_since_unix_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted / 146_097
    } else {
        (shifted - 146_096) / 146_097
    };
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

pub fn unique_output_path_in(extension: &str, configured_folder: Option<&str>) -> Result<PathBuf> {
    let directory = configured_output_directory(configured_folder);
    std::fs::create_dir_all(&directory).context("Failed to create capture directory")?;
    let date = current_date_for_filename();
    let mut path_state = OUTPUT_PATH_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut index = path_state
        .as_ref()
        .filter(|(last_date, _)| last_date == &date)
        .map(|(_, next_index)| *next_index)
        .unwrap_or(1);

    loop {
        let stem = format!("{date}_{index:03}");
        // Keep the sequence unique across both capture and recording files, even though their
        // extensions differ.
        let taken = ["png", "mp4", extension]
            .into_iter()
            .any(|candidate| directory.join(format!("{stem}.{candidate}")).exists());
        if !taken {
            // Reserve the number before releasing the lock so overlapping capture/record jobs
            // cannot be handed the same filename before either one reaches its save step.
            *path_state = Some((date.clone(), index.saturating_add(1)));
            return Ok(directory.join(format!("{stem}.{extension}")));
        }
        index = index.saturating_add(1);
    }
}

pub fn save_png_and_copy_to(image: &RgbaImage, configured_folder: Option<&str>) -> Result<PathBuf> {
    let path = unique_output_path_in("png", configured_folder)?;
    image.save(&path).context("Failed to save PNG capture")?;
    copy_image_to_clipboard(image)?;
    Ok(path)
}

pub fn copy_image_to_clipboard(image: &RgbaImage) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("Failed to open clipboard")?;
    clipboard
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: Cow::Borrowed(image.as_raw()),
        })
        .context("Failed to copy image to clipboard")
}

pub fn copy_text_to_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("Failed to open clipboard")?;
    clipboard
        .set_text(text)
        .context("Failed to copy text to clipboard")
}
