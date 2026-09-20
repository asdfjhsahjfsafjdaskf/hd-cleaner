//! Running official uninstallers.
//!
//! * MSI: `%SystemRoot%\System32\msiexec.exe /x {ProductCode}` built from a
//!   validated GUID — the registry string is not used at all.
//! * Others: the registered command is split into executable + parameters and
//!   started with `ShellExecuteExW` (no `cmd.exe`, no string concatenation;
//!   UAC appears when the uninstaller's manifest requires it).
//! * AppX/Store: `PackageManager.RemovePackageAsync` for the current user.
//!
//! Many uninstallers (Inno Setup, NSIS) copy themselves to %TEMP% and exit
//! immediately, so we wait for the whole process tree to finish.

use crate::programs::{Program, ProgramSource};
use crate::registry::{Hive, View};
use crate::util::{wide, OwnedHandle};
use crate::{AppError, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UninstallCommand {
    /// Executable to start (absolute path).
    pub file: String,
    /// Parameters exactly as they will be passed.
    pub params: String,
    pub quiet: bool,
    /// "msi" | "registry" | "appx"
    pub kind: &'static str,
}

fn valid_guid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && s[1..37].char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// Split a registered command line into (exe, params).
pub fn split_command(cmd: &str) -> Option<(String, String)> {
    let c = crate::registry::expand_env(cmd.trim());
    let (exe, rest) = if let Some(r) = c.strip_prefix('"') {
        let end = r.find('"')?;
        (r[..end].to_string(), r[end + 1..].to_string())
    } else {
        let lower = c.to_ascii_lowercase();
        let end = lower.find(".exe").map(|i| i + 4)?;
        (c[..end].to_string(), c[end..].to_string())
    };
    let exe = exe.trim().to_string();
    let b = exe.as_bytes();
    if !(b.len() >= 3 && b[1] == b':' && b[2] == b'\\') {
        return None;
    }
    Some((exe, rest.trim().to_string()))
}

/// How the program would be uninstalled. Errors explain why it cannot be.
pub fn command_for(p: &Program, quiet: bool) -> Result<UninstallCommand> {
    if matches!(p.source, ProgramSource::Appx | ProgramSource::Store) {
        let full = p.package_full_name.clone().ok_or_else(|| AppError::InvalidInput("package without full name".into()))?;
        if p.signature_kind.as_deref() == Some("system") {
            return Err(AppError::Protected { path: full, reason: "systemPackage".into() });
        }
        return Ok(UninstallCommand { file: String::new(), params: full, quiet: true, kind: "appx" });
    }
    if p.no_remove {
        return Err(AppError::NotSupported("the installer disabled removal for this program (NoRemove)".into()));
    }
    if let Some(code) = p.msi_product_code.as_deref().filter(|c| valid_guid(c)) {
        let msiexec = format!(r"{}\System32\msiexec.exe", crate::system::windows_dir());
        let params = if quiet { format!("/x {code} /passive /norestart") } else { format!("/x {code}") };
        return Ok(UninstallCommand { file: msiexec, params, quiet, kind: "msi" });
    }
    let registered = if quiet { p.quiet_uninstall_string.as_deref() } else { None }.or(p.uninstall_string.as_deref());
    let (raw, is_quiet) = match registered {
        Some(r) => (r, quiet && p.quiet_uninstall_string.is_some()),
        None => return Err(AppError::NotSupported("no uninstall command is registered for this program".into())),
    };
    let (file, params) = split_command(raw)
        .ok_or_else(|| AppError::InvalidInput(format!("the registered uninstall command could not be parsed: {raw}")))?;
    if !std::path::Path::new(&file).is_file() {
        return Err(AppError::NotFound { path: file });
    }
    Ok(UninstallCommand { file, params, quiet: is_quiet, kind: "registry" })
}

/// Still registered as installed? (entry present in the registry / package list)
pub fn still_installed(p: &Program) -> bool {
    if let Some(full) = &p.package_full_name {
        return crate::appx::packages().map(|l| l.iter().any(|x| x.package_full_name.as_ref() == Some(full))).unwrap_or(true);
    }
    let Some(id) = p.id.strip_prefix("reg:") else { return false };
    let Some((loc, sub)) = id.split_once(':') else { return false };
    let (hive, view) = match loc {
        "HKLM64" => (Hive::LocalMachine, View::Reg64),
        "HKLM32" => (Hive::LocalMachine, View::Reg32),
        "HKLM" => (Hive::LocalMachine, View::Default),
        _ => (Hive::CurrentUser, View::Default),
    };
    crate::regops::key_exists(hive, &format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{sub}"), view)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    pub exit_code: Option<u32>,
    /// Processes observed in the uninstaller's tree.
    pub processes: Vec<String>,
    pub still_installed: bool,
    /// The user stopped waiting (processes may still be running).
    pub stopped_waiting: bool,
    pub duration_ms: u64,
}

fn snapshot() -> Vec<(u32, u32, String)> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = Vec::new();
    unsafe {
        let h = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        let Some(h) = OwnedHandle::new(h) else { return out };
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(h.raw(), &mut e) != 0 {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
                if Process32NextW(h.raw(), &mut e) == 0 {
                    break;
                }
            }
        }
    }
    out
}

/// Start the uninstaller and wait for it and every descendant process.
/// `on_progress` receives the names of the processes currently running.
pub fn run(p: &Program, cmd: &UninstallCommand, stop: &AtomicBool, mut on_progress: impl FnMut(&[String])) -> Result<RunOutcome> {
    let t0 = std::time::Instant::now();
    if cmd.kind == "appx" {
        remove_package(&cmd.params)?;
        return Ok(RunOutcome {
            exit_code: Some(0),
            processes: vec![],
            still_installed: still_installed(p),
            stopped_waiting: false,
            duration_ms: t0.elapsed().as_millis() as u64,
        });
    }
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::*;
    let verb = wide("open");
    let file = wide(&cmd.file);
    let params = wide(&cmd.params);
    let dir = std::path::Path::new(&cmd.file).parent().map(|d| wide(d.as_os_str())).unwrap_or_else(|| wide(""));
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = if cmd.params.is_empty() { std::ptr::null() } else { params.as_ptr() };
    sei.lpDirectory = dir.as_ptr();
    sei.nShow = windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if unsafe { ShellExecuteExW(&mut sei) } == 0 {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if code == windows_sys::Win32::Foundation::ERROR_CANCELLED {
            return Err(AppError::ElevationRequired("the administrator prompt for the uninstaller was declined".into()));
        }
        return Err(AppError::from_win32(code, "starting the uninstaller", Some(&cmd.file)));
    }
    let root = OwnedHandle::new(sei.hProcess).ok_or_else(|| AppError::Helper("uninstaller started without a process handle".into()))?;
    let root_pid = unsafe { GetProcessId(root.raw()) };

    // Track the tree: any process whose parent is tracked becomes tracked.
    let mut tracked: HashMap<u32, String> = HashMap::new();
    tracked.insert(root_pid, std::path::Path::new(&cmd.file).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default());
    let mut seen: Vec<String> = Vec::new();
    let mut idle_polls = 0;
    let mut stopped = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            stopped = true;
            break;
        }
        let procs = snapshot();
        let alive: HashSet<u32> = procs.iter().map(|p| p.0).collect();
        let mut grew = true;
        while grew {
            grew = false;
            for (pid, ppid, name) in &procs {
                if tracked.contains_key(ppid) && !tracked.contains_key(pid) {
                    tracked.insert(*pid, name.clone());
                    grew = true;
                }
            }
        }
        let running: Vec<String> = tracked
            .iter()
            .filter(|(pid, _)| alive.contains(pid) && (**pid != root_pid || unsafe { WaitForSingleObject(root.raw(), 0) } != 0))
            .map(|(_, n)| n.clone())
            .collect();
        for n in &running {
            if !seen.contains(n) {
                seen.push(n.clone());
            }
        }
        on_progress(&running);
        if running.is_empty() {
            idle_polls += 1;
            // Two quiet polls: nothing spawned right as the parent exited.
            if idle_polls >= 2 {
                break;
            }
        } else {
            idle_polls = 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let mut code = 0u32;
    let exit_code = (unsafe { GetExitCodeProcess(root.raw(), &mut code) } != 0 && code != 259).then_some(code);
    Ok(RunOutcome {
        exit_code,
        processes: seen,
        still_installed: still_installed(p),
        stopped_waiting: stopped,
        duration_ms: t0.elapsed().as_millis() as u64,
    })
}

fn remove_package(full_name: &str) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Management::Deployment::{PackageManager, RemovalOptions};
    let map = |e: windows::core::Error| AppError::Win32 { code: e.code().0 as u32, context: format!("removing package: {}", e.message()), path: Some(full_name.into()) };
    let pm = PackageManager::new().map_err(map)?;
    let op = pm.RemovePackageWithOptionsAsync(&HSTRING::from(full_name), RemovalOptions::None).map_err(map)?;
    // Blocks until the deployment completes (callers run this on a worker thread).
    let result = op.join().map_err(map)?;
    let hr = result.ExtendedErrorCode().map_err(map)?;
    if hr.is_err() {
        let text = result.ErrorText().map(|t| t.to_string()).unwrap_or_default();
        return Err(AppError::Win32 { code: hr.0 as u32, context: format!("removing package: {text}"), path: Some(full_name.into()) });
    }
    Ok(())
}

/// System Restore point (requires administrator rights; runs in the helper).
pub fn create_restore_point(description: &str) -> Result<()> {
    use windows_sys::Win32::System::Restore::*;
    let mut info: RESTOREPOINTINFOW = unsafe { std::mem::zeroed() };
    info.dwEventType = BEGIN_SYSTEM_CHANGE;
    info.dwRestorePtType = APPLICATION_UNINSTALL;
    let desc: String = description.chars().filter(|c| !c.is_control()).take(60).collect();
    for (i, u) in desc.encode_utf16().take(255).enumerate() {
        info.szDescription[i] = u;
    }
    let mut status: STATEMGRSTATUS = unsafe { std::mem::zeroed() };
    if unsafe { SRSetRestorePointW(&info, &mut status) } == 0 {
        return Err(AppError::from_win32(status.nStatus, "creating a restore point (System Restore may be disabled)", None));
    }
    let mut end: RESTOREPOINTINFOW = unsafe { std::mem::zeroed() };
    end.dwEventType = END_SYSTEM_CHANGE;
    end.llSequenceNumber = status.llSequenceNumber;
    let mut status2: STATEMGRSTATUS = unsafe { std::mem::zeroed() };
    unsafe { SRSetRestorePointW(&end, &mut status2) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(split_command(r#""C:\P F\unins000.exe" /SILENT"#).unwrap(), (r"C:\P F\unins000.exe".into(), "/SILENT".into()));
        assert_eq!(split_command(r"C:\x\Update.exe --uninstall -s").unwrap(), (r"C:\x\Update.exe".into(), "--uninstall -s".into()));
        assert!(split_command("relative.exe /x").is_none());
        assert!(split_command("no exe here").is_none());
        assert!(valid_guid("{12345678-ABCD-1234-abcd-1234567890AB}"));
        assert!(!valid_guid("{12345678-ABCD-1234-abcd-1234567890AB} & calc"));

        let mut p = Program::blank("reg:HKLM64:{12345678-ABCD-1234-abcd-1234567890AB}".into(), "X".into(), ProgramSource::Msi);
        p.msi_product_code = Some("{12345678-ABCD-1234-abcd-1234567890AB}".into());
        p.uninstall_string = Some("MsiExec.exe /I{whatever} & evil".into());
        let c = command_for(&p, false).unwrap();
        assert!(c.file.to_lowercase().ends_with(r"system32\msiexec.exe"));
        assert_eq!(c.params, "/x {12345678-ABCD-1234-abcd-1234567890AB}");

        let mut q = Program::blank("reg:HKCU:missing".into(), "Y".into(), ProgramSource::Win32);
        q.uninstall_string = Some(r#""C:\definitely\missing\uninst.exe" /S"#.into());
        assert!(matches!(command_for(&q, false), Err(AppError::NotFound { .. })));
        q.no_remove = true;
        assert!(command_for(&q, false).is_err());
    }
}
