//! Least-privilege elevation.
//!
//! The application runs unelevated. When an operation genuinely needs
//! administrator rights, the *same executable* is started through UAC
//! (`ShellExecuteExW` verb `runas`) in "elevated helper" mode. The helper:
//!
//! * accepts only a fixed list of operations ([`HelperCommand`]) with
//!   strictly validated arguments — there is no generic "run as admin" API;
//! * receives no paths from the caller: the only arguments are a drive letter
//!   and a random 128-bit channel id;
//! * streams results back through a named pipe created by the unelevated
//!   process with `FILE_FLAG_FIRST_PIPE_INSTANCE` (so it cannot be squatted)
//!   and `PIPE_REJECT_REMOTE_CLIENTS`.
//!
//! Wire protocol (helper → app): frames `[u8 type][u32 len][payload]`:
//! 1 = progress JSON, 3 = error JSON, 2 = "snapshot follows" after which the
//! rest of the stream is a `.hdcs` snapshot.

use crate::branding::{DATA_DIR_NAME, ELEVATED_HELPER_SWITCH};
use crate::scan::{ScanControl, ScanOptions, ScanTree};
use crate::util::{wide, OwnedHandle};
use crate::{AppError, Result};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Pipes::*;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};

const FRAME_PROGRESS: u8 = 1;
const FRAME_SNAPSHOT: u8 = 2;
const FRAME_ERROR: u8 = 3;

/// Operations the elevated helper is allowed to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperCommand {
    ScanNtfs { letter: char, channel: String },
    /// Typed, individually validated operations sent over the pipe.
    ApplyOps { channel: String },
}

impl HelperCommand {
    fn to_args(&self) -> String {
        match self {
            // Only a validated letter and hex id: nothing needs quoting.
            HelperCommand::ScanNtfs { letter, channel } => format!("{ELEVATED_HELPER_SWITCH} scan-ntfs {letter} {channel}"),
            HelperCommand::ApplyOps { channel } => format!("{ELEVATED_HELPER_SWITCH} apply-ops {channel}"),
        }
    }

    /// Strict parser. Anything unexpected is rejected.
    pub fn parse(args: &[String]) -> Result<HelperCommand> {
        let bad = || AppError::InvalidInput("invalid elevated helper arguments".into());
        match args {
            [switch, op, letter, channel] if switch == ELEVATED_HELPER_SWITCH && op == "scan-ntfs" => {
                let mut chars = letter.chars();
                let l = chars.next().ok_or_else(bad)?;
                if chars.next().is_some() || !l.is_ascii_alphabetic() {
                    return Err(bad());
                }
                if !valid_channel(channel) {
                    return Err(bad());
                }
                Ok(HelperCommand::ScanNtfs { letter: l.to_ascii_uppercase(), channel: channel.clone() })
            }
            [switch, op, channel] if switch == ELEVATED_HELPER_SWITCH && op == "apply-ops" && valid_channel(channel) => {
                Ok(HelperCommand::ApplyOps { channel: channel.clone() })
            }
            _ => Err(bad()),
        }
    }
}

fn valid_channel(c: &str) -> bool {
    c.len() == 32 && c.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn pipe_name(channel: &str) -> String {
    format!(r"\\.\pipe\{DATA_DIR_NAME}-{channel}")
}

fn random_channel() -> String {
    // 128 bits from the OS CSPRNG-backed hasher seeds + time + pid; the
    // id only needs to be unguessable before the pipe exists (we create it
    // first with FIRST_PIPE_INSTANCE, so squatting fails loudly anyway).
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(32);
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_i64(crate::util::now_unix_ms());
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

struct Pipe(OwnedHandle);

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        let want = buf.len().min(u32::MAX as usize) as u32;
        let ok = unsafe { ReadFile(self.0.raw(), buf.as_mut_ptr(), want, &mut n, std::ptr::null_mut()) } != 0;
        if !ok {
            let err = unsafe { GetLastError() };
            if err == ERROR_BROKEN_PIPE {
                return Ok(0);
            }
            return Err(std::io::Error::from_raw_os_error(err as i32));
        }
        Ok(n as usize)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        let want = buf.len().min(1 << 20) as u32;
        let ok = unsafe { WriteFile(self.0.raw(), buf.as_ptr(), want, &mut n, std::ptr::null_mut()) } != 0;
        if !ok {
            return Err(std::io::Error::last_os_error());
        }
        Ok(n as usize)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn write_frame(w: &mut impl Write, ty: u8, payload: &[u8]) -> std::io::Result<()> {
    w.write_all(&[ty])?;
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)
}

// ---------------------------------------------------------------------------
// Caller side (unelevated)
// ---------------------------------------------------------------------------

/// Run an NTFS MFT scan of `letter:` in an elevated helper and return the tree.
/// Progress is mirrored into `ctl`; cancelling `ctl` stops the helper.
pub fn scan_ntfs_elevated(letter: char, ctl: &ScanControl) -> Result<ScanTree> {
    if !letter.is_ascii_alphabetic() {
        return Err(AppError::InvalidInput("invalid drive letter".into()));
    }
    let channel = random_channel();
    let name = wide(pipe_name(&channel));
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            1 << 20,
            0,
            std::ptr::null(),
        )
    };
    let pipe = OwnedHandle::new(pipe).ok_or_else(|| AppError::last_win32("creating helper channel", None))?;

    let exe = std::env::current_exe().map_err(|e| AppError::io("locating executable", None, e))?;
    let cmd = HelperCommand::ScanNtfs { letter: letter.to_ascii_uppercase(), channel: channel.clone() };
    let process = launch_elevated(&exe.to_string_lossy(), &cmd.to_args())?;

    // If the helper dies before connecting, unblock ConnectNamedPipe by
    // connecting a dummy client ourselves.
    let connected = Arc::new(AtomicBool::new(false));
    let watcher = {
        let connected = connected.clone();
        let name = name.clone();
        // The process handle is moved into (and closed by) the watcher.
        std::thread::spawn(move || {
            unsafe { WaitForSingleObject(process.raw(), INFINITE) };
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(process.raw(), &mut code) };
            if !connected.load(Ordering::SeqCst) {
                let h = unsafe {
                    CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
                };
                drop(OwnedHandle::new(h));
            }
            drop(process);
            code
        })
    };

    let ok = unsafe { ConnectNamedPipe(pipe.raw(), std::ptr::null_mut()) } != 0;
    if !ok {
        let err = unsafe { GetLastError() };
        if err != ERROR_PIPE_CONNECTED {
            return Err(AppError::from_win32(err, "waiting for elevated helper", None));
        }
    }
    connected.store(true, Ordering::SeqCst);
    let mut reader = std::io::BufReader::with_capacity(1 << 20, Pipe(pipe));

    let result = (|| -> Result<ScanTree> {
        loop {
            ctl.check()?;
            let mut head = [0u8; 5];
            if reader.read_exact(&mut head).is_err() {
                return Err(AppError::Helper("the elevated helper exited without a result".into()));
            }
            let len = u32::from_le_bytes(head[1..5].try_into().unwrap()) as usize;
            if len > 16 * 1024 * 1024 {
                return Err(AppError::Helper("invalid frame from helper".into()));
            }
            let mut payload = vec![0u8; len];
            reader
                .read_exact(&mut payload)
                .map_err(|_| AppError::Helper("helper channel closed unexpectedly".into()))?;
            match head[0] {
                FRAME_PROGRESS => {
                    if let Ok(p) = serde_json::from_slice::<serde_json::Value>(&payload) {
                        let g = |k: &str| p.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                        ctl.files.store(g("files"), Ordering::Relaxed);
                        ctl.dirs.store(g("dirs"), Ordering::Relaxed);
                        ctl.bytes.store(g("bytes"), Ordering::Relaxed);
                        ctl.records_done.store(g("recordsDone"), Ordering::Relaxed);
                        ctl.records_total.store(g("recordsTotal"), Ordering::Relaxed);
                        ctl.set_phase(match p.get("phase").and_then(|v| v.as_str()) {
                            Some("readingMft") => crate::scan::ScanPhase::ReadingMft,
                            Some("buildingTree") => crate::scan::ScanPhase::BuildingTree,
                            _ => crate::scan::ScanPhase::Enumerating,
                        });
                    }
                }
                FRAME_ERROR => {
                    let v: serde_json::Value = serde_json::from_slice(&payload).unwrap_or_default();
                    let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error").to_string();
                    return Err(AppError::Helper(msg));
                }
                FRAME_SNAPSHOT => return crate::scan::snapshot::read_from(&mut reader),
                _ => return Err(AppError::Helper("unknown frame from helper".into())),
            }
        }
    })();
    drop(reader); // closes the pipe: a still-running helper stops on its next write
    let code = watcher.join().unwrap_or(1);
    match result {
        Ok(t) => Ok(t),
        Err(AppError::Helper(m)) if code != 0 && m.contains("exited") => {
            Err(AppError::Helper(format!("the elevated helper exited with code {code}")))
        }
        Err(e) => Err(e),
    }
}

fn launch_elevated(exe: &str, params: &str) -> Result<OwnedHandle> {
    use windows_sys::Win32::UI::Shell::*;
    let verb = wide("runas");
    let file = wide(exe);
    let p = wide(params);
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = p.as_ptr();
    sei.nShow = windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    if unsafe { ShellExecuteExW(&mut sei) } == 0 {
        let err = unsafe { GetLastError() };
        if err == ERROR_CANCELLED {
            return Err(AppError::ElevationRequired("the administrator prompt was declined".into()));
        }
        return Err(AppError::from_win32(err, "starting elevated helper", None));
    }
    OwnedHandle::new(sei.hProcess).ok_or_else(|| AppError::Helper("no process handle for helper".into()))
}

// ---------------------------------------------------------------------------
// Helper side (elevated)
// ---------------------------------------------------------------------------

/// Entry point used by `main()` of every executable. Returns `Some(exit code)`
/// when the process was started in helper mode (the caller must then exit
/// without starting the UI), `None` otherwise.
pub fn maybe_run_helper() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(ELEVATED_HELPER_SWITCH) {
        return None;
    }
    let cmd = match HelperCommand::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return Some(2);
        }
    };
    Some(match run_helper(cmd) {
        Ok(()) => 0,
        Err(_) => 1,
    })
}

fn run_helper(cmd: HelperCommand) -> Result<()> {
    let (letter, channel) = match cmd {
        HelperCommand::ScanNtfs { letter, channel } => (letter, channel),
        HelperCommand::ApplyOps { channel } => return helper_apply_ops(&channel),
    };
    let name = wide(pipe_name(&channel));
    let h = unsafe {
        CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    let pipe = OwnedHandle::new(h).ok_or_else(|| AppError::last_win32("connecting to caller", None))?;
    let writer = Arc::new(parking_lot::Mutex::new(Pipe(pipe)));
    let ctl = Arc::new(ScanControl::new());
    let done = Arc::new(AtomicBool::new(false));

    let progress = {
        let (writer, ctl, done) = (writer.clone(), ctl.clone(), done.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let p = ctl.snapshot();
                let json = serde_json::to_vec(&p).unwrap_or_default();
                if write_frame(&mut *writer.lock(), FRAME_PROGRESS, &json).is_err() {
                    ctl.cancel(); // caller went away
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        })
    };

    let root = format!("{letter}:\\");
    let result = crate::scan::run_scan(
        &root,
        &ScanOptions { method: crate::scan::ScanMethod::NtfsMft, follow_junctions: false },
        &ctl,
    );
    done.store(true, Ordering::SeqCst);
    let _ = progress.join();
    let mut w = writer.lock();
    match result {
        Ok(tree) => {
            write_frame(&mut *w, FRAME_SNAPSHOT, &[]).map_err(|e| AppError::io("helper channel", None, e))?;
            let mut buf = std::io::BufWriter::with_capacity(1 << 20, &mut *w);
            crate::scan::snapshot::write_to(&tree, &mut buf).map_err(|e| AppError::io("sending snapshot", None, e))?;
            buf.flush().map_err(|e| AppError::io("sending snapshot", None, e))?;
            Ok(())
        }
        Err(e) => {
            let json = serde_json::to_vec(&e.to_payload()).unwrap_or_default();
            let _ = write_frame(&mut *w, FRAME_ERROR, &json);
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Typed privileged operations (leftover removal, restore point)
// ---------------------------------------------------------------------------

const FRAME_REQUEST: u8 = 10;
const FRAME_RESULT: u8 = 11;
const FRAME_DONE: u8 = 12;
const MAX_OPS: usize = 5000;

/// The only privileged operations that exist. Each is re-validated by the
/// helper (Protection Engine, registry allowlist, service/task rules).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ElevatedOp {
    Removal { item: crate::leftovers::PendingOp },
    CreateRestorePoint { description: String },
    /// Enable/disable a startup entry (HKLM approval, task, service start type).
    Startup { op: crate::startup::StartupOp },
    /// End a process (or its tree) owned by another account / elevated. The
    /// helper re-identifies it (PID + image name + parent PID, and the path
    /// when the caller could read it) and never ends Windows components.
    EndProcess { pid: u32, name: String, parent: u32, path: Option<String>, tree: bool },
    /// Read the boot performance event log (read-only; needs admin rights).
    ReadBootPerformance,
    /// Remove analysed files of a machine-wide cleaner category (the helper
    /// re-checks every item against the category's fixed folders).
    Clean { category: String, items: Vec<crate::cleaner::CleanItem> },
    /// List the items of machine-wide cleaner categories (read-only).
    AnalyzeClean { categories: Vec<String> },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElevatedResult {
    pub index: usize,
    pub ok: bool,
    pub message: Option<String>,
    pub error_kind: Option<String>,
    /// Structured result of a successful operation (e.g. the processes ended).
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

fn read_frame(r: &mut impl Read) -> Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head).map_err(|_| AppError::Helper("the elevated helper exited without a result".into()))?;
    let len = u32::from_le_bytes(head[1..5].try_into().unwrap()) as usize;
    if len > 64 * 1024 * 1024 {
        return Err(AppError::Helper("invalid frame".into()));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).map_err(|_| AppError::Helper("channel closed unexpectedly".into()))?;
    Ok((head[0], payload))
}

/// Run privileged operations in one elevated helper (one UAC prompt).
pub fn run_elevated_ops(ops: &[ElevatedOp]) -> Result<Vec<ElevatedResult>> {
    if ops.is_empty() {
        return Ok(Vec::new());
    }
    if ops.len() > MAX_OPS {
        return Err(AppError::InvalidInput("too many operations".into()));
    }
    let channel = random_channel();
    let name = wide(pipe_name(&channel));
    let pipe = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            1 << 20,
            1 << 20,
            0,
            std::ptr::null(),
        )
    };
    let pipe = OwnedHandle::new(pipe).ok_or_else(|| AppError::last_win32("creating helper channel", None))?;
    let exe = std::env::current_exe().map_err(|e| AppError::io("locating executable", None, e))?;
    let process = launch_elevated(&exe.to_string_lossy(), &HelperCommand::ApplyOps { channel }.to_args())?;
    let connected = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (connected, name) = (connected.clone(), name.clone());
        std::thread::spawn(move || {
            unsafe { WaitForSingleObject(process.raw(), INFINITE) };
            if !connected.load(Ordering::SeqCst) {
                let h = unsafe { CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
                drop(OwnedHandle::new(h));
            }
            drop(process);
        })
    };
    let ok = unsafe { ConnectNamedPipe(pipe.raw(), std::ptr::null_mut()) } != 0;
    if !ok {
        let err = unsafe { GetLastError() };
        if err != ERROR_PIPE_CONNECTED {
            return Err(AppError::from_win32(err, "waiting for elevated helper", None));
        }
    }
    connected.store(true, Ordering::SeqCst);
    let mut io = Pipe(pipe);
    let req = serde_json::to_vec(ops).map_err(|e| AppError::Corrupt(e.to_string()))?;
    write_frame(&mut io, FRAME_REQUEST, &req).map_err(|e| AppError::io("sending request to helper", None, e))?;
    let mut results = Vec::with_capacity(ops.len());
    let outcome = loop {
        match read_frame(&mut io) {
            Ok((FRAME_RESULT, p)) => match serde_json::from_slice::<ElevatedResult>(&p) {
                Ok(r) => results.push(r),
                Err(e) => break Err(AppError::Corrupt(e.to_string())),
            },
            Ok((FRAME_DONE, _)) => break Ok(()),
            Ok((FRAME_ERROR, p)) => {
                let v: serde_json::Value = serde_json::from_slice(&p).unwrap_or_default();
                break Err(AppError::Helper(v.get("message").and_then(|m| m.as_str()).unwrap_or("error").into()));
            }
            Ok(_) => break Err(AppError::Helper("unexpected frame from helper".into())),
            Err(e) => break Err(e),
        }
    };
    drop(io);
    let _ = watcher.join();
    outcome.map(|_| results)
}

fn helper_apply_ops(channel: &str) -> Result<()> {
    let name = wide(pipe_name(channel));
    let h = unsafe {
        CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    let pipe = OwnedHandle::new(h).ok_or_else(|| AppError::last_win32("connecting to caller", None))?;
    let mut io = Pipe(pipe);
    let (ty, payload) = read_frame(&mut io)?;
    if ty != FRAME_REQUEST {
        return Err(AppError::Helper("expected a request".into()));
    }
    let ops: Vec<ElevatedOp> = match serde_json::from_slice(&payload) {
        Ok(o) => o,
        Err(e) => {
            let _ = write_frame(&mut io, FRAME_ERROR, &serde_json::to_vec(&serde_json::json!({"message": e.to_string()})).unwrap_or_default());
            return Err(AppError::Corrupt(e.to_string()));
        }
    };
    if ops.len() > MAX_OPS {
        return Err(AppError::InvalidInput("too many operations".into()));
    }
    for (index, op) in ops.iter().enumerate() {
        let mut data = None;
        let r = match op {
            ElevatedOp::Removal { item } => crate::leftovers::apply_pending(item),
            ElevatedOp::CreateRestorePoint { description } => crate::uninstall::create_restore_point(description),
            ElevatedOp::Startup { op } => crate::startup::apply_elevated(op),
            ElevatedOp::EndProcess { pid, name, parent, path, tree } => {
                crate::processes::end_identified(*pid, name, *parent, path.as_deref(), *tree).map(|list| data = serde_json::to_value(list).ok())
            }
            ElevatedOp::ReadBootPerformance => crate::bootperf::read().map(|r| data = serde_json::to_value(r).ok()),
            ElevatedOp::Clean { category, items } => crate::cleaner::clean_elevated(category, items).map(|o| data = serde_json::to_value(o).ok()),
            ElevatedOp::AnalyzeClean { categories } => crate::cleaner::analyze_admin(categories).map(|r| data = serde_json::to_value(r).ok()),
        };
        let res = ElevatedResult {
            index,
            ok: r.is_ok(),
            message: r.as_ref().err().map(|e| e.to_string()),
            error_kind: r.as_ref().err().map(|e| e.kind().to_string()),
            data,
        };
        let json = serde_json::to_vec(&res).unwrap_or_default();
        write_frame(&mut io, FRAME_RESULT, &json).map_err(|e| AppError::io("helper channel", None, e))?;
    }
    write_frame(&mut io, FRAME_DONE, &[]).map_err(|e| AppError::io("helper channel", None, e))?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_args_are_strict() {
        let ok: Vec<String> =
            [ELEVATED_HELPER_SWITCH, "scan-ntfs", "c", "0123456789abcdef0123456789abcdef"].map(String::from).to_vec();
        assert_eq!(
            HelperCommand::parse(&ok).unwrap(),
            HelperCommand::ScanNtfs { letter: 'C', channel: "0123456789abcdef0123456789abcdef".into() }
        );
        for bad in [
            vec![ELEVATED_HELPER_SWITCH, "scan-ntfs", "CD", "0123456789abcdef0123456789abcdef"],
            vec![ELEVATED_HELPER_SWITCH, "scan-ntfs", "1", "0123456789abcdef0123456789abcdef"],
            vec![ELEVATED_HELPER_SWITCH, "scan-ntfs", "C", "..\\..\\x"],
            vec![ELEVATED_HELPER_SWITCH, "scan-ntfs", "C", "0123456789abcdef0123456789abcdeg"],
            vec![ELEVATED_HELPER_SWITCH, "delete", "C:\\Windows"],
            vec![ELEVATED_HELPER_SWITCH, "scan-ntfs", "C", "0123456789abcdef0123456789abcdef", "extra"],
            vec![ELEVATED_HELPER_SWITCH, "apply-ops", "not-a-channel"],
        ] {
            let v: Vec<String> = bad.into_iter().map(String::from).collect();
            assert!(HelperCommand::parse(&v).is_err(), "{v:?}");
        }
        assert!(valid_channel(&random_channel()));
    }
}
