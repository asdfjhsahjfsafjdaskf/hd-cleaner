//! Registry write operations: backup (.reg export) and deletion.
//!
//! Deletion is restricted to an allowlist of shapes that application
//! leftovers can legitimately have. Anything else (Windows keys, classes,
//! policies, SYSTEM...) is refused, whoever asks — this check also runs inside
//! the elevated helper.

use crate::registry::{Hive, Key, View};
use crate::util::wide;
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::*;

/// Subtrees under `Software\` that are never deleted (Windows / shared data).
const FORBIDDEN_SUBTREES: &[&str] = &[
    "microsoft", "classes", "policies", "wow6432node", "clients", "registeredapplications", "odbc", "windows",
    "system", "defaultusersettings",
];
/// Vendor keys shared by many products: only their product subkeys may go.
const SHARED_VENDORS: &[&str] = &["google", "mozilla", "intel", "khronos", "nvidia corporation", "amd", "apple inc."];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegTarget {
    pub hive: Hive,
    /// Path below the hive, e.g. `Software\Vendor\App`.
    pub path: String,
    pub view: View,
    /// `Some(name)` targets a single value instead of the whole key.
    pub value: Option<String>,
}

impl RegTarget {
    pub fn display(&self) -> String {
        let hive = match self.hive {
            Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
            Hive::CurrentUser => "HKEY_CURRENT_USER",
            Hive::ClassesRoot => "HKEY_CLASSES_ROOT",
            Hive::Users => "HKEY_USERS",
        };
        let view = if self.view == View::Reg32 && self.hive == Hive::LocalMachine { r"\[32-bit view]" } else { "" };
        match &self.value {
            Some(v) => format!(r"{hive}\{}{view} → {v}", self.path),
            None => format!(r"{hive}\{}{view}", self.path),
        }
    }
}

fn comps(path: &str) -> Vec<String> {
    path.split('\\').filter(|c| !c.is_empty()).map(|c| c.to_lowercase()).collect()
}

/// Is this registry target allowed to be deleted?
pub fn deletion_allowed(t: &RegTarget) -> std::result::Result<(), &'static str> {
    if !matches!(t.hive, Hive::LocalMachine | Hive::CurrentUser) {
        return Err("hiveNotAllowed");
    }
    let mut c = comps(&t.path);
    if c.iter().any(|p| p == ".." || p.contains('\0')) {
        return Err("invalidPath");
    }
    if c.first().map(String::as_str) != Some("software") {
        return Err("outsideSoftware");
    }
    c.remove(0);
    if c.first().map(String::as_str) == Some("wow6432node") {
        c.remove(0);
    }
    let cv = ["microsoft", "windows", "currentversion"];
    if c.len() >= 4 && c[..3] == cv {
        return match (c[3].as_str(), c.len(), t.value.is_some()) {
            // A single uninstall entry or App Paths entry (whole key).
            ("uninstall", 5, false) | ("app paths", 5, false) => Ok(()),
            // Values in Run / RunOnce, never the keys themselves.
            ("run" | "runonce", 4, true) => Ok(()),
            _ => Err("windowsKey"),
        };
    }
    match c.first() {
        None => Err("topLevelKey"),
        Some(v) if FORBIDDEN_SUBTREES.contains(&v.as_str()) => Err("reservedVendor"),
        Some(v) if c.len() == 1 && SHARED_VENDORS.contains(&v.as_str()) => Err("sharedVendor"),
        Some(_) => Ok(()),
    }
}

pub fn key_exists(hive: Hive, path: &str, view: View) -> bool {
    Key::open(hive, path, view).is_some()
}

fn view_flag(v: View) -> u32 {
    match v {
        View::Default => 0,
        View::Reg64 => KEY_WOW64_64KEY,
        View::Reg32 => KEY_WOW64_32KEY,
    }
}

fn reg_err(code: u32, what: &str, t: &RegTarget) -> AppError {
    AppError::from_win32(code, what, Some(&t.display()))
}

/// Delete a key tree or a single value. Missing targets count as success.
pub fn delete(t: &RegTarget) -> Result<()> {
    if let Err(reason) = deletion_allowed(t) {
        return Err(AppError::Protected { path: t.display(), reason: reason.into() });
    }
    let (parent, name) = match (&t.value, t.path.rfind('\\')) {
        (Some(_), _) => (t.path.as_str(), ""),
        (None, Some(i)) => (&t.path[..i], &t.path[i + 1..]),
        (None, None) => return Err(AppError::Protected { path: t.display(), reason: "topLevelKey".into() }),
    };
    let pw = wide(parent);
    let mut h: HKEY = std::ptr::null_mut();
    let access = KEY_READ | KEY_SET_VALUE | 0x0001_0000 /* DELETE */ | view_flag(t.view);
    let rc = unsafe { RegOpenKeyExW(t.hive.raw(), pw.as_ptr(), 0, access, &mut h) };
    if rc == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if rc != ERROR_SUCCESS {
        return Err(reg_err(rc, "opening registry key", t));
    }
    struct Close(HKEY);
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }
    let _c = Close(h);
    if let Some(v) = &t.value {
        let vw = wide(v);
        let rc = unsafe { RegDeleteValueW(h, vw.as_ptr()) };
        return match rc {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            _ => Err(reg_err(rc, "deleting registry value", t)),
        };
    }
    let nw = wide(name);
    let rc = unsafe { RegDeleteTreeW(h, nw.as_ptr()) };
    if rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND {
        return Err(reg_err(rc, "deleting registry tree", t));
    }
    // RegDeleteTree leaves the (now empty) key on some versions: remove it.
    let rc = unsafe { RegDeleteKeyExW(h, nw.as_ptr(), view_flag(t.view), 0) };
    match rc {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        _ => Err(reg_err(rc, "deleting registry key", t)),
    }
}

// ---- .reg export (REGEDIT5, UTF-16LE) ---------------------------------------

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn hex_bytes(prefix: &str, data: &[u8]) -> String {
    let body: Vec<String> = data.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}:{}", body.join(","))
}

fn value_line(name: &str, ty: u32, data: &[u8]) -> String {
    let n = if name.is_empty() { "@".to_string() } else { format!("\"{}\"", escape(name)) };
    let v = match ty {
        REG_SZ => {
            let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
            let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            format!("\"{}\"", escape(&String::from_utf16_lossy(&units[..end])))
        }
        REG_DWORD if data.len() >= 4 => format!("dword:{:08x}", u32::from_le_bytes(data[..4].try_into().unwrap())),
        REG_BINARY => hex_bytes("hex", data),
        other => hex_bytes(&format!("hex({other:x})"), data),
    };
    format!("{n}={v}")
}

fn hive_name(h: Hive) -> &'static str {
    match h {
        Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
        Hive::CurrentUser => "HKEY_CURRENT_USER",
        Hive::ClassesRoot => "HKEY_CLASSES_ROOT",
        Hive::Users => "HKEY_USERS",
    }
}

fn export_key(out: &mut String, hive: Hive, path: &str, view: View, only_value: Option<&str>, depth: u32) {
    let Some(k) = Key::open(hive, path, view) else { return };
    // .reg files have no notion of registry views: 32-bit HKLM keys are
    // written with their physical WOW6432Node path so import restores them.
    let physical = if view == View::Reg32 && hive == Hive::LocalMachine && !path.to_lowercase().contains("wow6432node") {
        path.replacen("Software\\", "Software\\WOW6432Node\\", 1).replacen("SOFTWARE\\", "SOFTWARE\\WOW6432Node\\", 1)
    } else {
        path.to_string()
    };
    out.push_str(&format!("\r\n[{}\\{}]\r\n", hive_name(hive), physical));
    for name in k.value_names() {
        if only_value.is_some_and(|v| !v.eq_ignore_ascii_case(&name)) {
            continue;
        }
        if let Some((ty, data)) = k.value_raw(&name) {
            out.push_str(&value_line(&name, ty, &data));
            out.push_str("\r\n");
        }
    }
    if only_value.is_none() && depth < 64 {
        for sub in k.subkeys() {
            export_key(out, hive, &format!("{path}\\{sub}"), view, None, depth + 1);
        }
    }
}

/// Write a `.reg` backup of the targets (keys recursively, or single values).
/// Returns how many targets existed and were exported.
pub fn export(targets: &[RegTarget], file: &std::path::Path) -> Result<usize> {
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n");
    let mut n = 0;
    for t in targets {
        if !key_exists(t.hive, &t.path, t.view) {
            continue;
        }
        n += 1;
        export_key(&mut text, t.hive, &t.path, t.view, t.value.as_deref(), 0);
    }
    if let Some(dir) = file.parent() {
        crate::util::ensure_dir(dir)?;
    }
    let mut f = std::fs::File::create(file).map_err(|e| AppError::io("writing registry backup", Some(file), e))?;
    let mut bytes = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    f.write_all(&bytes).map_err(|e| AppError::io("writing registry backup", Some(file), e))?;
    f.sync_all().map_err(|e| AppError::io("writing registry backup", Some(file), e))?;
    Ok(n)
}

// ---- .reg import (restoring a backup) ---------------------------------------

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Keys that were created or opened to write into.
    pub keys: usize,
    pub values: usize,
    /// Values left alone, with the reason: outside the allowlist, a deletion
    /// directive, or a line this reader does not understand.
    pub skipped: Vec<String>,
}

fn decode_reg_file(raw: &[u8]) -> Result<String> {
    if raw.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = raw[2..].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        return String::from_utf16(&units).map_err(|_| AppError::Corrupt("registry backup is not valid UTF-16".into()));
    }
    let start = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    String::from_utf8(raw[start..].to_vec()).map_err(|_| AppError::Corrupt("registry backup is not valid text".into()))
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(n) = it.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_hive(name: &str) -> Option<Hive> {
    match name.to_ascii_uppercase().as_str() {
        "HKEY_LOCAL_MACHINE" | "HKLM" => Some(Hive::LocalMachine),
        "HKEY_CURRENT_USER" | "HKCU" => Some(Hive::CurrentUser),
        _ => None,
    }
}

/// `"name"="text"`, `@=dword:0000002a`, `"b"=hex(7):61,00,…` → the raw value.
fn parse_value(text: &str) -> Option<(u32, Vec<u8>)> {
    if let Some(rest) = text.strip_prefix('"') {
        let end = {
            let mut i = 0;
            let b: Vec<char> = rest.chars().collect();
            loop {
                if i >= b.len() {
                    return None;
                }
                if b[i] == '\\' {
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    break i;
                }
                i += 1;
            }
        };
        let s: String = rest.chars().take(end).collect();
        let mut data: Vec<u8> = unescape(&s).encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        data.extend_from_slice(&[0, 0]);
        return Some((REG_SZ, data));
    }
    if let Some(hex) = text.strip_prefix("dword:") {
        let n = u32::from_str_radix(hex.trim(), 16).ok()?;
        return Some((REG_DWORD, n.to_le_bytes().to_vec()));
    }
    let (ty, body) = if let Some(rest) = text.strip_prefix("hex(") {
        let (t, r) = rest.split_once("):")?;
        (u32::from_str_radix(t.trim(), 16).ok()?, r)
    } else {
        (REG_BINARY, text.strip_prefix("hex:")?)
    };
    let mut data = Vec::new();
    for part in body.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        data.push(u8::from_str_radix(p, 16).ok()?);
    }
    Some((ty, data))
}

fn write_value(t: &RegTarget, ty: u32, data: &[u8]) -> Result<()> {
    let pw = wide(&t.path);
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegCreateKeyExW(t.hive.raw(), pw.as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE | view_flag(t.view), std::ptr::null(), &mut h, std::ptr::null_mut())
    };
    if rc != ERROR_SUCCESS {
        return Err(reg_err(rc, "creating registry key", t));
    }
    struct Close(HKEY);
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }
    let _c = Close(h);
    let Some(name) = &t.value else { return Ok(()) };
    let nw = wide(name);
    let name_ptr = if name.is_empty() { std::ptr::null() } else { nw.as_ptr() };
    let rc = unsafe { RegSetValueExW(h, name_ptr, 0, ty, data.as_ptr(), data.len() as u32) };
    if rc != ERROR_SUCCESS {
        return Err(reg_err(rc, "writing registry value", t));
    }
    Ok(())
}

/// Restore a `.reg` backup written by [`export`].
///
/// Only what this app is allowed to delete may be written back, so a hand
/// edited (or foreign) file cannot turn a restore into a way to write
/// anywhere in the registry. Deletion directives (`[-HKEY…]`) are ignored:
/// restoring never removes anything.
pub fn import(file: &std::path::Path) -> Result<ImportReport> {
    let raw = std::fs::read(file).map_err(|e| AppError::io("reading registry backup", Some(file), e))?;
    if raw.len() > 64 * 1024 * 1024 {
        return Err(AppError::InvalidInput("registry backup too large".into()));
    }
    let text = decode_reg_file(&raw)?;
    let mut report = ImportReport::default();
    let mut current: Option<(Hive, String, View)> = None;
    let mut keys_written: Vec<String> = Vec::new();

    // Physical `WOW6432Node` paths are written to the 32-bit view with the
    // marker removed, so the value lands where it came from.
    let key_target = |hive: Hive, path: &str| -> (String, View) {
        let low = path.to_lowercase();
        match low.find("wow6432node\\") {
            Some(i) if hive == Hive::LocalMachine => (format!("{}{}", &path[..i], &path[i + "wow6432node\\".len()..]), View::Reg32),
            _ => (path.to_string(), View::Default),
        }
    };

    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        match lines.last_mut() {
            // A line ending in `\` continues on the next one (long hex values).
            Some(prev) if prev.ends_with('\\') && !prev.ends_with("\\\\") => {
                prev.pop();
                prev.push_str(line.trim_start());
            }
            _ => lines.push(line.to_string()),
        }
    }

    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with("Windows Registry Editor") || line.starts_with("REGEDIT") {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some(del) = inner.strip_prefix('-') {
                report.skipped.push(format!("{del} (deletion in the backup file)"));
                current = None;
                continue;
            }
            match inner.split_once('\\').and_then(|(h, p)| parse_hive(h).map(|hive| (hive, p))) {
                Some((hive, path)) => {
                    let (path, view) = key_target(hive, path);
                    current = Some((hive, path, view));
                }
                None => {
                    report.skipped.push(inner.to_string());
                    current = None;
                }
            }
            continue;
        }
        let Some((hive, path, view)) = current.clone() else { continue };
        let Some((raw_name, raw_value)) = line.split_once('=') else {
            report.skipped.push(line.to_string());
            continue;
        };
        let name = if raw_name.trim() == "@" { String::new() } else { unescape(raw_name.trim().trim_matches('"')) };
        let target = RegTarget { hive, path: path.clone(), view, value: Some(name.clone()) };
        if let Err(reason) = deletion_allowed(&target) {
            report.skipped.push(format!("{} ({reason})", target.display()));
            continue;
        }
        let Some((ty, data)) = parse_value(raw_value.trim()) else {
            report.skipped.push(format!("{} (value not understood)", target.display()));
            continue;
        };
        write_value(&target, ty, &data)?;
        report.values += 1;
        if !keys_written.contains(&path) {
            keys_written.push(path);
            report.keys += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(hive: Hive, path: &str, value: Option<&str>) -> RegTarget {
        RegTarget { hive, path: path.into(), view: View::Default, value: value.map(Into::into) }
    }

    #[test]
    fn allowlist() {
        use Hive::*;
        assert!(deletion_allowed(&t(CurrentUser, r"Software\SomeVendor\App", None)).is_ok());
        assert!(deletion_allowed(&t(LocalMachine, r"SOFTWARE\WOW6432Node\SomeVendor", None)).is_ok());
        assert!(deletion_allowed(&t(LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{X}", None)).is_ok());
        assert!(deletion_allowed(&t(CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Run", Some("App"))).is_ok());
        assert!(deletion_allowed(&t(CurrentUser, r"Software\Google\Chrome", None)).is_ok());
        for bad in [
            t(CurrentUser, r"Software", None),
            t(CurrentUser, r"Software\Microsoft", None),
            t(CurrentUser, r"Software\Microsoft\Office", None),
            t(LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", None),
            t(CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Run", None),
            t(LocalMachine, r"SYSTEM\CurrentControlSet\Services\x", None),
            t(LocalMachine, r"SOFTWARE\Classes\CLSID\x", None),
            t(LocalMachine, r"SOFTWARE\Policies\x", None),
            t(ClassesRoot, r"Software\x", None),
            t(CurrentUser, r"Software\x\..\Microsoft", None),
            t(CurrentUser, r"Software\Google", None),
        ] {
            assert!(deletion_allowed(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn export_and_delete_hkcu_sandbox() {
        let path = format!(r"Software\NexusTestVendor{}\RegOpsTest", std::process::id());
        let root = path.rsplit_once('\\').unwrap().0.to_string();
        let full = wide(format!(r"{path}\Sub"));
        let mut h: HKEY = std::ptr::null_mut();
        assert_eq!(unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, full.as_ptr(), 0, std::ptr::null(), 0, KEY_ALL_ACCESS, std::ptr::null(), &mut h, std::ptr::null_mut()) }, ERROR_SUCCESS);
        let name = wide("Greeting");
        let val = wide("olá \"mundo\"");
        unsafe {
            RegSetValueExW(h, name.as_ptr(), 0, REG_SZ, val.as_ptr() as *const u8, (val.len() * 2) as u32);
            let d: u32 = 42;
            let dn = wide("Answer");
            RegSetValueExW(h, dn.as_ptr(), 0, REG_DWORD, &d as *const u32 as *const u8, 4);
            RegCloseKey(h);
        }
        let target = t(Hive::CurrentUser, &root, None);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b.reg");
        assert_eq!(export(std::slice::from_ref(&target), &file).unwrap(), 1);
        let raw = std::fs::read(&file).unwrap();
        let units: Vec<u16> = raw[2..].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        let text = String::from_utf16(&units).unwrap();
        assert!(text.starts_with("Windows Registry Editor Version 5.00"));
        assert!(text.contains(r#""Greeting"="olá \"mundo\"""#));
        assert!(text.contains(r#""Answer"=dword:0000002a"#));
        assert!(text.contains(r"\RegOpsTest\Sub]"));

        delete(&target).unwrap();
        assert!(!key_exists(Hive::CurrentUser, &root, View::Default));
        delete(&target).unwrap(); // already gone: still Ok

        // Restoring the backup puts the values back exactly as they were.
        let report = import(&file).unwrap();
        assert_eq!(report.values, 2, "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        let k = Key::open(Hive::CurrentUser, &format!(r"{path}\Sub"), View::Default).expect("the subkey is back");
        assert_eq!(k.string("Greeting").as_deref(), Some("olá \"mundo\""));
        assert_eq!(k.dword("Answer"), Some(42));
        delete(&target).unwrap();
    }

    #[test]
    fn import_refuses_what_deletion_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("evil.reg");
        let text = r#"Windows Registry Editor Version 5.00

[HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies]
"Anything"=dword:00000001

[-HKEY_CURRENT_USER\Software\Whatever]
"#;
        let mut bytes = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&file, bytes).unwrap();
        let report = import(&file).unwrap();
        assert_eq!(report.values, 0);
        assert_eq!(report.skipped.len(), 2, "{report:?}");
    }
}
