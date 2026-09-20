//! Volume enumeration: letters, capacity, file system, drive type, SSD/HDD.

use crate::util::{wide, OwnedHandle};
use serde::Serialize;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::*;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    CdRom,
    RamDisk,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Ssd,
    Hdd,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    /// e.g. `C:\`
    pub root: String,
    pub letter: char,
    pub label: String,
    pub file_system: String,
    pub kind: DriveKind,
    pub media: MediaKind,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub ready: bool,
    /// True when the volume supports the NTFS MFT fast scan.
    pub fast_scan_capable: bool,
}

pub fn list_drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        out.push(drive_info(letter));
    }
    out
}

pub fn drive_info(letter: char) -> DriveInfo {
    let root = format!("{letter}:\\");
    let root_w = wide(&root);
    let kind = match unsafe { GetDriveTypeW(root_w.as_ptr()) } {
        2 => DriveKind::Removable,
        3 => DriveKind::Fixed,
        4 => DriveKind::Network,
        5 => DriveKind::CdRom,
        6 => DriveKind::RamDisk,
        _ => DriveKind::Unknown,
    };

    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    let mut serial = 0u32;
    let mut max_comp = 0u32;
    let mut flags = 0u32;
    let ready = unsafe {
        GetVolumeInformationW(
            root_w.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            &mut serial,
            &mut max_comp,
            &mut flags,
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } != 0;
    let cstr = |b: &[u16]| {
        let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf16_lossy(&b[..n])
    };
    let file_system = if ready { cstr(&fs) } else { String::new() };

    let (mut total, mut free) = (0u64, 0u64);
    if ready {
        let mut avail = 0u64;
        unsafe {
            GetDiskFreeSpaceExW(root_w.as_ptr(), &mut avail, &mut total, &mut free);
        }
    }
    let media = if kind == DriveKind::Fixed || kind == DriveKind::Removable {
        detect_media(letter)
    } else {
        MediaKind::Unknown
    };

    DriveInfo {
        root,
        letter,
        label: if ready { cstr(&label) } else { String::new() },
        fast_scan_capable: file_system.eq_ignore_ascii_case("NTFS")
            && matches!(kind, DriveKind::Fixed | DriveKind::Removable),
        file_system,
        kind,
        media,
        total_bytes: total,
        free_bytes: free,
        used_bytes: total.saturating_sub(free),
        ready,
    }
}

/// Uses `IOCTL_STORAGE_QUERY_PROPERTY(StorageDeviceSeekPenaltyProperty)`, which
/// does not need administrative rights when the device is opened with zero access.
fn detect_media(letter: char) -> MediaKind {
    let path = wide(format!(r"\\.\{letter}:"));
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    let Some(h) = OwnedHandle::new(h) else { return MediaKind::Unknown };
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceSeekPenaltyProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut desc: DEVICE_SEEK_PENALTY_DESCRIPTOR = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h.raw(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const _,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            &mut desc as *mut _ as *mut _,
            std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    } != 0;
    if !ok || returned < std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32 {
        return MediaKind::Unknown;
    }
    if desc.IncursSeekPenalty {
        MediaKind::Hdd
    } else {
        MediaKind::Ssd
    }
}

/// File system name and cluster size of the volume containing `path`.
pub fn volume_fs_for_path(path: &str) -> (String, u32) {
    let root = volume_root_of(path);
    let root_w = wide(&root);
    let mut fs = [0u16; 64];
    let ok = unsafe {
        GetVolumeInformationW(
            root_w.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } != 0;
    let name = if ok {
        let n = fs.iter().position(|&c| c == 0).unwrap_or(fs.len());
        String::from_utf16_lossy(&fs[..n])
    } else {
        String::new()
    };
    let (mut spc, mut bps, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    let cluster = if unsafe { GetDiskFreeSpaceW(root_w.as_ptr(), &mut spc, &mut bps, &mut free, &mut total) } != 0 {
        spc * bps
    } else {
        4096
    };
    (name, cluster.max(512))
}

/// `C:\foo\bar` -> `C:\`, `\\srv\share\x` -> `\\srv\share\`.
pub fn volume_root_of(path: &str) -> String {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        return format!("{}:\\", (b[0] as char).to_ascii_uppercase());
    }
    if let Some(rest) = path.strip_prefix(r"\\") {
        let mut parts = rest.split('\\');
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return format!(r"\\{server}\{share}\");
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots() {
        assert_eq!(volume_root_of("c:\\x\\y"), "C:\\");
        assert_eq!(volume_root_of(r"\\srv\share\a\b"), r"\\srv\share\");
    }

    #[test]
    fn system_drive_is_listed() {
        let drives = list_drives();
        let sys = std::env::var("SystemDrive").unwrap_or("C:".into());
        let letter = sys.chars().next().unwrap();
        let d = drives.iter().find(|d| d.letter == letter).expect("system drive present");
        assert!(d.ready);
        assert!(d.total_bytes > 0);
        assert!(d.used_bytes <= d.total_bytes);
    }
}
