//! The signed-in YouTube session, shared by `mochy-app` and the
//! `mochy-signin` window: JSON encrypted with Windows DPAPI for the current
//! user (`yt_session.bin` in the data dir). Other platforms store it with
//! owner-only permissions.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// The signed-in session (secret).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub cookie: String,
    #[serde(default)]
    pub data_sync_id: Option<String>,
    #[serde(default)]
    pub visitor_data: Option<String>,
}

pub fn session_path() -> PathBuf {
    crate::data_dir().join("yt_session.bin")
}

/// WebView2 profile used by the sign-in window.
pub fn webview_dir() -> PathBuf {
    crate::data_dir().join("webview-youtube")
}

/// Modification time of the session file (to notice a sign-in made by the
/// standalone sign-in window while the app runs).
pub fn session_mtime() -> Option<SystemTime> {
    std::fs::metadata(session_path()).and_then(|m| m.modified()).ok()
}

pub fn load_session() -> Option<Session> {
    let bytes = std::fs::read(session_path()).ok()?;
    let plain = unprotect(&bytes).ok()?;
    serde_json::from_slice::<Session>(&plain).ok().filter(|s| !s.cookie.is_empty())
}

pub fn save_session(s: &Session) -> Result<(), String> {
    let json = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    let blob = protect(&json)?;
    let path = session_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("bin.tmp");
    std::fs::write(&tmp, blob).map_err(|e| e.to_string())?;
    restrict_permissions(&tmp);
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn delete_session() {
    let _ = std::fs::remove_file(session_path());
}

// ---------------------------------------------------------------------------
// Encryption
// ---------------------------------------------------------------------------

/// Extra entropy so other apps using DPAPI for this user can't trivially
/// decrypt the blob by calling `CryptUnprotectData` without it.
#[cfg(windows)]
const ENTROPY: &[u8] = b"mochy-desktop/youtube-session/v1";

#[cfg(windows)]
pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(data, true)
}

#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(data, false)
}

#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: all pointers are valid for the duration of the call; DPAPI
    // allocates `out.pbData` with LocalAlloc, freed below after copying.
    let ok = unsafe {
        if encrypt {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return Err(format!("DPAPI {} failed (error {})", if encrypt { "encrypt" } else { "decrypt" }, std::io::Error::last_os_error()));
    }
    // SAFETY: DPAPI returned `cbData` valid bytes at `pbData`.
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    Ok(v)
}

#[cfg(not(windows))]
pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(data.to_vec())
}

#[cfg(not(windows))]
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(data.to_vec())
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_roundtrip() {
        let secret = br#"{"cookie":"SAPISID=abc; SID=def"}"#;
        let blob = protect(secret).unwrap();
        #[cfg(windows)]
        assert_ne!(blob.as_slice(), secret.as_slice(), "DPAPI output must not be the plaintext");
        assert_eq!(unprotect(&blob).unwrap(), secret);
        #[cfg(windows)]
        assert!(unprotect(b"not a dpapi blob").is_err());
    }
}
