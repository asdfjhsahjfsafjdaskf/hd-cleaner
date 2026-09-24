//! The "Analyze with HD Cleaner" entry in Explorer's context menu.
//!
//! Everything lives under `HKCU\Software\Classes`, so no administrator is
//! needed and nothing machine-wide is touched: the entry belongs to this user
//! and disappears completely when turned off. On Windows 11 it shows under
//! "Show more options", which is where classic entries go.

use crate::util::wide;
use crate::{AppError, Result};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::*;

/// Key name used under each class; also what identifies our own entry.
const KEY: &str = "HDCleanerAnalyze";

/// Where the entry is added: folders, the background of an open folder, and
/// drives. The second field is the argument the command receives (`%1` is the
/// clicked item, `%V` the folder you are looking at).
const TARGETS: &[(&str, &str)] = &[
    (r"Software\Classes\Directory\shell", "%1"),
    (r"Software\Classes\Directory\Background\shell", "%V"),
    (r"Software\Classes\Drive\shell", "%1"),
];

fn set_string(key: HKEY, name: Option<&str>, value: &str) -> Result<()> {
    let n = name.map(wide);
    let v = wide(value);
    let rc = unsafe {
        RegSetValueExW(
            key,
            n.as_ref().map(|x| x.as_ptr()).unwrap_or(std::ptr::null()),
            0,
            REG_SZ,
            v.as_ptr() as *const u8,
            (v.len() * 2) as u32,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(AppError::from_win32(rc, "writing the context menu entry", Some(value)));
    }
    Ok(())
}

fn create(path: &str) -> Result<HKEY> {
    let p = wide(path);
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, p.as_ptr(), 0, std::ptr::null(), 0, KEY_WRITE, std::ptr::null(), &mut h, std::ptr::null_mut())
    };
    if rc != ERROR_SUCCESS {
        return Err(AppError::from_win32(rc, "creating the context menu key", Some(path)));
    }
    Ok(h)
}

/// Is the entry there right now?
pub fn installed() -> bool {
    TARGETS.iter().all(|(base, _)| {
        crate::registry::Key::open(crate::registry::Hive::CurrentUser, &format!(r"{base}\{KEY}\command"), crate::registry::View::Default).is_some()
    })
}

/// Add the entry, pointing at this executable.
pub fn install(label: &str) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| AppError::io("finding this program", None, e))?;
    let exe = exe.to_string_lossy().to_string();
    for (base, arg) in TARGETS {
        let path = format!(r"{base}\{KEY}");
        let key = create(&path)?;
        let r = set_string(key, None, label).and_then(|_| set_string(key, Some("Icon"), &format!("\"{exe}\",0")));
        unsafe { RegCloseKey(key) };
        r?;

        let cmd_key = create(&format!(r"{path}\command"))?;
        let r = set_string(cmd_key, None, &format!("\"{exe}\" \"{arg}\""));
        unsafe { RegCloseKey(cmd_key) };
        r?;
    }
    Ok(())
}

/// Remove the entry. A missing entry counts as success.
pub fn uninstall() -> Result<()> {
    for (base, _) in TARGETS {
        let path = wide(format!(r"{base}\{KEY}"));
        let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
        if rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND {
            return Err(AppError::from_win32(rc, "removing the context menu entry", Some(base)));
        }
        let rc = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, path.as_ptr()) };
        if rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND {
            return Err(AppError::from_win32(rc, "removing the context menu entry", Some(base)));
        }
    }
    Ok(())
}

/// A path handed to the app by Explorer (or any other caller) that is worth
/// scanning. Anything that is not an existing folder or drive is ignored.
pub fn path_from_args(args: &[String]) -> Option<String> {
    args.iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(|a| a.trim_matches('"').to_string())
        .filter(|a| std::path::Path::new(a).is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "modifies the current user's Explorer context menu"]
    fn install_and_uninstall_round_trip() {
        // Whatever the machine started with, it ends the same way.
        let was = installed();
        install("Analyze with HD Cleaner (test)").unwrap();
        assert!(installed(), "the entry is there after installing");

        let key = crate::registry::Key::open(
            crate::registry::Hive::CurrentUser,
            &format!(r"Software\Classes\Directory\shell\{KEY}\command"),
            crate::registry::View::Default,
        )
        .unwrap();
        let cmd = key.string("").unwrap_or_default();
        assert!(cmd.contains("%1"), "the clicked folder is passed: {cmd}");
        assert!(cmd.starts_with('"'), "the exe path is quoted: {cmd}");

        uninstall().unwrap();
        assert!(!installed());
        uninstall().unwrap(); // removing twice is fine
        if was {
            install("Analyze with HD Cleaner").unwrap();
        }
    }

    #[test]
    fn only_a_real_folder_is_taken_from_the_arguments() {
        let exe = "C:\\app.exe".to_string();
        let tmp = std::env::temp_dir().to_string_lossy().into_owned();
        assert_eq!(path_from_args(&[exe.clone(), tmp.clone()]).as_deref(), Some(tmp.as_str()));
        assert!(path_from_args(&[exe.clone(), r"C:\nope\not\here".into()]).is_none());
        assert!(path_from_args(&[exe.clone(), "--flag".into()]).is_none());
        assert!(path_from_args(&[exe]).is_none());
    }
}
