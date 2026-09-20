//! Process listing with executable paths, and guarded termination.

use crate::util::{wide, OwnedHandle};
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::System::Threading::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcInfo {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    /// Full image path (None when the process cannot be queried, e.g. protected).
    pub path: Option<String>,
}

pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OwnedHandle::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid))?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(h.raw(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn list() -> Vec<ProcInfo> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = Vec::new();
    unsafe {
        let Some(h) = OwnedHandle::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)) else { return out };
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(h.raw(), &mut e) != 0 {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                let pid = e.th32ProcessID;
                out.push(ProcInfo {
                    pid,
                    parent: e.th32ParentProcessID,
                    name: String::from_utf16_lossy(&e.szExeFile[..n]),
                    path: if pid > 4 { image_path(pid) } else { None },
                });
                if Process32NextW(h.raw(), &mut e) == 0 {
                    break;
                }
            }
        }
    }
    out
}

fn within(path: &str, dir: &str) -> bool {
    let (p, d) = (path.to_lowercase(), dir.trim_end_matches('\\').to_lowercase());
    p.starts_with(&d) && p.as_bytes().get(d.len()) == Some(&b'\\')
}

/// Processes whose executable lives inside `dir`.
pub fn running_from(dir: &str) -> Vec<ProcInfo> {
    list().into_iter().filter(|p| p.path.as_deref().is_some_and(|x| within(x, dir))).collect()
}

/// Terminate a process after re-checking that it is still the same program
/// (PID reuse) and not part of Windows or this application.
pub fn terminate(pid: u32, expected_path: &str) -> Result<()> {
    if pid <= 4 || pid == std::process::id() {
        return Err(AppError::Protected { path: format!("pid {pid}"), reason: "systemProcess".into() });
    }
    let win = crate::system::windows_dir().to_lowercase();
    if expected_path.to_lowercase().starts_with(&win) {
        return Err(AppError::Protected { path: expected_path.into(), reason: "windowsDirectory".into() });
    }
    unsafe {
        let h = OwnedHandle::new(OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid))
            .ok_or_else(|| AppError::last_win32("opening process", Some(expected_path)))?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(h.raw(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) == 0 {
            return Err(AppError::last_win32("querying process", Some(expected_path)));
        }
        let now = String::from_utf16_lossy(&buf[..len as usize]);
        if !now.eq_ignore_ascii_case(expected_path) {
            return Err(AppError::ChangedSinceReview { path: format!("pid {pid}") });
        }
        if TerminateProcess(h.raw(), 1) == 0 {
            return Err(AppError::last_win32("terminating process", Some(expected_path)));
        }
        WaitForSingleObject(h.raw(), 5000);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Process manager: detailed rows, CPU usage between samples, process trees
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRow {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    pub path: Option<String>,
    pub threads: u32,
    /// Private bytes (commit charge), the "Memory" column of Task Manager.
    pub private_bytes: Option<u64>,
    pub working_set: Option<u64>,
    /// % of the whole machine since the previous sample (None on the first one).
    pub cpu: Option<f32>,
    pub cpu_time_ms: Option<u64>,
    pub started_ms: Option<i64>,
    pub user: Option<String>,
    pub company: Option<String>,
    pub description: Option<String>,
    /// Image inside the Windows directory (Windows component).
    pub is_windows: bool,
    pub is_self: bool,
}

struct Times {
    created: u64,
    cpu: u64,
}

fn ft(f: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn times(h: windows_sys::Win32::Foundation::HANDLE) -> Option<Times> {
    use windows_sys::Win32::Foundation::FILETIME;
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    (unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) } != 0).then(|| Times { created: ft(c), cpu: ft(k) + ft(u) })
}

/// Creation time (FILETIME ticks) of a live process.
pub fn creation_time(pid: u32) -> Option<u64> {
    let h = OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    times(h.raw()).map(|t| t.created)
}

fn memory(h: windows_sys::Win32::Foundation::HANDLE) -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    let mut m: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let ok = unsafe { K32GetProcessMemoryInfo(h, (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(), m.cb) } != 0;
    ok.then(|| (m.PrivateUsage as u64, m.WorkingSetSize as u64))
}

/// Owner account (`DOMAIN\user`) of a process, cached per SID.
fn owner(h: windows_sys::Win32::Foundation::HANDLE, cache: &mut std::collections::HashMap<Vec<u8>, Option<String>>) -> Option<String> {
    use windows_sys::Win32::Security::*;
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(h, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let token = OwnedHandle::new(token)?;
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        if GetTokenInformation(token.raw(), TokenUser, buf.as_mut_ptr().cast(), buf.len() as u32, &mut len) == 0 {
            return None;
        }
        let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
        let sid_len = GetLengthSid(sid) as usize;
        let key = std::slice::from_raw_parts(sid as *const u8, sid_len).to_vec();
        if let Some(v) = cache.get(&key) {
            return v.clone();
        }
        let (mut name, mut domain) = (vec![0u16; 256], vec![0u16; 256]);
        let (mut nl, mut dl, mut use_) = (name.len() as u32, domain.len() as u32, 0);
        let v = (LookupAccountSidW(std::ptr::null(), sid, name.as_mut_ptr(), &mut nl, domain.as_mut_ptr(), &mut dl, &mut use_) != 0).then(|| {
            let n = String::from_utf16_lossy(&name[..nl as usize]);
            let d = String::from_utf16_lossy(&domain[..dl as usize]);
            if d.is_empty() { n } else { format!(r"{d}\{n}") }
        });
        cache.insert(key, v.clone());
        v
    }
}

/// Keeps the previous sample so CPU usage can be computed between calls.
#[derive(Default)]
pub struct ProcessSampler {
    prev: std::collections::HashMap<(u32, u64), u64>,
    prev_at: Option<std::time::Instant>,
    versions: std::collections::HashMap<String, VersionInfo>,
    users: std::collections::HashMap<Vec<u8>, Option<String>>,
}

impl ProcessSampler {
    pub fn sample(&mut self) -> Vec<ProcessRow> {
        use rayon::prelude::*;
        let now = std::time::Instant::now();
        let elapsed = self.prev_at.map(|t| now.duration_since(t).as_secs_f64());
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let win = format!("{}\\", crate::system::windows_dir().to_lowercase());
        let me = std::process::id();
        let mut next = std::collections::HashMap::new();
        let mut rows = Vec::new();
        for e in snapshot() {
            let (mut row, t) = base_row(e, &win, me, &mut self.users);
            if let Some(t) = t {
                if let (Some(el), Some(prev)) = (elapsed, self.prev.get(&(row.pid, t.created))) {
                    let used = t.cpu.saturating_sub(*prev) as f64 / 10_000_000.0;
                    row.cpu = Some(((used / (el * cpus)) * 100.0).clamp(0.0, 100.0) as f32);
                } else if elapsed.is_some() {
                    row.cpu = Some(0.0);
                }
                next.insert((row.pid, t.created), t.cpu);
            }
            rows.push(row);
        }
        // Version resources of executables not seen yet (read once, in parallel).
        let missing: Vec<String> = {
            let mut v: Vec<String> = rows.iter().filter_map(|r| r.path.clone()).filter(|p| !self.versions.contains_key(p)).collect();
            v.sort();
            v.dedup();
            v
        };
        let found: Vec<(String, VersionInfo)> = missing.into_par_iter().map(|p| {
            let v = version_info(&p);
            (p, v)
        }).collect();
        self.versions.extend(found);
        for r in &mut rows {
            if let Some(v) = r.path.as_ref().and_then(|p| self.versions.get(p)) {
                r.company = v.company.clone();
                r.description = v.description.clone();
            }
        }
        self.prev = next;
        self.prev_at = Some(now);
        rows
    }
}

fn base_row(e: SnapEntry, win: &str, me: u32, users: &mut std::collections::HashMap<Vec<u8>, Option<String>>) -> (ProcessRow, Option<Times>) {
    let mut row = ProcessRow {
        pid: e.pid,
        parent: e.parent,
        name: e.name,
        path: None,
        threads: e.threads,
        private_bytes: None,
        working_set: None,
        cpu: None,
        cpu_time_ms: None,
        started_ms: None,
        user: None,
        company: None,
        description: None,
        is_windows: false,
        is_self: e.pid == me,
    };
    let mut t = None;
    if e.pid > 4 {
        if let Some(h) = OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.pid) }) {
            row.path = image_path_of(h.raw());
            if let Some((p, w)) = memory(h.raw()) {
                row.private_bytes = Some(p);
                row.working_set = Some(w);
            }
            t = times(h.raw());
            if let Some(t) = &t {
                row.cpu_time_ms = Some(t.cpu / 10_000);
                row.started_ms = Some(crate::util::filetime_to_unix_ms(t.created));
            }
            row.user = owner(h.raw(), users);
        }
    }
    row.is_windows = row.path.as_deref().is_some_and(|p| p.to_lowercase().starts_with(win)) || e.pid <= 4;
    (row, t)
}

/// One process with its details (no CPU %, which needs two samples).
pub fn process_row(pid: u32) -> Option<ProcessRow> {
    let e = snapshot().into_iter().find(|e| e.pid == pid)?;
    let win = format!("{}\\", crate::system::windows_dir().to_lowercase());
    let (mut row, _) = base_row(e, &win, std::process::id(), &mut Default::default());
    if let Some(p) = &row.path {
        let v = version_info(p);
        row.company = v.company;
        row.description = v.description;
    }
    Some(row)
}

struct SnapEntry {
    pid: u32,
    parent: u32,
    threads: u32,
    name: String,
}

fn snapshot() -> Vec<SnapEntry> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = Vec::new();
    unsafe {
        let Some(h) = OwnedHandle::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)) else { return out };
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(h.raw(), &mut e) != 0 {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                out.push(SnapEntry {
                    pid: e.th32ProcessID,
                    parent: e.th32ParentProcessID,
                    threads: e.cntThreads,
                    name: String::from_utf16_lossy(&e.szExeFile[..n]),
                });
                if Process32NextW(h.raw(), &mut e) == 0 {
                    break;
                }
            }
        }
    }
    out
}

fn image_path_of(h: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    (unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0)
        .then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// Descendants of `pid`, deepest first. A child only counts if it was created
/// after its parent (a dead parent's PID may have been reused).
pub fn descendants(pid: u32) -> Vec<ProcInfo> {
    let all = list();
    let created: std::collections::HashMap<u32, u64> = all.iter().filter_map(|p| creation_time(p.pid).map(|c| (p.pid, c))).collect();
    let mut out = Vec::new();
    let mut frontier = vec![pid];
    let mut seen = std::collections::HashSet::from([pid]);
    while let Some(parent) = frontier.pop() {
        let pc = created.get(&parent).copied();
        for c in all.iter().filter(|p| p.parent == parent && p.pid != parent) {
            let newer = match (pc, created.get(&c.pid)) {
                (Some(a), Some(b)) => *b >= a,
                _ => false,
            };
            if newer && seen.insert(c.pid) {
                out.push(c.clone());
                frontier.push(c.pid);
            }
        }
    }
    out.reverse();
    out
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeKill {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
    pub ok: bool,
    /// Windows component left alone (e.g. conhost.exe; it closes with its client).
    pub skipped: bool,
    pub error: Option<crate::ErrorPayload>,
}

/// End a process and all its descendants (children first). Each process is
/// re-verified and protected the same way as [`terminate`].
pub fn terminate_tree(pid: u32, expected_path: &str) -> Result<Vec<TreeKill>> {
    // Validate the root before touching any child.
    let root_now = image_path(pid).ok_or_else(|| AppError::NotFound { path: format!("pid {pid}") })?;
    if !root_now.eq_ignore_ascii_case(expected_path) {
        return Err(AppError::ChangedSinceReview { path: format!("pid {pid}") });
    }
    let mut out = Vec::new();
    let win = format!("{}\\", crate::system::windows_dir().to_lowercase());
    for c in descendants(pid) {
        if c.path.as_deref().is_some_and(|p| p.to_lowercase().starts_with(&win)) {
            out.push(TreeKill { pid: c.pid, name: c.name, path: c.path, ok: false, skipped: true, error: None });
            continue;
        }
        let r = match &c.path {
            Some(p) => terminate(c.pid, p),
            None => Err(AppError::Protected { path: format!("pid {}", c.pid), reason: "processNotAccessible".into() }),
        };
        out.push(TreeKill { pid: c.pid, name: c.name, path: c.path, ok: r.is_ok(), skipped: false, error: r.err().map(|e| e.to_payload()) });
    }
    let r = terminate(pid, expected_path);
    let name = expected_path.rsplit('\\').next().unwrap_or(expected_path).to_string();
    out.push(TreeKill { pid, name, path: Some(expected_path.into()), ok: r.is_ok(), skipped: false, error: r.err().map(|e| e.to_payload()) });
    Ok(out)
}

/// What the elevated helper sends back for each process it handled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KillRecord {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
    pub ok: bool,
    pub skipped: bool,
    pub error: Option<String>,
}

impl From<TreeKill> for KillRecord {
    fn from(k: TreeKill) -> Self {
        KillRecord { pid: k.pid, name: k.name, path: k.path, ok: k.ok, skipped: k.skipped, error: k.error.map(|e| e.message) }
    }
}

impl From<KillRecord> for TreeKill {
    fn from(k: KillRecord) -> Self {
        TreeKill { pid: k.pid, name: k.name, path: k.path, ok: k.ok, skipped: k.skipped, error: k.error.map(|m| AppError::Helper(m).to_payload()) }
    }
}

/// Runs inside the elevated helper: re-identify the process from the live
/// snapshot (PID + image name + parent PID, plus the path when the caller
/// could read it), then end it — or its tree — with the usual protections
/// (Windows components are never ended).
pub fn end_identified(pid: u32, name: &str, parent: u32, expected_path: Option<&str>, tree: bool) -> Result<Vec<KillRecord>> {
    let e = snapshot().into_iter().find(|e| e.pid == pid).ok_or_else(|| AppError::NotFound { path: format!("pid {pid}") })?;
    if !e.name.eq_ignore_ascii_case(name) || e.parent != parent {
        return Err(AppError::ChangedSinceReview { path: format!("pid {pid}") });
    }
    let actual = image_path(pid).ok_or_else(|| AppError::AccessDenied { path: format!("pid {pid}") })?;
    if expected_path.is_some_and(|p| !p.eq_ignore_ascii_case(&actual)) {
        return Err(AppError::ChangedSinceReview { path: format!("pid {pid}") });
    }
    let list = if tree {
        terminate_tree(pid, &actual)?
    } else {
        terminate(pid, &actual)?;
        vec![TreeKill { pid, name: e.name, path: Some(actual), ok: true, skipped: false, error: None }]
    };
    Ok(list.into_iter().map(KillRecord::from).collect())
}

/// End a process (and optionally its tree) through the elevated helper (one
/// UAC prompt). Works even when this process cannot read the target's path
/// (SYSTEM or another account): the helper identifies it itself.
pub fn terminate_elevated(pid: u32, expected_path: Option<&str>, tree: bool) -> Result<Vec<TreeKill>> {
    use crate::elevation::{run_elevated_ops, ElevatedOp};
    let e = snapshot().into_iter().find(|e| e.pid == pid).ok_or_else(|| AppError::NotFound { path: format!("pid {pid}") })?;
    if pid <= 4 || pid == std::process::id() {
        return Err(AppError::Protected { path: format!("pid {pid}"), reason: "systemProcess".into() });
    }
    let op = ElevatedOp::EndProcess { pid, name: e.name.clone(), parent: e.parent, path: expected_path.map(str::to_string), tree };
    let r = run_elevated_ops(&[op])?.into_iter().next().ok_or_else(|| AppError::Helper("no result from helper".into()))?;
    if !r.ok {
        let msg = r.message.unwrap_or_default();
        return Err(match r.error_kind.as_deref() {
            Some("protected") => AppError::Protected { path: e.name, reason: msg },
            Some("changedSinceReview") => AppError::ChangedSinceReview { path: format!("pid {pid}") },
            Some("notFound") => AppError::NotFound { path: format!("pid {pid}") },
            _ => AppError::Helper(msg),
        });
    }
    let list: Vec<KillRecord> = r.data.and_then(|d| serde_json::from_value(d).ok()).unwrap_or_default();
    Ok(list.into_iter().map(TreeKill::from).collect())
}

/// File version resource strings (CompanyName, ProductName, FileDescription).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub company: Option<String>,
    pub product: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
}

pub fn version_info(path: &str) -> VersionInfo {
    use windows_sys::Win32::Storage::FileSystem::*;
    let w = wide(path);
    let mut out = VersionInfo::default();
    unsafe {
        let size = GetFileVersionInfoSizeW(w.as_ptr(), std::ptr::null_mut());
        if size == 0 || size > 16 * 1024 * 1024 {
            return out;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return out;
        }
        // First translation (language + code page).
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let tr = wide(r"\VarFileInfo\Translation");
        let (lang, cp) = if VerQueryValueW(data.as_ptr().cast(), tr.as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let p = ptr as *const u16;
            (*p, *p.add(1))
        } else {
            (0x0409, 0x04b0)
        };
        let get = |key: &str| -> Option<String> {
            let q = wide(&format!(r"\StringFileInfo\{lang:04x}{cp:04x}\{key}"));
            let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut l = 0u32;
            if VerQueryValueW(data.as_ptr().cast(), q.as_ptr(), &mut p, &mut l) == 0 || l == 0 {
                return None;
            }
            let s = std::slice::from_raw_parts(p as *const u16, l as usize);
            let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
            Some(String::from_utf16_lossy(&s[..end]).trim().to_string()).filter(|x| !x.is_empty())
        };
        out.company = get("CompanyName");
        out.product = get("ProductName");
        out.description = get("FileDescription");
        out.version = get("ProductVersion").or_else(|| get("FileVersion"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_self_and_reads_version() {
        let me = list().into_iter().find(|p| p.pid == std::process::id()).expect("own process listed");
        assert!(me.path.is_some());
        let win = crate::system::windows_dir();
        let vi = version_info(&format!(r"{win}\System32\notepad.exe"));
        assert!(vi.company.as_deref().unwrap_or("").contains("Microsoft"));
        assert!(terminate(4, r"C:\x.exe").is_err());
        assert!(terminate(std::process::id(), r"C:\x.exe").is_err());
    }

    #[test]
    fn terminates_only_the_expected_process() {
        let mut child = std::process::Command::new(format!(r"{}\System32\ping.exe", crate::system::windows_dir()))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Windows dir binaries are refused.
        let path = format!(r"{}\System32\PING.EXE", crate::system::windows_dir());
        assert!(terminate(child.id(), &path).is_err());
        // A wrong expected path is refused (PID reuse protection).
        assert!(matches!(terminate(child.id(), r"C:\Other\app.exe"), Err(AppError::ChangedSinceReview { .. })));
        let _ = child.kill();
    }
}
