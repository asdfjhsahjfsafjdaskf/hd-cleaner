//! Shell shortcuts (.lnk): resolve targets without launching anything.

use crate::{AppError, Result};
use std::path::{Path, PathBuf};

/// Start Menu / Desktop folders of the current user and all users.
pub fn shortcut_roots() -> Vec<PathBuf> {
    let env = |v: &str| std::env::var(v).ok().filter(|s| !s.is_empty()).map(PathBuf::from);
    let mut out = Vec::new();
    if let Some(a) = env("APPDATA") {
        out.push(a.join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(p) = env("ProgramData") {
        out.push(p.join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(u) = env("USERPROFILE") {
        out.push(u.join("Desktop"));
    }
    if let Some(p) = env("PUBLIC") {
        out.push(p.join("Desktop"));
    }
    out
}

/// All `.lnk` files below the roots (bounded depth/count).
pub fn find_shortcuts(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let mut stack = vec![(root.clone(), 0u32)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                let p = e.path();
                if ft.is_dir() && !ft.is_symlink() && depth < 6 {
                    stack.push((p, depth + 1));
                } else if ft.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                    out.push(p);
                }
                if out.len() > 20_000 {
                    return out;
                }
            }
        }
    }
    out
}

/// Resolve targets of many shortcuts on one STA thread. `None` = unreadable
/// or no file-system target (e.g. shell namespace links).
pub fn resolve_targets(links: &[PathBuf]) -> Vec<Option<String>> {
    let links = links.to_vec();
    std::thread::spawn(move || resolve_sta(&links)).join().unwrap_or_default()
}

fn resolve_sta(links: &[PathBuf]) -> Vec<Option<String>> {
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let out = (|| -> Option<Vec<Option<String>>> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let file: IPersistFile = link.cast().ok()?;
            Some(
                links
                    .iter()
                    .map(|p| {
                        let path = HSTRING::from(p.as_os_str());
                        file.Load(PCWSTR(path.as_ptr()), STGM_READ).ok()?;
                        let mut buf = [0u16; 1024];
                        link.GetPath(&mut buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32).ok()?;
                        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
                        (n > 0).then(|| crate::registry::expand_env(&String::from_utf16_lossy(&buf[..n])))
                    })
                    .collect(),
            )
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        out.unwrap_or_else(|| vec![None; links.len()])
    }
}

pub fn resolve_target(link: &Path) -> Result<String> {
    resolve_targets(&[link.to_path_buf()])
        .pop()
        .flatten()
        .ok_or_else(|| AppError::NotFound { path: link.display().to_string() })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Create a .lnk pointing at `target` (test helper, also used by fixtures).
    pub fn create_shortcut(link: &Path, target: &str) {
        use windows::core::{Interface, HSTRING, PCWSTR};
        use windows::Win32::System::Com::*;
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        let (link, target) = (link.to_path_buf(), target.to_string());
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            sl.SetPath(PCWSTR(HSTRING::from(target).as_ptr())).unwrap();
            let pf: IPersistFile = sl.cast().unwrap();
            pf.Save(PCWSTR(HSTRING::from(link.as_os_str()).as_ptr()), true).unwrap();
            CoUninitialize();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn resolves_created_shortcut() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app.exe");
        std::fs::write(&target, b"x").unwrap();
        let link = dir.path().join("App.lnk");
        create_shortcut(&link, target.to_str().unwrap());
        assert_eq!(resolve_target(&link).unwrap().to_lowercase(), target.to_string_lossy().to_lowercase());
        let found = find_shortcuts(&[dir.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
    }
}
