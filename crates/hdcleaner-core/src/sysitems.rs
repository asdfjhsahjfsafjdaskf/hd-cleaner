//! Startup entries (Run keys), services and scheduled tasks: enumeration for
//! leftover detection, plus the narrowly-scoped delete operations used by the
//! leftover remover (services/tasks normally need the elevated helper).

use crate::programs::exe_from_command;
use crate::registry::{Hive, Key, View};
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunEntry {
    pub hive: Hive,
    pub view: View,
    /// `Software\Microsoft\Windows\CurrentVersion\Run` or `...\RunOnce`
    pub key: String,
    pub name: String,
    pub command: String,
    pub exe: Option<String>,
}

pub fn run_entries() -> Vec<RunEntry> {
    let mut out = Vec::new();
    let keys = [r"Software\Microsoft\Windows\CurrentVersion\Run", r"Software\Microsoft\Windows\CurrentVersion\RunOnce"];
    let sources = [
        (Hive::CurrentUser, View::Default),
        (Hive::LocalMachine, View::Reg64),
        (Hive::LocalMachine, View::Reg32),
    ];
    for (hive, view) in sources {
        for key in keys {
            let Some(k) = Key::open(hive, key, view) else { continue };
            for name in k.value_names() {
                let Some(command) = k.string(&name) else { continue };
                let exe = exe_from_command(&command);
                let e = RunEntry { hive, view, key: key.to_string(), name, command, exe };
                // The 32/64-bit HKLM views can alias the same key for Run.
                if !out.contains(&e) {
                    out.push(e);
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: Option<String>,
    pub image_path: Option<String>,
    pub exe: Option<String>,
    /// 0 boot, 1 system, 2 automatic, 3 manual, 4 disabled.
    pub start: Option<u32>,
    pub delayed: bool,
    /// SERVICE_TYPE bits (0x10/0x20 = Win32 service; 1/2 = drivers).
    pub service_type: u32,
    pub description: Option<String>,
}

/// Normalise a service ImagePath (`\??\C:\x`, `system32\drivers\x.sys`, quotes, args).
pub fn service_exe(image: &str) -> Option<String> {
    let s = image.trim().trim_start_matches(r"\??\");
    if s.to_lowercase().starts_with("system32\\") || s.to_lowercase().starts_with(r"\systemroot\") {
        let rest = s.trim_start_matches(r"\SystemRoot\").trim_start_matches(r"\systemroot\");
        return Some(format!("{}\\{}", crate::system::windows_dir(), rest.split(' ').next().unwrap_or(rest)));
    }
    exe_from_command(s).or_else(|| crate::programs::clean_dir(s.split(" -").next().unwrap_or(s)))
}

pub fn services() -> Vec<ServiceInfo> {
    let Some(root) = Key::open(Hive::LocalMachine, r"SYSTEM\CurrentControlSet\Services", View::Default) else {
        return Vec::new();
    };
    root.subkeys()
        .into_iter()
        .filter_map(|name| {
            let k = root.open_sub(&name, View::Default)?;
            let image = k.string("ImagePath");
            let exe = image.as_deref().and_then(service_exe);
            Some(ServiceInfo {
                display_name: k.string("DisplayName"),
                image_path: image,
                exe,
                start: k.dword("Start"),
                delayed: k.dword("DelayedAutostart") == Some(1),
                service_type: k.dword("Type").unwrap_or(0),
                description: k.string("Description"),
                name,
            })
        })
        .collect()
}

/// Stop and delete a service (requires administrator rights). The current
/// ImagePath must still match what the user reviewed, and services whose
/// binary lives in the Windows directory are never touched.
pub fn delete_service(name: &str, expected_image: &str) -> Result<()> {
    use crate::util::wide;
    use windows_sys::Win32::System::Services::*;
    if name.is_empty() || name.contains(['\\', '/']) || name.len() > 256 {
        return Err(AppError::InvalidInput("invalid service name".into()));
    }
    let current = Key::open(Hive::LocalMachine, &format!(r"SYSTEM\CurrentControlSet\Services\{name}"), View::Default)
        .and_then(|k| k.string("ImagePath"))
        .ok_or_else(|| AppError::NotFound { path: format!("service {name}") })?;
    if !current.eq_ignore_ascii_case(expected_image) {
        return Err(AppError::ChangedSinceReview { path: format!("service {name}") });
    }
    let exe = service_exe(&current).unwrap_or_default();
    let win = crate::system::windows_dir().to_lowercase();
    if exe.is_empty() || exe.to_lowercase().starts_with(&win) {
        return Err(AppError::Protected { path: format!("service {name}"), reason: "windowsService".into() });
    }
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return Err(AppError::last_win32("opening service manager", None));
        }
        let n = wide(name);
        let svc = OpenServiceW(scm, n.as_ptr(), SERVICE_STOP | SERVICE_QUERY_STATUS | 0x0001_0000 /* DELETE */);
        if svc.is_null() {
            let e = AppError::last_win32("opening service", Some(name));
            CloseServiceHandle(scm);
            return Err(e);
        }
        let mut status: SERVICE_STATUS = std::mem::zeroed();
        let _ = ControlService(svc, SERVICE_CONTROL_STOP, &mut status);
        let ok = DeleteService(svc) != 0;
        let err = (!ok).then(|| AppError::last_win32("deleting service", Some(name)));
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    /// Full task path, e.g. `\Vendor\Updater`.
    pub path: String,
    pub name: String,
    pub exes: Vec<String>,
    /// First action as a command line (path + arguments).
    pub command: String,
    pub enabled: bool,
    /// "logon" / "boot" when the task starts with Windows or the user session.
    pub startup_trigger: Option<&'static str>,
}

/// Scheduled tasks visible to the current user, excluding `\Microsoft\`.
pub fn tasks() -> Vec<TaskInfo> {
    std::thread::spawn(tasks_sta).join().unwrap_or_default()
}

fn tasks_sta() -> Vec<TaskInfo> {
    use windows::core::{Interface, BSTR};
    use windows::Win32::System::Com::*;
    use windows::Win32::System::TaskScheduler::*;
    use windows::Win32::System::Variant::VARIANT;
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut out = Vec::new();
        let mut run = || -> windows::core::Result<()> {
            let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            svc.Connect(&VARIANT::default(), &VARIANT::default(), &VARIANT::default(), &VARIANT::default())?;
            let mut stack = vec![svc.GetFolder(&BSTR::from("\\"))?];
            while let Some(folder) = stack.pop() {
                let fpath = folder.Path()?.to_string();
                if fpath.to_lowercase().starts_with(r"\microsoft") {
                    continue;
                }
                if let Ok(tasks) = folder.GetTasks(TASK_ENUM_HIDDEN.0) {
                    for i in 1..=tasks.Count()? {
                        let Ok(t) = tasks.get_Item(&VARIANT::from(i)) else { continue };
                        let mut exes = Vec::new();
                        let mut command = String::new();
                        let mut startup_trigger = None;
                        let def = t.Definition();
                        if let Ok(actions) = def.as_ref().map_err(Clone::clone).and_then(|d| d.Actions()) {
                            let mut count = 0i32;
                            let _ = actions.Count(&mut count);
                            for j in 1..=count {
                                let Ok(a) = actions.get_Item(j) else { continue };
                                if let Ok(exec) = a.cast::<IExecAction>() {
                                    let mut p = BSTR::new();
                                    if exec.Path(&mut p).is_ok() {
                                        let s = crate::registry::expand_env(p.to_string().trim_matches('"'));
                                        if !s.is_empty() {
                                            if command.is_empty() {
                                                let mut args = BSTR::new();
                                                let _ = exec.Arguments(&mut args);
                                                let args = args.to_string();
                                                command = if args.is_empty() { format!("\"{s}\"") } else { format!("\"{s}\" {args}") };
                                            }
                                            exes.push(s);
                                        }
                                    }
                                }
                            }
                        }
                        if let Ok(triggers) = def.and_then(|d| d.Triggers()) {
                            let mut count = 0i32;
                            let _ = triggers.Count(&mut count);
                            for j in 1..=count {
                                let Ok(tr) = triggers.get_Item(j) else { continue };
                                let mut ty = TASK_TRIGGER_TYPE2(0);
                                if tr.Type(&mut ty).is_ok() {
                                    if ty == TASK_TRIGGER_LOGON {
                                        startup_trigger = Some("logon");
                                    } else if ty == TASK_TRIGGER_BOOT && startup_trigger.is_none() {
                                        startup_trigger = Some("boot");
                                    }
                                }
                            }
                        }
                        let enabled = t.Enabled().map(|b| b.as_bool()).unwrap_or(true);
                        out.push(TaskInfo { path: t.Path()?.to_string(), name: t.Name()?.to_string(), exes, command, enabled, startup_trigger });
                    }
                }
                if let Ok(subs) = folder.GetFolders(0) {
                    for i in 1..=subs.Count()? {
                        if let Ok(f) = subs.get_Item(&VARIANT::from(i)) {
                            stack.push(f);
                        }
                    }
                }
            }
            Ok(())
        };
        if let Err(e) = run() {
            tracing::info!("scheduled tasks not enumerated: {e}");
        }
        if init.is_ok() {
            CoUninitialize();
        }
        out
    }
}

fn check_task_path(path: &str) -> Result<()> {
    if !path.starts_with('\\') || path.to_lowercase().starts_with(r"\microsoft\") || path.contains("..") || path.len() > 512 {
        return Err(AppError::Protected { path: path.into(), reason: "windowsTask".into() });
    }
    Ok(())
}

/// Run `f` with the task folder and task name on a COM thread.
fn with_task<T: Send + 'static>(
    path: &str,
    what: &'static str,
    f: impl FnOnce(&windows::Win32::System::TaskScheduler::ITaskFolder, &str) -> windows::core::Result<T> + Send + 'static,
) -> Result<T> {
    check_task_path(path)?;
    let path = path.to_string();
    std::thread::spawn(move || -> Result<T> {
        use windows::core::BSTR;
        use windows::Win32::System::Com::*;
        use windows::Win32::System::TaskScheduler::*;
        use windows::Win32::System::Variant::VARIANT;
        let map = |e: windows::core::Error| match e.code().0 as u32 {
            0x8007_0005 => AppError::AccessDenied { path: path.clone() },
            0x8007_0002 => AppError::NotFound { path: path.clone() },
            code => AppError::Win32 { code, context: format!("{what}: {}", e.message()), path: Some(path.clone()) },
        };
        unsafe {
            let init = CoInitializeEx(None, COINIT_MULTITHREADED);
            let r = (|| -> windows::core::Result<T> {
                let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
                svc.Connect(&VARIANT::default(), &VARIANT::default(), &VARIANT::default(), &VARIANT::default())?;
                let (parent, name) = path.rsplit_once('\\').unwrap_or(("", &path));
                let folder = svc.GetFolder(&BSTR::from(if parent.is_empty() { "\\" } else { parent }))?;
                f(&folder, name)
            })();
            if init.is_ok() {
                CoUninitialize();
            }
            r.map_err(map)
        }
    })
    .join()
    .unwrap_or_else(|_| Err(AppError::Helper("task thread panicked".into())))
}

/// Delete a scheduled task (outside `\Microsoft\`).
pub fn delete_task(path: &str) -> Result<()> {
    with_task(path, "deleting scheduled task", |folder, name| unsafe { folder.DeleteTask(&windows::core::BSTR::from(name), 0) })
}

/// Enable or disable a scheduled task (outside `\Microsoft\`).
pub fn set_task_enabled(path: &str, enabled: bool) -> Result<()> {
    with_task(path, "changing scheduled task", move |folder, name| unsafe {
        folder.GetTask(&windows::core::BSTR::from(name))?.SetEnabled(enabled.into())
    })
}

/// The task definition as XML (kept as a backup before removal).
pub fn task_xml(path: &str) -> Result<String> {
    with_task(path, "reading scheduled task", |folder, name| unsafe {
        Ok(folder.GetTask(&windows::core::BSTR::from(name))?.Xml()?.to_string())
    })
}

/// SERVICE_AUTO_START / SERVICE_DEMAND_START / SERVICE_DISABLED.
pub const START_AUTO: u32 = 2;
pub const START_MANUAL: u32 = 3;
pub const START_DISABLED: u32 = 4;

/// Switch a third-party service between automatic and manual start
/// (requires administrator rights). The ImagePath must still be what the
/// user reviewed; drivers and Windows services are refused.
pub fn set_service_start(name: &str, expected_image: &str, auto: bool) -> Result<()> {
    use crate::util::wide;
    use windows_sys::Win32::System::Services::*;
    if name.is_empty() || name.contains(['\\', '/']) || name.len() > 256 {
        return Err(AppError::InvalidInput("invalid service name".into()));
    }
    let key = Key::open(Hive::LocalMachine, &format!(r"SYSTEM\CurrentControlSet\Services\{name}"), View::Default)
        .ok_or_else(|| AppError::NotFound { path: format!("service {name}") })?;
    let current = key.string("ImagePath").ok_or_else(|| AppError::NotFound { path: format!("service {name}") })?;
    if !current.eq_ignore_ascii_case(expected_image) {
        return Err(AppError::ChangedSinceReview { path: format!("service {name}") });
    }
    let exe = service_exe(&current).unwrap_or_default();
    let win = crate::system::windows_dir().to_lowercase();
    let ty = key.dword("Type").unwrap_or(0);
    if exe.is_empty() || exe.to_lowercase().starts_with(&win) || ty & 0x30 == 0 {
        return Err(AppError::Protected { path: format!("service {name}"), reason: "windowsService".into() });
    }
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return Err(AppError::last_win32("opening service manager", None));
        }
        let n = wide(name);
        let svc = OpenServiceW(scm, n.as_ptr(), SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG);
        if svc.is_null() {
            let e = AppError::last_win32("opening service", Some(name));
            CloseServiceHandle(scm);
            return Err(e);
        }
        let start = if auto { SERVICE_AUTO_START } else { SERVICE_DEMAND_START };
        let ok = ChangeServiceConfigW(
            svc,
            SERVICE_NO_CHANGE,
            start,
            SERVICE_NO_CHANGE,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        ) != 0;
        let err = (!ok).then(|| AppError::last_win32("changing service start type", Some(name)));
        CloseServiceHandle(svc);
        CloseServiceHandle(scm);
        err.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_paths() {
        let win = crate::system::windows_dir();
        assert_eq!(service_exe(r"\??\C:\Program Files\X\svc.exe").unwrap(), r"C:\Program Files\X\svc.exe");
        assert_eq!(service_exe(r#""C:\Program Files\X\svc.exe" -k run"#).unwrap(), r"C:\Program Files\X\svc.exe");
        assert_eq!(service_exe(r"system32\drivers\x.sys").unwrap().to_lowercase(), format!(r"{win}\system32\drivers\x.sys").to_lowercase());
    }

    #[test]
    fn enumerations_work() {
        assert!(services().iter().any(|s| s.name.eq_ignore_ascii_case("EventLog")));
        let _ = run_entries();
        let _ = tasks(); // may be empty on locked-down systems, must not panic
        assert!(delete_task(r"\Microsoft\Windows\Defrag\ScheduledDefrag").is_err());
        assert!(delete_service("EventLog", "whatever").is_err());
    }
}
