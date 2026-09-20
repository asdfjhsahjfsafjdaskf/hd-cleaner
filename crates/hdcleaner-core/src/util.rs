//! Small shared helpers: UTF-16 conversion, long-path handling, FILETIME, handles.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

/// NUL-terminated UTF-16 copy of a string.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Decode UTF-16 (lossy for unpaired surrogates, which NTFS allows).
pub fn from_wide(w: &[u16]) -> String {
    String::from_utf16_lossy(w)
}

/// Convert a user path into an extended-length (`\\?\`) path so that paths
/// longer than MAX_PATH work. Relative paths are rejected by callers before
/// they get here; this function only rewrites the prefix.
pub fn to_extended(path: &str) -> String {
    let p = path.replace('/', "\\");
    if p.starts_with(r"\\?\") || p.starts_with(r"\\.\") {
        p
    } else if let Some(rest) = p.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{rest}")
    } else {
        format!(r"\\?\{p}")
    }
}

/// Strip an extended-length prefix for display.
pub fn from_extended(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Normalize a user-supplied root path: absolute, backslashes, a drive root
/// keeps its trailing backslash, other paths lose it.
pub fn normalize_root(path: &str) -> crate::Result<String> {
    let trimmed = path.trim().trim_matches('"').replace('/', "\\");
    if trimmed.is_empty() {
        return Err(crate::AppError::InvalidInput("empty path".into()));
    }
    if trimmed.contains('\0') {
        return Err(crate::AppError::InvalidInput("path contains NUL".into()));
    }
    let bytes = trimmed.as_bytes();
    let is_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let is_unc = trimmed.starts_with(r"\\");
    if !is_drive && !is_unc {
        return Err(crate::AppError::InvalidInput(format!("path must be absolute: {trimmed}")));
    }
    if is_drive && bytes.len() == 2 {
        return Ok(format!("{}:\\", (bytes[0] as char).to_ascii_uppercase()));
    }
    if is_drive && bytes.len() >= 3 && bytes[2] != b'\\' {
        return Err(crate::AppError::InvalidInput(format!("drive-relative paths are not supported: {trimmed}")));
    }
    // Reject `..` components so a root can never escape what the user picked.
    if trimmed.split('\\').any(|c| c == "..") {
        return Err(crate::AppError::InvalidInput(format!("path traversal component in: {trimmed}")));
    }
    let mut out = trimmed;
    if is_drive {
        out.replace_range(0..1, &out[0..1].to_ascii_uppercase());
    }
    while out.ends_with('\\') && !(is_drive && out.len() == 3) && out.len() > 2 {
        out.pop();
    }
    Ok(out)
}

pub fn join(parent: &str, name: &str) -> String {
    if parent.ends_with('\\') {
        format!("{parent}{name}")
    } else {
        format!("{parent}\\{name}")
    }
}

/// FILETIME (100ns ticks since 1601) -> Unix milliseconds. 0 stays 0.
pub fn filetime_to_unix_ms(ft: u64) -> i64 {
    if ft == 0 {
        return 0;
    }
    const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
    if ft < EPOCH_DIFF_100NS {
        return 0;
    }
    ((ft - EPOCH_DIFF_100NS) / 10_000) as i64
}

pub fn unix_ms_to_filetime(ms: i64) -> u64 {
    const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
    if ms <= 0 {
        return 0;
    }
    (ms as u64) * 10_000 + EPOCH_DIFF_100NS
}

pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// RAII wrapper that closes a Win32 handle.
pub struct OwnedHandle(pub HANDLE);

impl OwnedHandle {
    pub fn new(h: HANDLE) -> Option<Self> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(h))
        }
    }
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

/// Directory where the application keeps its local data.
pub fn app_data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(crate::branding::DATA_DIR_NAME)
}

pub fn ensure_dir(p: &Path) -> crate::Result<()> {
    std::fs::create_dir_all(p).map_err(|e| crate::AppError::io("creating directory", Some(p), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_roots() {
        assert_eq!(normalize_root("c:").unwrap(), "C:\\");
        assert_eq!(normalize_root("c:\\").unwrap(), "C:\\");
        assert_eq!(normalize_root("C:\\Users\\").unwrap(), "C:\\Users");
        assert_eq!(normalize_root("\"C:/Users/x\"").unwrap(), "C:\\Users\\x");
        assert_eq!(normalize_root(r"\\server\share\").unwrap(), r"\\server\share");
        assert!(normalize_root("relative\\path").is_err());
        assert!(normalize_root("C:relative").is_err());
        assert!(normalize_root("C:\\a\\..\\Windows").is_err());
        assert!(normalize_root("").is_err());
    }

    #[test]
    fn extended_paths() {
        assert_eq!(to_extended("C:\\x"), "\\\\?\\C:\\x");
        assert_eq!(to_extended("\\\\srv\\s"), "\\\\?\\UNC\\srv\\s");
        assert_eq!(from_extended("\\\\?\\UNC\\srv\\s"), "\\\\srv\\s");
        assert_eq!(from_extended("\\\\?\\C:\\x"), "C:\\x");
    }

    #[test]
    fn filetime_roundtrip() {
        let ms = 1_700_000_000_000;
        assert_eq!(filetime_to_unix_ms(unix_ms_to_filetime(ms)), ms);
        assert_eq!(filetime_to_unix_ms(0), 0);
    }
}
