//! Process / OS level queries.

use crate::util::OwnedHandle;
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// True when the current process runs with an elevated (administrator) token.
pub fn is_elevated() -> bool {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let Some(token) = OwnedHandle::new(token) else { return false };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    } != 0;
    ok && elevation.TokenIsElevated != 0
}

/// Windows directory (e.g. `C:\Windows`).
pub fn windows_dir() -> String {
    std::env::var("SystemRoot")
        .or_else(|_| std::env::var("windir"))
        .unwrap_or_else(|_| r"C:\Windows".into())
}

pub fn system_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    pub elevated: bool,
    pub arch: &'static str,
    pub windows_dir: String,
    pub build: u32,
}

pub fn os_info() -> OsInfo {
    OsInfo {
        elevated: is_elevated(),
        arch: std::env::consts::ARCH,
        windows_dir: windows_dir(),
        build: os_build(),
    }
}

/// Windows build number from the registry (`CurrentBuildNumber`).
pub fn os_build() -> u32 {
    crate::registry::read_string(
        crate::registry::Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
        crate::registry::View::Default,
    )
    .and_then(|s| s.parse().ok())
    .unwrap_or(0)
}
