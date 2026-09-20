//! Launchers for built-in Windows tools. The set is a fixed whitelist: the UI
//! passes an id, never a command line. `ShellExecuteExW` is used so tools
//! that require elevation (regedit, services...) trigger UAC themselves.

use crate::util::wide;
use crate::{AppError, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub id: &'static str,
    pub group: &'static str,
}

struct Tool {
    id: &'static str,
    group: &'static str,
    /// Path relative to %SystemRoot% (or a URI when starting with "ms-").
    file: &'static str,
    params: &'static str,
}

const TOOLS: &[Tool] = &[
    Tool { id: "taskManager", group: "system", file: r"System32\Taskmgr.exe", params: "" },
    Tool { id: "resourceMonitor", group: "system", file: r"System32\resmon.exe", params: "" },
    Tool { id: "systemInformation", group: "system", file: r"System32\msinfo32.exe", params: "" },
    Tool { id: "eventViewer", group: "system", file: r"System32\eventvwr.msc", params: "" },
    Tool { id: "services", group: "management", file: r"System32\services.msc", params: "" },
    Tool { id: "deviceManager", group: "management", file: r"System32\devmgmt.msc", params: "" },
    Tool { id: "diskManagement", group: "management", file: r"System32\diskmgmt.msc", params: "" },
    Tool { id: "registryEditor", group: "management", file: r"regedit.exe", params: "" },
    Tool { id: "controlPanel", group: "settings", file: r"System32\control.exe", params: "" },
    Tool { id: "windowsSettings", group: "settings", file: "ms-settings:", params: "" },
    Tool { id: "systemProperties", group: "settings", file: r"System32\SystemPropertiesAdvanced.exe", params: "" },
    Tool { id: "environmentVariables", group: "settings", file: r"System32\rundll32.exe", params: "sysdm.cpl,EditEnvironmentVariables" },
    Tool { id: "powershell", group: "shell", file: r"System32\WindowsPowerShell\v1.0\powershell.exe", params: "" },
    Tool { id: "commandPrompt", group: "shell", file: r"System32\cmd.exe", params: "" },
];

pub fn list() -> Vec<ToolInfo> {
    TOOLS.iter().map(|t| ToolInfo { id: t.id, group: t.group }).collect()
}

pub fn launch(id: &str) -> Result<()> {
    let tool = TOOLS
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| AppError::InvalidInput(format!("unknown tool: {id}")))?;
    let file = if tool.file.starts_with("ms-") {
        tool.file.to_string()
    } else {
        format!("{}\\{}", crate::system::windows_dir(), tool.file)
    };
    shell_open(&file, tool.params, "open")
}

fn shell_open(file: &str, params: &str, verb: &str) -> Result<()> {
    use windows_sys::Win32::UI::Shell::*;
    let verb_w = wide(verb);
    let file_w = wide(file);
    let params_w = wide(params);
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOASYNC;
    sei.lpVerb = verb_w.as_ptr();
    sei.lpFile = file_w.as_ptr();
    sei.lpParameters = if params.is_empty() { std::ptr::null() } else { params_w.as_ptr() };
    sei.nShow = windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if unsafe { ShellExecuteExW(&mut sei) } == 0 {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if code == windows_sys::Win32::Foundation::ERROR_CANCELLED {
            return Err(AppError::Cancelled);
        }
        return Err(AppError::from_win32(code, "starting tool", Some(file)));
    }
    Ok(())
}

/// Restart the whole application elevated (user-initiated, e.g. from the
/// "Run as administrator" button). The caller exits on success.
pub fn restart_elevated() -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| AppError::io("locating executable", None, e))?;
    shell_open(&exe.to_string_lossy(), "", "runas").map_err(|e| match e {
        AppError::Cancelled => AppError::ElevationRequired("the administrator prompt was declined".into()),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn whitelist_is_closed() {
        assert!(super::launch("rm -rf").is_err());
        assert!(super::launch("").is_err());
        assert_eq!(super::list().len(), super::TOOLS.len());
        for t in super::TOOLS {
            assert!(!t.file.contains(".."));
        }
    }
}
