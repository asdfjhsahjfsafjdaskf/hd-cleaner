//! Startup manager: what starts with Windows or with the user session.
//!
//! Sources: Run / RunOnce (HKCU, HKLM 64 and 32-bit), the user and common
//! Startup folders, scheduled tasks with a logon/boot trigger (outside
//! `\Microsoft\`) and third-party Win32 services.
//!
//! Disabling uses the same mechanisms as Windows itself, so it is reversible
//! and Task Manager / Settings show the same state:
//! * Run and Startup folder items → `Explorer\StartupApproved` values;
//! * tasks → the task's Enabled flag;
//! * services → automatic ↔ manual start.
//!
//! Removal is only offered for Run values, Startup folder files and tasks,
//! always with a backup (.reg export, file copy, task XML) first. Items whose
//! program lives in the Windows directory can be disabled but not removed;
//! services are never removed here.

use crate::leftovers::{needs_elevation, PendingOp};
use crate::registry::{Hive, Key, View};
use crate::regops::RegTarget;
use crate::util::wide;
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ApprovedKey {
    Run,
    Run32,
    StartupFolder,
}

impl ApprovedKey {
    fn sub(self) -> &'static str {
        match self {
            ApprovedKey::Run => "Run",
            ApprovedKey::Run32 => "Run32",
            ApprovedKey::StartupFolder => "StartupFolder",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Source {
    RunKey { hive: Hive, view: View, once: bool },
    StartupFolder { common: bool },
    Task,
    Service,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupItem {
    /// Stable identifier (source + location + key).
    pub id: String,
    pub name: String,
    pub source: Source,
    /// Registry key, folder, task folder or "Services".
    pub location: String,
    /// Run value name, file path, task path or service name.
    pub key: String,
    pub command: String,
    pub exe: Option<String>,
    pub exe_exists: bool,
    pub enabled: bool,
    pub disabled_at_ms: Option<i64>,
    pub can_disable: bool,
    pub can_remove: bool,
    /// Changing it certainly needs administrator rights (HKLM, common folder, services).
    pub needs_admin: bool,
    /// The program lives in the Windows directory.
    pub is_windows: bool,
    pub company: Option<String>,
    pub description: Option<String>,
    /// Task trigger ("logon"/"boot") or service start mode ("auto", "delayed", "manual", "disabled").
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// StartupApproved values
// ---------------------------------------------------------------------------

/// Windows writes 12 bytes: a flag (even = enabled, odd = disabled) and,
/// when disabled, the FILETIME of the change.
pub fn parse_approved(data: &[u8]) -> Option<(bool, Option<i64>)> {
    let flag = *data.first()?;
    let enabled = flag & 1 == 0;
    let when = (!enabled && data.len() >= 12)
        .then(|| u64::from_le_bytes(data[4..12].try_into().unwrap()))
        .filter(|&t| t != 0)
        .map(crate::util::filetime_to_unix_ms);
    Some((enabled, when))
}

pub fn encode_approved(enabled: bool, now_filetime: u64) -> [u8; 12] {
    let mut v = [0u8; 12];
    if enabled {
        v[0] = 2;
    } else {
        v[0] = 3;
        v[4..12].copy_from_slice(&now_filetime.to_le_bytes());
    }
    v
}

fn read_approved(hive: Hive, key: ApprovedKey, name: &str) -> Option<(bool, Option<i64>)> {
    let k = Key::open(hive, &format!(r"{APPROVED}\{}", key.sub()), View::Default)?;
    let (_, data) = k.value_raw(name)?;
    parse_approved(&data)
}

fn now_filetime() -> u64 {
    crate::util::unix_ms_to_filetime(crate::util::now_unix_ms())
}

/// The Run value name this app uses for itself.
pub const SELF_RUN_VALUE: &str = "HD Cleaner";

/// Is this app set to start with Windows (and with which command)?
pub fn self_startup() -> Option<String> {
    crate::registry::read_string(
        Hive::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        SELF_RUN_VALUE,
        crate::registry::View::Default,
    )
}

/// Add or remove this app's own Run entry (current user only — no admin
/// rights and nothing machine-wide).
pub fn set_self_startup(enabled: bool) -> Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::*;
    let exe = std::env::current_exe().map_err(|e| AppError::io("finding this program", None, e))?;
    let command = format!("\"{}\"", exe.display());
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegCreateKeyExW(Hive::CurrentUser.raw(), path.as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE, std::ptr::null(), &mut h, std::ptr::null_mut())
    };
    if rc != ERROR_SUCCESS {
        return Err(AppError::from_win32(rc, "opening the Run key", Some(SELF_RUN_VALUE)));
    }
    let name = wide(SELF_RUN_VALUE);
    let rc = if enabled {
        let value = wide(&command);
        unsafe { RegSetValueExW(h, name.as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, (value.len() * 2) as u32) }
    } else {
        match unsafe { RegDeleteValueW(h, name.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
            other => other,
        }
    };
    unsafe { RegCloseKey(h) };
    if rc != ERROR_SUCCESS {
        return Err(AppError::from_win32(rc, "writing the Run entry", Some(SELF_RUN_VALUE)));
    }
    Ok(())
}

/// Write one StartupApproved value. Only the three fixed keys can be written.
pub fn set_approved(hive: Hive, key: ApprovedKey, name: &str, enabled: bool) -> Result<()> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::*;
    if !matches!(hive, Hive::CurrentUser | Hive::LocalMachine) || name.is_empty() || name.len() > 1024 || name.contains('\0') {
        return Err(AppError::InvalidInput("invalid startup entry".into()));
    }
    let path = format!(r"{APPROVED}\{}", key.sub());
    let target = format!(r"{}\{path} → {name}", hive.short());
    let p = wide(&path);
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegCreateKeyExW(hive.raw(), p.as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE | KEY_WOW64_64KEY, std::ptr::null(), &mut h, std::ptr::null_mut())
    };
    if rc != ERROR_SUCCESS {
        return Err(AppError::from_win32(rc, "opening StartupApproved", Some(&target)));
    }
    let data = encode_approved(enabled, now_filetime());
    let n = wide(name);
    let rc = unsafe { RegSetValueExW(h, n.as_ptr(), 0, REG_BINARY, data.as_ptr(), data.len() as u32) };
    unsafe { RegCloseKey(h) };
    if rc != ERROR_SUCCESS {
        return Err(AppError::from_win32(rc, "writing StartupApproved", Some(&target)));
    }
    Ok(())
}

/// Forget the approval state of a removed entry (best effort).
fn delete_approved(hive: Hive, key: ApprovedKey, name: &str) {
    use windows_sys::Win32::System::Registry::*;
    let p = wide(format!(r"{APPROVED}\{}", key.sub()));
    let n = wide(name);
    unsafe { RegDeleteKeyValueW(hive.raw(), p.as_ptr(), n.as_ptr()) };
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

pub fn startup_folders() -> Vec<(bool, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(a) = std::env::var("APPDATA") {
        out.push((false, Path::new(&a).join(r"Microsoft\Windows\Start Menu\Programs\Startup")));
    }
    if let Ok(p) = std::env::var("ProgramData") {
        out.push((true, Path::new(&p).join(r"Microsoft\Windows\Start Menu\Programs\StartUp")));
    }
    out
}

fn hive_name(h: Hive) -> &'static str {
    match h {
        Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
        Hive::CurrentUser => "HKEY_CURRENT_USER",
        Hive::ClassesRoot => "HKEY_CLASSES_ROOT",
        Hive::Users => "HKEY_USERS",
    }
}

fn run_approved_key(hive: Hive, view: View) -> ApprovedKey {
    if hive == Hive::LocalMachine && view == View::Reg32 {
        ApprovedKey::Run32
    } else {
        ApprovedKey::Run
    }
}

fn service_mode(start: Option<u32>, delayed: bool) -> &'static str {
    match start {
        Some(2) if delayed => "delayed",
        Some(2) => "auto",
        Some(3) => "manual",
        Some(4) => "disabled",
        _ => "other",
    }
}

/// False only when Windows says the file does not exist; "access denied"
/// (e.g. anti-cheat folders) is not a missing file.
fn surely_missing(path: &str) -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    let w = wide(crate::util::to_extended(path));
    if unsafe { GetFileAttributesW(w.as_ptr()) } != INVALID_FILE_ATTRIBUTES {
        return false;
    }
    matches!(unsafe { GetLastError() }, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
}

/// Everything that starts automatically. `nexus_disabled_services` are
/// services this app switched to manual start: they stay listed (as
/// disabled) so they can be turned back on. Other manual services are not
/// startup items and are not shown.
pub fn list(nexus_disabled_services: &[String]) -> Vec<StartupItem> {
    use rayon::prelude::*;
    let win = format!("{}\\", crate::system::windows_dir().to_lowercase());
    let in_windows = |exe: &Option<String>| exe.as_deref().is_some_and(|e| e.to_lowercase().starts_with(&win));
    let mut out = Vec::new();

    for e in crate::sysitems::run_entries() {
        let once = e.key.to_lowercase().ends_with("runonce");
        let approved = (!once).then(|| read_approved(e.hive, run_approved_key(e.hive, e.view), &e.name)).flatten();
        let is_windows = in_windows(&e.exe);
        let view = if e.hive == Hive::LocalMachine && e.view == View::Reg32 { r" [32-bit]" } else { "" };
        out.push(StartupItem {
            id: format!("run|{}|{:?}|{}|{}", e.hive.short(), e.view, e.key, e.name),
            name: e.name.clone(),
            source: Source::RunKey { hive: e.hive, view: e.view, once },
            location: format!(r"{}\{}{view}", hive_name(e.hive), e.key),
            key: e.name,
            command: e.command,
            exe_exists: false,
            exe: e.exe,
            enabled: approved.is_none_or(|a| a.0),
            disabled_at_ms: approved.and_then(|a| a.1),
            can_disable: !once,
            can_remove: !is_windows,
            needs_admin: e.hive == Hive::LocalMachine,
            is_windows,
            company: None,
            description: None,
            detail: once.then(|| "once".to_string()),
        });
    }

    for (common, dir) in startup_folders() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let files: Vec<PathBuf> = rd
            .flatten()
            .filter(|f| f.file_type().is_ok_and(|t| t.is_file()))
            .map(|f| f.path())
            .filter(|p| !p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("desktop.ini")))
            .collect();
        let links: Vec<PathBuf> = files.iter().filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"))).cloned().collect();
        let targets = crate::shortcuts::resolve_targets(&links);
        let hive = if common { Hive::LocalMachine } else { Hive::CurrentUser };
        for f in files {
            let file_name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let target = links.iter().position(|l| l == &f).and_then(|i| targets[i].clone());
            let exe = target.clone().or_else(|| Some(f.to_string_lossy().to_string()));
            let approved = read_approved(hive, ApprovedKey::StartupFolder, &file_name);
            out.push(StartupItem {
                id: format!("folder|{}|{}", if common { "common" } else { "user" }, file_name),
                name: f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| file_name.clone()),
                source: Source::StartupFolder { common },
                location: dir.to_string_lossy().to_string(),
                key: f.to_string_lossy().to_string(),
                command: target.unwrap_or_else(|| f.to_string_lossy().to_string()),
                exe_exists: false,
                is_windows: false,
                exe,
                enabled: approved.is_none_or(|a| a.0),
                disabled_at_ms: approved.and_then(|a| a.1),
                can_disable: true,
                can_remove: true,
                needs_admin: common,
                company: None,
                description: None,
                detail: None,
            });
        }
    }

    for t in crate::sysitems::tasks().into_iter().filter(|t| t.startup_trigger.is_some()) {
        let exe = t.exes.first().cloned();
        let is_windows = in_windows(&exe);
        let folder = t.path.rsplit_once('\\').map(|(f, _)| if f.is_empty() { "\\".to_string() } else { f.to_string() }).unwrap_or_default();
        out.push(StartupItem {
            id: format!("task|{}", t.path),
            name: t.name,
            source: Source::Task,
            location: folder,
            key: t.path,
            command: t.command,
            exe_exists: false,
            exe,
            enabled: t.enabled,
            disabled_at_ms: None,
            can_disable: true,
            can_remove: !is_windows,
            needs_admin: false,
            is_windows,
            company: None,
            description: None,
            detail: t.startup_trigger.map(str::to_string),
        });
    }

    for s in crate::sysitems::services() {
        // Third-party Win32 services only (no drivers, nothing in Windows).
        let ours = nexus_disabled_services.iter().any(|n| n.eq_ignore_ascii_case(&s.name));
        if s.service_type & 0x30 == 0 || in_windows(&s.exe) || s.exe.is_none() || !(s.start == Some(2) || (ours && s.start == Some(3))) {
            continue;
        }
        let display = s.display_name.clone().filter(|d| !d.starts_with('@')).unwrap_or_else(|| s.name.clone());
        out.push(StartupItem {
            id: format!("service|{}", s.name),
            name: display,
            source: Source::Service,
            location: r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services".into(),
            key: s.name,
            command: s.image_path.unwrap_or_default(),
            exe_exists: false,
            exe: s.exe,
            enabled: s.start == Some(2),
            disabled_at_ms: None,
            can_disable: true,
            can_remove: false,
            needs_admin: true,
            is_windows: false,
            company: None,
            description: s.description.filter(|d| !d.starts_with('@')),
            detail: Some(service_mode(s.start, s.delayed).to_string()),
        });
    }

    // File details (exists, publisher) in parallel; version resources are
    // read without executing anything.
    out.par_iter_mut().for_each(|i| {
        if let Some(exe) = &i.exe {
            i.exe_exists = !surely_missing(exe);
            if i.exe_exists && exe.to_lowercase().ends_with(".exe") {
                let v = crate::processes::version_info(exe);
                i.company = v.company;
                if i.description.is_none() {
                    i.description = v.description.or(v.product);
                }
            }
        }
    });
    out.sort_by_key(|i| i.name.to_lowercase());
    out
}

/// Internal setting: services this app switched from automatic to manual
/// (shared by the GUI and the CLI through the same database).
const DISABLED_SERVICES_KEY: &str = "internal.startupDisabledServices";

pub fn disabled_services(db: &crate::db::Database) -> Vec<String> {
    db.get_setting(DISABLED_SERVICES_KEY).ok().flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn remember_service(db: &crate::db::Database, name: &str, disabled: bool) {
    let mut list = disabled_services(db);
    list.retain(|n| !n.eq_ignore_ascii_case(name));
    if disabled {
        list.push(name.to_string());
    }
    let _ = db.set_setting(DISABLED_SERVICES_KEY, &serde_json::json!(list));
}

/// Re-read an item by id and check it is still what the user reviewed.
pub fn find(id: &str, expected_command: &str, nexus_disabled_services: &[String]) -> Result<StartupItem> {
    let item = list(nexus_disabled_services).into_iter().find(|i| i.id == id).ok_or_else(|| AppError::NotFound { path: id.to_string() })?;
    if item.command != expected_command {
        return Err(AppError::ChangedSinceReview { path: item.name });
    }
    Ok(item)
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// Typed operations that change the enabled state. Also executed by the
/// elevated helper, which only accepts HKLM approval values (the helper's
/// HKCU may belong to another account).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum StartupOp {
    SetApproved { hive: Hive, key: ApprovedKey, name: String, enabled: bool },
    SetTaskEnabled { path: String, enabled: bool },
    SetServiceStart { name: String, image: String, auto: bool },
    /// Drop the approval value of a removed entry (only in the three StartupApproved keys).
    ForgetApproved { hive: Hive, key: ApprovedKey, name: String },
}

pub fn apply(op: &StartupOp) -> Result<()> {
    match op {
        StartupOp::SetApproved { hive, key, name, enabled } => set_approved(*hive, *key, name, *enabled),
        StartupOp::SetTaskEnabled { path, enabled } => crate::sysitems::set_task_enabled(path, *enabled),
        StartupOp::SetServiceStart { name, image, auto } => crate::sysitems::set_service_start(name, image, *auto),
        StartupOp::ForgetApproved { hive, key, name } => {
            if !matches!(hive, Hive::CurrentUser | Hive::LocalMachine) || name.is_empty() || name.contains('\0') {
                return Err(AppError::InvalidInput("invalid startup entry".into()));
            }
            delete_approved(*hive, *key, name);
            Ok(())
        }
    }
}

/// Same as [`apply`], with the extra rule for the elevated helper.
pub fn apply_elevated(op: &StartupOp) -> Result<()> {
    if let StartupOp::SetApproved { hive, .. } | StartupOp::ForgetApproved { hive, .. } = op {
        if *hive != Hive::LocalMachine {
            return Err(AppError::InvalidInput("per-user startup entries are changed without elevation".into()));
        }
    }
    apply(op)
}

pub fn toggle_op(item: &StartupItem, enabled: bool) -> Result<StartupOp> {
    if !item.can_disable {
        return Err(AppError::NotSupported("this entry cannot be disabled (RunOnce entries run only once)".into()));
    }
    Ok(match &item.source {
        Source::RunKey { hive, view, .. } => StartupOp::SetApproved { hive: *hive, key: run_approved_key(*hive, *view), name: item.key.clone(), enabled },
        Source::StartupFolder { common } => StartupOp::SetApproved {
            hive: if *common { Hive::LocalMachine } else { Hive::CurrentUser },
            key: ApprovedKey::StartupFolder,
            name: Path::new(&item.key).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            enabled,
        },
        Source::Task => StartupOp::SetTaskEnabled { path: item.key.clone(), enabled },
        Source::Service => StartupOp::SetServiceStart { name: item.key.clone(), image: item.command.clone(), auto: enabled },
    })
}

/// Enable/disable now; `Ok(Some(op))` = needs the elevated helper.
pub fn set_enabled(item: &StartupItem, enabled: bool) -> Result<Option<StartupOp>> {
    let op = toggle_op(item, enabled)?;
    let certainly_admin = matches!(op, StartupOp::SetServiceStart { .. } | StartupOp::SetApproved { hive: Hive::LocalMachine, .. });
    if certainly_admin && !crate::system::is_elevated() {
        return Ok(Some(op));
    }
    match apply(&op) {
        Ok(()) => Ok(None),
        Err(e) if needs_elevation(&e) && !crate::system::is_elevated() => Ok(Some(op)),
        Err(e) => Err(e),
    }
}

fn safe_file_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).take(60).collect();
    n.trim_matches('_').to_string()
}

/// Remove an entry after backing it up into `backup_dir`. Returns the
/// operation still to run elevated, if any (the backup is already made).
pub fn remove(item: &StartupItem, backup_dir: &Path) -> Result<Option<PendingOp>> {
    if !item.can_remove {
        return Err(AppError::Protected { path: item.name.clone(), reason: if item.is_windows { "windowsComponent".into() } else { "notRemovable".into() } });
    }
    crate::util::ensure_dir(backup_dir)?;
    let mut manifest = crate::backups::Manifest::new("startup", &item.name);
    let op = match &item.source {
        Source::RunKey { hive, view, .. } => {
            let key_path = item.location.split_once('\\').map(|(_, p)| p.trim_end_matches(" [32-bit]").to_string()).unwrap_or_default();
            let target = RegTarget { hive: *hive, path: key_path, view: *view, value: Some(item.key.clone()) };
            crate::regops::export(std::slice::from_ref(&target), &backup_dir.join("startup.reg"))?;
            manifest.entries.push(crate::backups::Entry {
                kind: crate::backups::EntryKind::Registry,
                path: target.display(),
                stored: Some("startup.reg".into()),
                size: 0,
            });
            PendingOp::DeleteRegistry { target }
        }
        Source::StartupFolder { .. } => {
            let name = Path::new(&item.key).file_name().map(|n| n.to_owned()).unwrap_or_default();
            let size = std::fs::copy(&item.key, backup_dir.join(&name)).map_err(|e| AppError::io("backing up startup file", Some(Path::new(&item.key)), e))?;
            manifest.entries.push(crate::backups::Entry {
                kind: crate::backups::EntryKind::File,
                path: item.key.clone(),
                stored: Some(name.to_string_lossy().into_owned()),
                size,
            });
            PendingOp::DeletePath { path: item.key.clone(), recycle: true }
        }
        Source::Task => {
            let xml = crate::sysitems::task_xml(&item.key)?;
            let file = backup_dir.join(format!("{}.xml", safe_file_name(&item.name)));
            std::fs::write(&file, &xml).map_err(|e| AppError::io("backing up task", Some(&file), e))?;
            manifest.entries.push(crate::backups::Entry {
                kind: crate::backups::EntryKind::Task,
                path: item.key.clone(),
                stored: file.file_name().map(|n| n.to_string_lossy().into_owned()),
                size: xml.len() as u64,
            });
            PendingOp::DeleteTask { path: item.key.clone() }
        }
        Source::Service => return Err(AppError::NotSupported("services are only disabled, never removed".into())),
    };
    manifest.save(backup_dir)?;
    let r = match &op {
        PendingOp::DeleteRegistry { target } if target.hive == Hive::LocalMachine && !crate::system::is_elevated() => return Ok(Some(op)),
        PendingOp::DeletePath { .. } if item.needs_admin && !crate::system::is_elevated() => return Ok(Some(op)),
        _ => crate::leftovers::apply_pending(&op),
    };
    match r {
        Ok(()) => {
            forget_approval(item);
            Ok(None)
        }
        Err(e) if needs_elevation(&e) && !crate::system::is_elevated() => Ok(Some(op)),
        Err(e) => Err(e),
    }
}

/// Where the approval state of an entry lives (Run and Startup folder items).
fn approval_of(item: &StartupItem) -> Option<(Hive, ApprovedKey, String)> {
    match &item.source {
        Source::RunKey { hive, view, once: false } => Some((*hive, run_approved_key(*hive, *view), item.key.clone())),
        Source::StartupFolder { common } => Path::new(&item.key)
            .file_name()
            .map(|n| (if *common { Hive::LocalMachine } else { Hive::CurrentUser }, ApprovedKey::StartupFolder, n.to_string_lossy().to_string())),
        _ => None,
    }
}

/// Drop the StartupApproved value of a removed per-user entry.
pub fn forget_approval(item: &StartupItem) {
    if let Some((Hive::CurrentUser, key, name)) = approval_of(item) {
        delete_approved(Hive::CurrentUser, key, &name);
    }
}

/// For a machine-wide entry: the elevated op that drops its approval value
/// (batched with the removal, so still a single UAC prompt).
pub fn forget_approval_op(item: &StartupItem) -> Option<StartupOp> {
    approval_of(item).filter(|a| a.0 == Hive::LocalMachine).map(|(hive, key, name)| StartupOp::ForgetApproved { hive, key, name })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_bytes_roundtrip() {
        let ft = crate::util::unix_ms_to_filetime(1_700_000_000_000);
        let off = encode_approved(false, ft);
        assert_eq!(parse_approved(&off), Some((false, Some(1_700_000_000_000))));
        assert_eq!(parse_approved(&encode_approved(true, ft)), Some((true, None)));
        // Variants written by Windows: 0x06 enabled, 0x07 disabled.
        assert_eq!(parse_approved(&[6, 0, 0, 0]).map(|a| a.0), Some(true));
        assert_eq!(parse_approved(&[7]).map(|a| a.0), Some(false));
        assert_eq!(parse_approved(&[]), None);
    }

    #[test]
    fn enumerates_and_protects() {
        let items = list(&[]);
        let mut ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(n, ids.len(), "ids are unique");
        for i in &items {
            if i.is_windows {
                assert!(!i.can_remove, "{}", i.name);
            }
            if matches!(i.source, Source::Service) {
                assert!(!i.can_remove && i.needs_admin);
            }
        }
        assert!(set_approved(Hive::ClassesRoot, ApprovedKey::Run, "x", true).is_err());
        let elevated_hkcu = StartupOp::SetApproved { hive: Hive::CurrentUser, key: ApprovedKey::Run, name: "x".into(), enabled: true };
        assert!(apply_elevated(&elevated_hkcu).is_err());
    }
}
