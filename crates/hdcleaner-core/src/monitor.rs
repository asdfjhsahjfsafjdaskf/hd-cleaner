//! Installation monitor: what an installer actually put on the machine.
//!
//! Three steps. `snapshot` records the state of the folders installers use
//! plus the relevant registry areas, services, scheduled tasks, startup
//! entries and installed programs. While the installer runs, `Watcher`
//! listens to the same folders with `ReadDirectoryChangesW`, so files that
//! are created and deleted in between are still seen. `diff` then compares
//! the two snapshots (plus what the watcher saw) and produces an
//! **installation trace**: the list of what appeared.
//!
//! Nothing here changes the system; a trace is only a record. The uninstall
//! wizard can use it later to find leftovers the generic rules would miss.

use crate::registry::{Hive, Key, View};
use crate::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Folders installers write to. Only these are snapshotted and watched.
pub fn default_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: Option<PathBuf>| {
        if let Some(p) = p.filter(|p| p.is_dir()) {
            out.push(p);
        }
    };
    let env = |v: &str| std::env::var_os(v).map(PathBuf::from);
    push(env("ProgramFiles"));
    push(env("ProgramFiles(x86)"));
    push(env("ProgramData"));
    push(env("LOCALAPPDATA"));
    push(env("APPDATA"));
    push(env("USERPROFILE").map(|p| p.join("Desktop")));
    push(env("PUBLIC").map(|p| p.join("Desktop")));
    out.sort();
    out.dedup();
    out
}

/// Registry areas an installer normally touches (keys only, limited depth).
fn registry_roots() -> Vec<(Hive, &'static str, View, u32)> {
    use Hive::*;
    vec![
        (LocalMachine, r"SOFTWARE", View::Reg64, 3),
        (LocalMachine, r"SOFTWARE", View::Reg32, 3),
        (CurrentUser, r"SOFTWARE", View::Default, 3),
        (LocalMachine, r"SYSTEM\CurrentControlSet\Services", View::Default, 1),
        (LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths", View::Reg64, 1),
        (LocalMachine, r"SOFTWARE\Classes", View::Reg64, 1),
        // Deeper than the generic depth above, but exactly where installers
        // register themselves.
        (LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", View::Reg64, 1),
        (LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", View::Reg32, 1),
        (CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Uninstall", View::Default, 1),
        (LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData", View::Reg64, 2),
    ]
}

/// Registry places whose *values* matter (startup entries, uninstall entries).
fn value_roots() -> Vec<(Hive, &'static str, View)> {
    use Hive::*;
    vec![
        (CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Run", View::Default),
        (CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\RunOnce", View::Default),
        (LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", View::Reg64),
        (LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", View::Reg32),
        (LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce", View::Reg64),
    ]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub taken_ms: i64,
    pub roots: Vec<String>,
    /// Lowercased file paths.
    pub files: HashSet<String>,
    /// Lowercased registry keys, as `HKLM\Software\...`.
    pub keys: HashSet<String>,
    /// Lowercased `HKCU\...\Run → name` value identifiers.
    pub values: HashSet<String>,
    pub services: HashSet<String>,
    pub tasks: HashSet<String>,
    pub programs: HashSet<String>,
}

fn walk_files(dir: &Path, depth: u32, out: &mut HashSet<String>, limit: usize) {
    if out.len() >= limit || depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        // Links are recorded but never followed.
        let is_link = {
            use std::os::windows::fs::MetadataExt;
            md.file_attributes() & 0x400 != 0
        };
        out.insert(e.path().to_string_lossy().to_lowercase());
        if out.len() >= limit {
            return;
        }
        if md.is_dir() && !is_link {
            walk_files(&e.path(), depth - 1, out, limit);
        }
    }
}

fn walk_keys(hive: Hive, path: &str, view: View, depth: u32, out: &mut HashSet<String>) {
    let Some(k) = Key::open(hive, path, view) else { return };
    for sub in k.subkeys() {
        let full = format!("{path}\\{sub}");
        out.insert(format!("{}\\{}", hive.short(), full).to_lowercase());
        if depth > 1 {
            walk_keys(hive, &full, view, depth - 1, out);
        }
    }
}

/// Snapshot the machine state that an installation would change.
pub fn snapshot(roots: &[PathBuf]) -> Snapshot {
    use rayon::prelude::*;
    const FILE_LIMIT: usize = 1_500_000;
    let files: HashSet<String> = roots
        .par_iter()
        .map(|r| {
            let mut set = HashSet::new();
            walk_files(r, 12, &mut set, FILE_LIMIT / roots.len().max(1));
            set
        })
        .reduce(HashSet::new, |mut a, b| {
            a.extend(b);
            a
        });
    let keys: HashSet<String> = registry_roots()
        .par_iter()
        .map(|(hive, path, view, depth)| {
            let mut set = HashSet::new();
            walk_keys(*hive, path, *view, *depth, &mut set);
            set
        })
        .reduce(HashSet::new, |mut a, b| {
            a.extend(b);
            a
        });
    let mut values = HashSet::new();
    for (hive, path, view) in value_roots() {
        if let Some(k) = Key::open(hive, path, view) {
            for name in k.value_names() {
                values.insert(format!("{}\\{} → {}", hive.short(), path, name).to_lowercase());
            }
        }
    }
    Snapshot {
        taken_ms: crate::util::now_unix_ms(),
        roots: roots.iter().map(|r| r.to_string_lossy().to_string()).collect(),
        files,
        keys,
        values,
        services: crate::sysitems::services().into_iter().map(|s| s.name.to_lowercase()).collect(),
        tasks: crate::sysitems::tasks().into_iter().map(|t| t.path.to_lowercase()).collect(),
        programs: crate::programs::registry_programs().into_iter().map(|p| p.id).collect(),
    }
}

// ---------------------------------------------------------------------------
// Live watch (ReadDirectoryChangesW)
// ---------------------------------------------------------------------------

/// Watches folders while the installer runs, so files that only exist for a
/// moment are still recorded.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<HashSet<String>>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    /// Handles kept so the blocking read can be cancelled on stop.
    dirs: Vec<crate::util::OwnedHandle>,
}

impl Watcher {
    /// Paths seen so far (lowercased).
    pub fn seen(&self) -> HashSet<String> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn count(&self) -> usize {
        self.seen.lock().map(|s| s.len()).unwrap_or(0)
    }

    /// Stop watching and return everything that was seen.
    pub fn stop(mut self) -> HashSet<String> {
        use windows_sys::Win32::System::IO::CancelIoEx;
        self.stop.store(true, Ordering::SeqCst);
        for d in &self.dirs {
            unsafe { CancelIoEx(d.raw(), std::ptr::null()) };
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        self.seen()
    }
}

/// Start watching the folders. Read-only: it never changes anything.
pub fn watch(roots: &[PathBuf]) -> Result<Watcher> {
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::*;
    let stop = Arc::new(AtomicBool::new(false));
    let seen: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut threads = Vec::new();
    let mut dirs = Vec::new();
    for root in roots {
        let w = crate::util::wide(root);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        let Some(handle) = crate::util::OwnedHandle::new(h) else {
            continue; // folder not readable: the before/after diff still covers it
        };
        let raw = handle.raw() as isize;
        dirs.push(handle);
        let (stop_c, seen_c, base) = (stop.clone(), seen.clone(), root.clone());
        threads.push(std::thread::spawn(move || watch_loop(raw, base, stop_c, seen_c)));
    }
    if dirs.is_empty() {
        return Err(AppError::AccessDenied { path: "installation monitor folders".into() });
    }
    Ok(Watcher { stop, seen, threads, dirs })
}

fn watch_loop(raw: isize, base: PathBuf, stop: Arc<AtomicBool>, seen: Arc<Mutex<HashSet<String>>>) {
    use windows_sys::Win32::Storage::FileSystem::*;
    const BUF: usize = 64 * 1024;
    const MAX_SEEN: usize = 200_000;
    let handle = raw as windows_sys::Win32::Foundation::HANDLE;
    let mut buf = vec![0u8; BUF];
    while !stop.load(Ordering::SeqCst) {
        let mut returned = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr().cast(),
                BUF as u32,
                1, // watch subtree
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE,
                &mut returned,
                std::ptr::null_mut(),
                None,
            )
        };
        if ok == 0 || stop.load(Ordering::SeqCst) {
            return;
        }
        let mut offset = 0usize;
        while offset + 12 <= returned as usize {
            let next = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as usize;
            let action = u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap());
            let len = u32::from_le_bytes(buf[offset + 8..offset + 12].try_into().unwrap()) as usize;
            let start = offset + 12;
            if start + len > returned as usize {
                break;
            }
            let units: Vec<u16> = buf[start..start + len].as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
            let name = String::from_utf16_lossy(&units);
            // 1 added, 3 modified, 5 renamed-to.
            if matches!(action, 1 | 3 | 5) && !name.is_empty() {
                if let Ok(mut s) = seen.lock() {
                    if s.len() < MAX_SEEN {
                        s.insert(base.join(&name).to_string_lossy().to_lowercase());
                    }
                }
            }
            if next == 0 {
                break;
            }
            offset += next;
        }
    }
}

// ---------------------------------------------------------------------------
// Trace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraceFile {
    pub path: String,
    pub size: u64,
    /// The file was seen only while the installer ran (temporary).
    pub transient: bool,
    /// The path looks like it belongs to the installed program (its name
    /// appears in it). Everything else changed in the same window but was
    /// probably another program doing its own thing.
    pub related: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraceRegistry {
    /// `HKLM\Software\Vendor\Product` or `HKCU\...\Run → Name`.
    pub path: String,
    /// "key" or "value"
    pub kind: String,
    /// Mentions the installed program (see [`TraceFile::related`]).
    pub related: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstallTrace {
    pub id: i64,
    /// What the user called this installation.
    pub name: String,
    /// Program that appeared in the installed-programs list, if any.
    pub program_id: Option<String>,
    pub program_name: Option<String>,
    pub started_ms: i64,
    pub finished_ms: i64,
    pub files: Vec<TraceFile>,
    pub registry: Vec<TraceRegistry>,
    pub services: Vec<String>,
    pub tasks: Vec<String>,
    pub bytes: u64,
}

impl InstallTrace {
    /// Folders that hold the new files (deepest common places first).
    pub fn folders(&self) -> Vec<String> {
        let mut dirs: HashMap<String, u64> = HashMap::new();
        for f in self.files.iter().filter(|f| !f.transient) {
            if let Some((d, _)) = f.path.rsplit_once('\\') {
                *dirs.entry(d.to_string()).or_default() += f.size;
            }
        }
        let mut v: Vec<(String, u64)> = dirs.into_iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        v.into_iter().map(|(d, _)| d).collect()
    }
}

fn file_size(path: &str) -> Option<u64> {
    std::fs::symlink_metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// Compare two snapshots (and what the watcher saw) into a trace.
pub fn diff(name: &str, before: &Snapshot, after: &Snapshot, watched: &HashSet<String>) -> InstallTrace {
    let mut files: Vec<TraceFile> = Vec::new();
    let mut bytes = 0;
    // Still present now.
    for p in after.files.difference(&before.files) {
        if let Some(size) = file_size(p) {
            bytes += size;
            files.push(TraceFile { path: p.clone(), size, transient: false, related: false });
        } else if Path::new(p).is_dir() {
            files.push(TraceFile { path: p.clone(), size: 0, transient: false, related: false });
        }
    }
    // Seen while installing but gone now (installer temporaries).
    for p in watched {
        if before.files.contains(p) || after.files.contains(p) {
            continue;
        }
        files.push(TraceFile { path: p.clone(), size: 0, transient: !Path::new(p).exists(), related: false });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);

    let mut registry: Vec<TraceRegistry> = after.keys.difference(&before.keys).map(|k| TraceRegistry { path: k.clone(), kind: "key".into(), related: false }).collect();
    registry.extend(after.values.difference(&before.values).map(|v| TraceRegistry { path: v.clone(), kind: "value".into(), related: false }));
    registry.sort_by(|a, b| a.path.cmp(&b.path));
    // A key whose parent is also new adds nothing to the list.
    let keys: HashSet<String> = registry.iter().filter(|r| r.kind == "key").map(|r| r.path.clone()).collect();
    registry.retain(|r| r.kind != "key" || !r.path.rsplit_once('\\').is_some_and(|(p, _)| keys.contains(p)));

    let program = after.programs.difference(&before.programs).next().cloned();
    let program_name = program.as_ref().and_then(|id| crate::programs::registry_programs().into_iter().find(|p| &p.id == id).map(|p| p.name));

    // Other programs write to the same folders while an installer runs. A
    // path counts as "this program's" when its own name (or the name the
    // user gave) shows up in it; the rest is only reported as noise.
    let mut words: Vec<String> = [program_name.as_deref(), Some(name)]
        .into_iter()
        .flatten()
        .map(crate::appsize::norm)
        .filter(|w| w.len() >= 3)
        .collect();
    words.sort();
    words.dedup();
    let relates = |path: &str| {
        let p = crate::appsize::norm(path);
        words.iter().any(|w| p.contains(w.as_str()))
    };
    for f in &mut files {
        f.related = relates(&f.path);
    }
    for r in &mut registry {
        r.related = relates(&r.path);
    }

    InstallTrace {
        id: 0,
        name: name.to_string(),
        program_id: program,
        program_name,
        started_ms: before.taken_ms,
        finished_ms: after.taken_ms,
        files,
        registry,
        services: after.services.difference(&before.services).cloned().collect(),
        tasks: after.tasks.difference(&before.tasks).cloned().collect(),
        bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_new_files_keys_and_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("before.txt"), b"x").unwrap();
        let before = Snapshot {
            files: {
                let mut s = HashSet::new();
                walk_files(&root, 8, &mut s, 1000);
                s
            },
            keys: HashSet::from([r"hklm\software\old".to_string()]),
            programs: HashSet::from(["reg:HKLM:Old".to_string()]),
            ..Default::default()
        };
        std::fs::create_dir(root.join("app")).unwrap();
        std::fs::write(root.join(r"app\program.exe"), vec![0u8; 2048]).unwrap();
        let after = Snapshot {
            files: {
                let mut s = HashSet::new();
                walk_files(&root, 8, &mut s, 1000);
                s
            },
            keys: HashSet::from([r"hklm\software\old".to_string(), r"hklm\software\vendor".to_string(), r"hklm\software\vendor\app".to_string()]),
            values: HashSet::from([r"hkcu\software\microsoft\windows\currentversion\run → app".to_string()]),
            services: HashSet::from(["appsvc".to_string()]),
            ..Default::default()
        };
        let watched = HashSet::from([root.join("setup-temp.tmp").to_string_lossy().to_lowercase()]);
        let t = diff("App 1.0", &before, &after, &watched);

        let paths: Vec<&str> = t.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.iter().any(|p| p.ends_with(r"app\program.exe")));
        assert!(paths.iter().any(|p| p.ends_with("setup-temp.tmp")));
        assert!(!paths.iter().any(|p| p.ends_with("before.txt")), "files that already existed are not in the trace");
        assert_eq!(t.bytes, 2048);
        assert!(t.files.iter().find(|f| f.path.ends_with("setup-temp.tmp")).unwrap().transient);
        // The parent key is enough; the child adds nothing.
        let regs: Vec<&str> = t.registry.iter().map(|r| r.path.as_str()).collect();
        assert!(regs.contains(&r"hklm\software\vendor"));
        assert!(!regs.contains(&r"hklm\software\vendor\app"));
        assert!(regs.iter().any(|r| r.ends_with("→ app")));
        assert_eq!(t.services, vec!["appsvc".to_string()]);
        assert!(t.folders().iter().any(|d| d.ends_with(r"\app")));
    }

    #[test]
    fn watches_a_folder_while_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let w = watch(std::slice::from_ref(&root)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join(r"sub\new.dat"), b"data").unwrap();
        std::fs::write(root.join("gone.tmp"), b"tmp").unwrap();
        std::fs::remove_file(root.join("gone.tmp")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let seen = w.stop();
        assert!(seen.iter().any(|p| p.ends_with(r"sub\new.dat")), "{seen:?}");
        assert!(seen.iter().any(|p| p.ends_with("gone.tmp")), "a file that no longer exists was still seen");
    }

    #[test]
    fn snapshot_sees_the_machine() {
        let roots = default_roots();
        assert!(!roots.is_empty());
        let s = snapshot(&roots[..1]);
        assert!(!s.files.is_empty() && !s.keys.is_empty());
        assert!(s.keys.iter().any(|k| k.starts_with(r"hklm\software")));
        assert!(!s.programs.is_empty());
    }
}
