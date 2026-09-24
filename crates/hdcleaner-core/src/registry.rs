//! Read-only registry access helpers (write/delete operations live in the
//! backup-aware registry operations module).

use crate::util::wide;
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Hive {
    LocalMachine,
    CurrentUser,
    ClassesRoot,
    Users,
}

impl Hive {
    pub fn raw(self) -> HKEY {
        match self {
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::ClassesRoot => HKEY_CLASSES_ROOT,
            Hive::Users => HKEY_USERS,
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Hive::LocalMachine => "HKLM",
            Hive::CurrentUser => "HKCU",
            Hive::ClassesRoot => "HKCR",
            Hive::Users => "HKU",
        }
    }
}

/// Registry view for 32/64-bit redirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum View {
    Default,
    Reg64,
    Reg32,
}

impl View {
    fn flag(self) -> u32 {
        match self {
            View::Default => 0,
            View::Reg64 => KEY_WOW64_64KEY,
            View::Reg32 => KEY_WOW64_32KEY,
        }
    }
}

pub struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

impl Key {
    pub fn open(hive: Hive, path: &str, view: View) -> Option<Key> {
        let p = wide(path);
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegOpenKeyExW(hive.raw(), p.as_ptr(), 0, KEY_READ | view.flag(), &mut h) };
        (rc == ERROR_SUCCESS).then_some(Key(h))
    }

    pub fn open_sub(&self, name: &str, view: View) -> Option<Key> {
        let p = wide(name);
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegOpenKeyExW(self.0, p.as_ptr(), 0, KEY_READ | view.flag(), &mut h) };
        (rc == ERROR_SUCCESS).then_some(Key(h))
    }

    pub fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = [0u16; 512];
        let mut i = 0u32;
        loop {
            let mut len = buf.len() as u32;
            let rc = unsafe {
                RegEnumKeyExW(
                    self.0,
                    i,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc == ERROR_SUCCESS {
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            i += 1;
            if i > 1_000_000 {
                break;
            }
        }
        out
    }

    pub fn value_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = vec![0u16; 16384];
        let mut i = 0u32;
        loop {
            let mut len = buf.len() as u32;
            let rc = unsafe {
                RegEnumValueW(
                    self.0,
                    i,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc == ERROR_SUCCESS {
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            i += 1;
            if i > 1_000_000 {
                break;
            }
        }
        out
    }

    fn raw_value(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        let n = wide(name);
        let mut ty = 0u32;
        let mut len = 0u32;
        let rc = unsafe { RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null(), &mut ty, std::ptr::null_mut(), &mut len) };
        if rc != ERROR_SUCCESS && rc != ERROR_MORE_DATA {
            return None;
        }
        for _ in 0..4 {
            let mut buf = vec![0u8; len as usize + 2];
            let mut l = buf.len() as u32;
            let rc = unsafe { RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null(), &mut ty, buf.as_mut_ptr(), &mut l) };
            if rc == ERROR_SUCCESS {
                buf.truncate(l as usize);
                return Some((ty, buf));
            }
            if rc != ERROR_MORE_DATA {
                return None;
            }
            len = l;
        }
        None
    }

    /// REG_SZ / REG_EXPAND_SZ (expanded) / REG_DWORD rendered as string.
    pub fn string(&self, name: &str) -> Option<String> {
        let (ty, data) = self.raw_value(name)?;
        match ty {
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
                let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
                let s = String::from_utf16_lossy(&units[..end]);
                Some(if ty == REG_EXPAND_SZ { expand_env(&s) } else { s })
            }
            REG_DWORD if data.len() >= 4 => Some(u32::from_le_bytes(data[..4].try_into().ok()?).to_string()),
            _ => None,
        }
    }

    pub fn dword(&self, name: &str) -> Option<u32> {
        let (ty, data) = self.raw_value(name)?;
        match ty {
            REG_DWORD if data.len() >= 4 => Some(u32::from_le_bytes(data[..4].try_into().ok()?)),
            REG_SZ => self.string(name)?.trim().parse().ok(),
            _ => None,
        }
    }

    /// Raw bytes + type, used for registry backups.
    pub fn value_raw(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        self.raw_value(name)
    }
}

pub fn read_string(hive: Hive, path: &str, name: &str, view: View) -> Option<String> {
    Key::open(hive, path, view)?.string(name)
}

/// Expand `%VAR%` references using the process environment.
pub fn expand_env(s: &str) -> String {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    let src = wide(s);
    let needed = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return s.to_string();
    }
    let mut buf = vec![0u16; needed as usize + 1];
    let got = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if got == 0 || got as usize > buf.len() {
        return s.to_string();
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_known_value() {
        let v = read_string(
            Hive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuildNumber",
            View::Default,
        );
        assert!(v.map(|s| s.parse::<u32>().is_ok()).unwrap_or(false));
        let k = Key::open(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", View::Reg64).unwrap();
        assert!(!k.subkeys().is_empty());
    }

    #[test]
    fn expands() {
        let win = std::env::var("SystemRoot").unwrap();
        assert_eq!(expand_env("%SystemRoot%\\x").to_lowercase(), format!("{win}\\x").to_lowercase());
    }
}
