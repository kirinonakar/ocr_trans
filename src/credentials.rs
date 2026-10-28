use anyhow::Result;

#[cfg(target_os = "windows")]
mod platform {
    use anyhow::{Context, Result};
    use std::ffi::c_void;
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    pub fn read_credential(target: &str) -> Option<String> {
        let target_h = HSTRING::from(target);
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();

        let read_result = unsafe { CredReadW(&target_h, CRED_TYPE_GENERIC, None, &mut credential) };
        if let Err(err) = read_result {
            if !is_not_found(&err) {
                log::warn!(
                    "Failed to read credential '{}' from Credential Manager: {err}",
                    target
                );
            }
            return None;
        }

        if credential.is_null() {
            return None;
        }

        let key_bytes = unsafe {
            let credential_ref = &*credential;
            if credential_ref.CredentialBlob.is_null() || credential_ref.CredentialBlobSize == 0 {
                CredFree(credential as *const c_void);
                return None;
            }
            std::slice::from_raw_parts(
                credential_ref.CredentialBlob as *const u8,
                credential_ref.CredentialBlobSize as usize,
            )
            .to_vec()
        };
        unsafe {
            CredFree(credential as *const c_void);
        }

        String::from_utf8(key_bytes)
            .ok()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    }

    pub fn store_credential(
        target: &str,
        user_name: &str,
        comment: &str,
        api_key: &str,
    ) -> Result<()> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return delete_credential(target);
        }

        let mut target_w = to_wide(target);
        let mut user_name_w = to_wide(user_name);
        let mut comment_w = to_wide(comment);
        let mut blob = api_key.as_bytes().to_vec();

        let credential = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target_w.as_mut_ptr()),
            Comment: PWSTR(comment_w.as_mut_ptr()),
            CredentialBlobSize: blob
                .len()
                .try_into()
                .context("API key is too large for Credential Manager")?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user_name_w.as_mut_ptr()),
            ..Default::default()
        };

        unsafe { CredWriteW(&credential, 0) }
            .context("Failed to store API key in Credential Manager")
    }

    fn delete_credential(target: &str) -> Result<()> {
        let target_h = HSTRING::from(target);
        match unsafe { CredDeleteW(&target_h, CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(err) if is_not_found(&err) => Ok(()),
            Err(err) => Err(err).context("Failed to delete API key from Credential Manager"),
        }
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn is_not_found(err: &windows::core::Error) -> bool {
        err.code() == hresult_from_win32(ERROR_NOT_FOUND.0)
    }

    const fn hresult_from_win32(error: u32) -> windows::core::HRESULT {
        if error as i32 <= 0 {
            windows::core::HRESULT(error as i32)
        } else {
            windows::core::HRESULT(((error & 0x0000_FFFF) | (7 << 16) | 0x8000_0000) as i32)
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use anyhow::Result;
    pub fn read_credential(_target: &str) -> Option<String> {
        None
    }
    pub fn store_credential(_target: &str, _name: &str, _comment: &str, _key: &str) -> Result<()> {
        Ok(())
    }
}

pub fn read_api_key(provider: &crate::providers::Provider) -> Option<String> {
    provider.key_target.and_then(platform::read_credential)
}

pub fn store_api_key(provider: &crate::providers::Provider, api_key: &str) -> Result<()> {
    if let Some(target) = provider.key_target {
        if platform::read_credential(target)
            .as_deref()
            .unwrap_or_default()
            == api_key.trim()
        {
            return Ok(());
        }
        platform::store_credential(
            target,
            provider.name,
            &format!("OCR Translator {} API key", provider.name),
            api_key,
        )?;
    }
    Ok(())
}
