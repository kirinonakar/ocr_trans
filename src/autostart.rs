//! Per-user Windows logon registration. The registry is the source of truth for this option.
use anyhow::{Context, Result};
use std::path::Path;
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_registry::CURRENT_USER;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "OCRTrans";
const AUTOSTART_ARG: &str = "--autostart";

pub(crate) fn requested() -> bool {
    std::env::args_os().skip(1).any(|arg| arg == AUTOSTART_ARG)
}

fn startup_command(executable: &Path) -> Result<String> {
    let path = executable
        .to_str()
        .context("The executable path is not valid Unicode")?;
    anyhow::ensure!(
        executable.is_absolute() && !path.contains(['"', '\0']),
        "Invalid executable path"
    );
    let command = format!("\"{path}\" {AUTOSTART_ARG}");
    anyhow::ensure!(
        command.encode_utf16().count() <= 260,
        "The executable path is too long for Windows autostart"
    );
    Ok(command)
}

fn missing(error: &windows::core::Error) -> bool {
    error.code() == ERROR_FILE_NOT_FOUND.to_hresult()
}

fn registration_at(path: &str) -> Result<Option<String>> {
    let key = match CURRENT_USER.open(path) {
        Ok(key) => key,
        Err(error) if missing(&error) => return Ok(None),
        Err(error) => return Err(error).context("Unable to read Windows autostart settings"),
    };
    match key.get_string(VALUE_NAME) {
        Ok(value) => Ok(Some(value)),
        Err(error) if missing(&error) => Ok(None),
        Err(error) => Err(error).context("Unable to read the OCR Translator autostart entry"),
    }
}

pub(crate) fn is_enabled() -> Result<bool> {
    let Some(command) = registration_at(RUN_KEY)? else {
        return Ok(false);
    };
    Ok(command.eq_ignore_ascii_case(&startup_command(&std::env::current_exe()?)?))
}

fn set_enabled_at(path: &str, executable: &Path, enabled: bool) -> Result<()> {
    if enabled {
        let command = startup_command(executable)?;
        CURRENT_USER.create(path)?.set_string(VALUE_NAME, command)?;
    } else {
        // Do not create a key when disabling. Only remove this application's value.
        let key = match CURRENT_USER.options().write().open(path) {
            Ok(key) => key,
            Err(error) if missing(&error) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        match key.remove_value(VALUE_NAME) {
            Ok(()) => {}
            Err(error) if missing(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn set_enabled(enabled: bool) -> Result<()> {
    set_enabled_at(RUN_KEY, &std::env::current_exe()?, enabled)
        .context("Unable to update Windows autostart")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_executable_paths_and_supplies_the_tray_startup_argument() {
        assert_eq!(
            startup_command(Path::new(r"C:\OCR 번역 & Capture\ocr_trans.exe")).unwrap(),
            "\"C:\\OCR 번역 & Capture\\ocr_trans.exe\" --autostart"
        );
        assert!(startup_command(Path::new("ocr_trans.exe")).is_err());
        assert!(
            startup_command(Path::new(&format!(r"C:\{}\ocr_trans.exe", "a".repeat(260)))).is_err()
        );
    }

    #[test]
    fn registration_roundtrips_and_disabling_preserves_other_startup_entries() {
        struct TestKey(String);
        impl Drop for TestKey {
            fn drop(&mut self) {
                let _ = CURRENT_USER.remove_tree(&self.0);
            }
        }
        // An isolated key: tests never register a real Windows startup application.
        let path = TestKey(format!(
            r"Software\OCRTrans.Autostart.Test.{}",
            uuid::Uuid::new_v4()
        ));
        let executable = Path::new(r"C:\OCR 번역 & Capture\ocr_trans.exe");
        set_enabled_at(&path.0, executable, false).unwrap();
        assert!(registration_at(&path.0).unwrap().is_none());
        let key = CURRENT_USER.create(&path.0).unwrap();
        key.set_string("OtherApplication", "untouched").unwrap();
        set_enabled_at(&path.0, executable, true).unwrap();
        assert_eq!(
            registration_at(&path.0).unwrap(),
            Some(startup_command(executable).unwrap())
        );
        set_enabled_at(&path.0, executable, false).unwrap();
        set_enabled_at(&path.0, executable, false).unwrap();
        assert!(registration_at(&path.0).unwrap().is_none());
        assert_eq!(key.get_string("OtherApplication").unwrap(), "untouched");
    }
}
