//! Per-user Windows Credential Manager; never a plaintext settings fallback.
use anyhow::{bail, Result};
#[cfg(any(windows, test))]
use sha2::{Digest, Sha256};
use std::path::Path;
#[cfg(any(windows, test))]
fn target(root: &Path) -> String {
    format!(
        "StreamSliceLite:Groq:{:x}",
        Sha256::digest(root.to_string_lossy().as_bytes())
    )
}
#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
#[cfg(windows)]
pub(crate) fn load(root: &Path) -> Result<Option<String>> {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_NOT_FOUND},
        Security::Credentials::*,
    };
    let name = wide(&target(root));
    let mut p = std::ptr::null_mut();
    // The OS owns this bounded credential buffer until CredFree.
    unsafe {
        if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 {
            let e = GetLastError();
            if e == ERROR_NOT_FOUND {
                return Ok(None);
            }
            bail!("Windows credential store unavailable (code {e}); use a session-only key");
        }
        if p.is_null() {
            bail!("Empty credential response");
        }
        let size = (*p).CredentialBlobSize as usize;
        let value = if (12..=512).contains(&size) && !(*p).CredentialBlob.is_null() {
            String::from_utf8(std::slice::from_raw_parts((*p).CredentialBlob, size).to_vec()).ok()
        } else {
            None
        };
        CredFree(p.cast());
        match value {
            Some(s) if s.bytes().all(|b| b.is_ascii_graphic()) => Ok(Some(s)),
            _ => bail!("Stored key is invalid; remove it and enter it again"),
        }
    }
}
#[cfg(windows)]
pub(crate) fn save(root: &Path, key: &str) -> Result<()> {
    use windows_sys::Win32::{Foundation::GetLastError, Security::Credentials::*};
    if !(12..=512).contains(&key.len()) || !key.bytes().all(|b| b.is_ascii_graphic()) {
        bail!("Invalid credential format");
    }
    let name = wide(&target(root));
    let username = wide("StreamSlice Lite Groq API key");
    // Both strings and the read-only UTF-8 key outlive this synchronous OS call.
    unsafe {
        let mut c: CREDENTIALW = std::mem::zeroed();
        c.Type = CRED_TYPE_GENERIC;
        c.TargetName = name.as_ptr() as *mut u16;
        c.UserName = username.as_ptr() as *mut u16;
        c.CredentialBlob = key.as_ptr() as *mut u8;
        c.CredentialBlobSize = key.len() as u32;
        c.Persist = CRED_PERSIST_LOCAL_MACHINE;
        if CredWriteW(&c, 0) == 0 {
            bail!(
                "Windows could not store the key (code {}); uncheck Remember for this session",
                GetLastError()
            );
        }
    }
    Ok(())
}
#[cfg(windows)]
pub(crate) fn clear(root: &Path) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_NOT_FOUND},
        Security::Credentials::*,
    };
    let name = wide(&target(root));
    unsafe {
        if CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
            let e = GetLastError();
            if e != ERROR_NOT_FOUND {
                bail!("Memory key removed, but the stored Windows credential could not be deleted (code {e})");
            }
        }
    }
    Ok(())
}
#[cfg(not(windows))]
pub(crate) fn load(_root: &Path) -> Result<Option<String>> {
    Ok(None)
}
#[cfg(not(windows))]
pub(crate) fn save(_root: &Path, _key: &str) -> Result<()> {
    bail!("Remember key is Windows-only; session-only entry remains available")
}
#[cfg(not(windows))]
pub(crate) fn clear(_root: &Path) -> Result<()> {
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets_are_data_folder_scoped() {
        assert_ne!(target(Path::new("one")), target(Path::new("two")));
        assert!(target(Path::new("one")).starts_with("StreamSliceLite:Groq:"));
    }
    #[test]
    fn target_contains_no_key() {
        assert_eq!(target(Path::new("one")).len(), 21 + 64);
    }
}
