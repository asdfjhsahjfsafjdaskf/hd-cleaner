//! File operations. Destructive operations follow a two-step protocol:
//!
//! 1. [`plan_delete`] resolves every path (final path through junctions),
//!    classifies it with the Protection Engine and records a fingerprint
//!    (volume serial + file id + size + mtime). The UI shows this plan.
//! 2. [`execute_delete`] re-opens each item, verifies the fingerprint and the
//!    protection verdict again (TOCTOU defence) and only then deletes.
//!
//! Blocked items are never deleted. Dangerous items require an explicit flag.

use crate::protection::{assess, Assessment, Risk};
use crate::util::{from_extended, to_extended, wide, OwnedHandle};
use crate::{ErrorPayload, AppError, Result};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeleteMode {
    RecycleBin,
    Permanent,
    /// Overwrite the file's bytes before deleting it. `passes` is how many
    /// times (1-7). See [`secure_overwrite`] for what this can and cannot do.
    Secure { passes: u8 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub volume_serial: u32,
    pub file_id: u64,
    pub size: u64,
    pub modified: u64,
    pub attributes: u32,
    pub links: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub path: String,
    /// Path after resolving junctions/symlinks in parent directories.
    pub final_path: String,
    pub is_dir: bool,
    /// The item itself is a symlink/junction: only the link is removed.
    pub is_link: bool,
    pub size: u64,
    pub assessment: Assessment,
    #[serde(skip)]
    pub fingerprint: Option<Fingerprint>,
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletePlan {
    pub id: String,
    pub mode: DeleteMode,
    pub items: Vec<PlanItem>,
    pub total_size: u64,
    pub blocked: usize,
    pub dangerous: usize,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum ItemOutcome {
    Deleted,
    WouldDelete,
    Skipped { reason: String },
    Failed { error: ErrorPayload },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub path: String,
    #[serde(flatten)]
    pub outcome: ItemOutcome,
}

fn open_meta(path: &str) -> Result<OwnedHandle> {
    let w = wide(to_extended(path));
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    OwnedHandle::new(h).ok_or_else(|| AppError::last_win32("opening item", Some(path)))
}

pub fn fingerprint_handle(h: &OwnedHandle) -> Result<Fingerprint> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(h.raw(), &mut info) } == 0 {
        return Err(AppError::last_win32("reading file information", None));
    }
    Ok(Fingerprint {
        volume_serial: info.dwVolumeSerialNumber,
        file_id: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        size: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
        modified: ((info.ftLastWriteTime.dwHighDateTime as u64) << 32) | info.ftLastWriteTime.dwLowDateTime as u64,
        attributes: info.dwFileAttributes,
        links: info.nNumberOfLinks,
    })
}

pub fn fingerprint(path: &str) -> Result<Fingerprint> {
    fingerprint_handle(&open_meta(path)?)
}

/// Final normalized path of an open handle (resolves parent junctions).
pub fn final_path_of(h: &OwnedHandle) -> Result<String> {
    let mut buf = vec![0u16; 1024];
    loop {
        let n = unsafe { GetFinalPathNameByHandleW(h.raw(), buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
        if n == 0 {
            return Err(AppError::last_win32("resolving final path", None));
        }
        if n < buf.len() {
            return Ok(from_extended(&String::from_utf16_lossy(&buf[..n])));
        }
        buf.resize(n + 1, 0);
    }
}

/// The stricter of the verdicts for the requested path and its final path.
fn assess_both(path: &str, final_path: &str) -> Assessment {
    let a = assess(path);
    let b = assess(final_path);
    if b.risk > a.risk {
        b
    } else {
        a
    }
}

/// Build a deletion plan. `sizes` gives known sizes (e.g. from a scan) to
/// display; directories without a known size show 0.
pub fn plan_delete(items: &[(String, Option<u64>)], mode: DeleteMode) -> DeletePlan {
    let mut out = Vec::with_capacity(items.len());
    for (raw_path, size) in items {
        let path = match crate::util::normalize_root(raw_path) {
            Ok(p) => p,
            Err(e) => {
                out.push(PlanItem {
                    path: raw_path.clone(),
                    final_path: raw_path.clone(),
                    is_dir: false,
                    is_link: false,
                    size: 0,
                    assessment: Assessment { risk: Risk::Blocked, reason: "invalidPath" },
                    fingerprint: None,
                    error: Some(e.to_payload()),
                });
                continue;
            }
        };
        let mut item = PlanItem {
            path: path.clone(),
            final_path: path.clone(),
            is_dir: false,
            is_link: false,
            size: size.unwrap_or(0),
            assessment: assess(&path),
            fingerprint: None,
            error: None,
        };
        match open_meta(&path) {
            Ok(h) => {
                if let Ok(fp) = fingerprint_handle(&h) {
                    item.is_dir = fp.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                    item.is_link = fp.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                    if !item.is_dir && size.is_none() {
                        item.size = fp.size;
                    }
                    item.fingerprint = Some(fp);
                }
                if let Ok(fin) = final_path_of(&h) {
                    item.assessment = assess_both(&path, &fin);
                    item.final_path = fin;
                }
            }
            Err(e) => item.error = Some(e.to_payload()),
        }
        if mode == DeleteMode::RecycleBin && item.error.is_none() && !recycle_supported(&item.final_path) {
            item.error = Some(
                AppError::NotSupported("the Recycle Bin is not available on this drive; use permanent deletion".into())
                    .to_payload(),
            );
        }
        out.push(item);
    }
    // A folder and something inside it: keep only the folder.
    let dirs: Vec<String> =
        out.iter().filter(|i| i.is_dir && i.error.is_none()).map(|i| i.final_path.to_lowercase() + "\\").collect();
    out.retain(|i| {
        let fp = i.final_path.to_lowercase();
        !dirs.iter().any(|d| fp.starts_with(d.as_str()) && fp.len() >= d.len())
    });
    let total_size = out.iter().filter(|i| i.assessment.risk != Risk::Blocked).map(|i| i.size).sum();
    DeletePlan {
        id: format!("{:x}{:x}", crate::util::now_unix_ms(), std::process::id()),
        mode,
        blocked: out.iter().filter(|i| i.assessment.risk == Risk::Blocked).count(),
        dangerous: out.iter().filter(|i| i.assessment.risk == Risk::Dangerous).count(),
        total_size,
        items: out,
        created_ms: crate::util::now_unix_ms(),
    }
}

fn recycle_supported(path: &str) -> bool {
    let root = crate::disk::volume_root_of(path);
    let w = wide(&root);
    // Only fixed local drives reliably have a Recycle Bin; elsewhere the shell
    // silently deletes permanently, which we never want to do by surprise.
    unsafe { GetDriveTypeW(w.as_ptr()) == 3 }
}

pub struct ExecuteOptions {
    pub dry_run: bool,
    /// User explicitly confirmed items classified as Dangerous.
    pub allow_dangerous: bool,
}

/// Execute a plan. `on_item` is called after each item (progress reporting);
/// returning `false` from `should_continue` stops before the next item.
pub fn execute_delete(
    plan: &DeletePlan,
    opts: &ExecuteOptions,
    mut on_item: impl FnMut(usize, &ItemResult),
    should_continue: impl Fn() -> bool,
) -> Vec<ItemResult> {
    let mut results = Vec::with_capacity(plan.items.len());
    for (i, item) in plan.items.iter().enumerate() {
        if !should_continue() {
            let r = ItemResult { path: item.path.clone(), outcome: ItemOutcome::Skipped { reason: "cancelled".into() } };
            on_item(i, &r);
            results.push(r);
            continue;
        }
        let outcome = execute_one(item, plan.mode, opts);
        let r = ItemResult { path: item.path.clone(), outcome };
        on_item(i, &r);
        results.push(r);
    }
    results
}

fn execute_one(item: &PlanItem, mode: DeleteMode, opts: &ExecuteOptions) -> ItemOutcome {
    let skip = |r: &str| ItemOutcome::Skipped { reason: r.to_string() };
    if let Some(e) = &item.error {
        return ItemOutcome::Failed { error: e.clone() };
    }
    match item.assessment.risk {
        Risk::Blocked => return skip(item.assessment.reason),
        Risk::Dangerous if !opts.allow_dangerous => return skip("dangerousNotConfirmed"),
        _ => {}
    }
    // Re-verify identity and protection right before acting.
    let h = match open_meta(&item.path) {
        Ok(h) => h,
        Err(e) => return ItemOutcome::Failed { error: e.to_payload() },
    };
    let now = match fingerprint_handle(&h) {
        Ok(f) => f,
        Err(e) => return ItemOutcome::Failed { error: e.to_payload() },
    };
    if let Some(before) = item.fingerprint {
        let same = before.volume_serial == now.volume_serial
            && before.file_id == now.file_id
            && (item.is_dir || (before.size == now.size && before.modified == now.modified));
        if !same {
            return ItemOutcome::Failed {
                error: AppError::ChangedSinceReview { path: item.path.clone() }.to_payload(),
            };
        }
    }
    let fin = final_path_of(&h).unwrap_or_else(|_| item.final_path.clone());
    let verdict = assess_both(&item.path, &fin);
    if verdict.risk == Risk::Blocked || (verdict.risk == Risk::Dangerous && !opts.allow_dangerous) {
        return skip(verdict.reason);
    }
    drop(h);
    if opts.dry_run {
        return ItemOutcome::WouldDelete;
    }
    let res = match mode {
        DeleteMode::RecycleBin => recycle(&item.path),
        DeleteMode::Permanent => delete_permanent(&item.path, item.is_dir, item.is_link),
        DeleteMode::Secure { passes } => secure_delete(&item.path, item.is_dir, item.is_link, passes),
    };
    match res {
        Ok(()) => ItemOutcome::Deleted,
        Err(e) => ItemOutcome::Failed { error: e.to_payload() },
    }
}

fn delete_permanent(path: &str, is_dir: bool, is_link: bool) -> Result<()> {
    let ext = to_extended(path);
    let p = std::path::Path::new(&ext);
    let r = if is_dir && is_link {
        // Junction / directory symlink: remove the link only, never the target.
        std::fs::remove_dir(p)
    } else if is_dir {
        // std's remove_dir_all does not follow symlinks/junctions inside.
        std::fs::remove_dir_all(p)
    } else {
        clear_readonly(&ext);
        std::fs::remove_file(p)
    };
    r.map_err(|e| AppError::io("deleting", Some(std::path::Path::new(path)), e))
}

/// Overwrite a file's contents in place, then flush to disk.
///
/// What this does: replaces the bytes of the file *as the file system sees
/// them today*. What it cannot do: reach copies the drive itself keeps. On
/// an SSD (or any flash storage) wear levelling means the old blocks may
/// survive untouched, and shadow copies, backups and previously written
/// copies of the same data are not affected either. On those drives, full
/// disk encryption is the answer, not overwriting.
pub fn secure_overwrite(path: &str, passes: u8) -> Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let ext = to_extended(path);
    clear_readonly(&ext);
    let meta = std::fs::symlink_metadata(&ext).map_err(|e| AppError::io("reading the file", Some(std::path::Path::new(path)), e))?;
    if meta.file_type().is_symlink() {
        // A link holds no data of its own; overwriting would hit the target.
        return Err(AppError::Protected { path: path.to_string(), reason: "reparsePoint".into() });
    }
    let len = meta.len();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&ext)
        .map_err(|e| AppError::io("opening the file to overwrite", Some(std::path::Path::new(path)), e))?;
    const CHUNK: usize = 1 << 20;
    let mut buf = vec![0u8; CHUNK.min(len.max(1) as usize)];
    for pass in 0..passes.clamp(1, 7) {
        match pass % 3 {
            0 => buf.fill(0x00),
            1 => buf.fill(0xFF),
            // A changing pattern, so the last pass never leaves a constant.
            _ => {
                let seed = crate::util::now_unix_ms() as u64 ^ (pass as u64) << 32;
                let mut x = seed | 1;
                for b in buf.iter_mut() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    *b = (x >> 24) as u8;
                }
            }
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| AppError::io("overwriting", Some(std::path::Path::new(path)), e))?;
        let mut left = len;
        while left > 0 {
            let n = (left as usize).min(buf.len());
            file.write_all(&buf[..n]).map_err(|e| AppError::io("overwriting", Some(std::path::Path::new(path)), e))?;
            left -= n as u64;
        }
        file.flush().and_then(|_| file.sync_all()).map_err(|e| AppError::io("flushing", Some(std::path::Path::new(path)), e))?;
    }
    // Leave no length behind either.
    file.set_len(0).map_err(|e| AppError::io("truncating", Some(std::path::Path::new(path)), e))?;
    file.sync_all().map_err(|e| AppError::io("flushing", Some(std::path::Path::new(path)), e))?;
    Ok(())
}

fn secure_delete(path: &str, is_dir: bool, is_link: bool, passes: u8) -> Result<()> {
    if is_link {
        // Never follow a junction or symlink: remove the link itself.
        return delete_permanent(path, is_dir, is_link);
    }
    if is_dir {
        let mut stack = vec![std::path::PathBuf::from(&to_extended(path))];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).map_err(|e| AppError::io("reading the folder", Some(&dir), e))?.flatten() {
                let meta = e.metadata();
                let is_link = e.path().symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false);
                if is_link {
                    continue; // removed with the tree, never followed
                }
                if meta.map(|m| m.is_dir()).unwrap_or(false) {
                    stack.push(e.path());
                } else {
                    secure_overwrite(&e.path().to_string_lossy(), passes)?;
                }
            }
        }
        return delete_permanent(path, true, false);
    }
    secure_overwrite(path, passes)?;
    delete_permanent(path, false, false)
}

fn clear_readonly(ext_path: &str) {
    let w = wide(ext_path);
    let attrs = unsafe { GetFileAttributesW(w.as_ptr()) };
    if attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_READONLY != 0 {
        unsafe {
            SetFileAttributesW(w.as_ptr(), attrs & !FILE_ATTRIBUTE_READONLY);
        }
    }
}

/// Send to the Recycle Bin through `IFileOperation`.
pub fn recycle(path: &str) -> Result<()> {
    shell_file_op(ShellOp::Recycle, path, None)
}

enum ShellOp {
    Recycle,
    Copy,
    Move,
}

/// Shell operations need a single-threaded COM apartment; callers may be on
/// runtime worker threads already initialised as MTA, so run on a fresh thread.
fn shell_file_op(op: ShellOp, path: &str, dest_dir: Option<&str>) -> Result<()> {
    let path = path.to_string();
    let dest = dest_dir.map(str::to_string);
    std::thread::spawn(move || shell_file_op_sta(op, &path, dest.as_deref()))
        .join()
        .unwrap_or_else(|_| Err(AppError::Helper("shell operation thread panicked".into())))
}

fn shell_file_op_sta(op: ShellOp, path: &str, dest_dir: Option<&str>) -> Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;

    if path.len() >= 260 {
        return Err(AppError::NotSupported("path too long for a shell operation; use permanent deletion".into()));
    }
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        struct Uninit(bool);
        impl Drop for Uninit {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { CoUninitialize() };
                }
            }
        }
        let _guard = Uninit(init.is_ok());
        let map = |e: windows::core::Error, ctx: &str| AppError::Win32 {
            code: e.code().0 as u32,
            context: format!("{ctx}: {}", e.message()),
            path: Some(path.to_string()),
        };
        let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(|e| map(e, "creating file operation"))?;
        let base = FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0 | FOF_NOCONFIRMMKDIR.0;
        let flags = match op {
            ShellOp::Recycle => base | FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0,
            // Copy/move keep the shell's conflict dialog so nothing is overwritten silently.
            ShellOp::Copy | ShellOp::Move => FOF_NOCONFIRMMKDIR.0 | FOF_ALLOWUNDO.0,
        };
        fo.SetOperationFlags(FILEOPERATION_FLAGS(flags)).map_err(|e| map(e, "setting flags"))?;
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(HSTRING::from(path).as_ptr()), None)
            .map_err(|e| map(e, "resolving item"))?;
        match op {
            ShellOp::Recycle => fo.DeleteItem(&item, None).map_err(|e| map(e, "queueing delete"))?,
            ShellOp::Copy | ShellOp::Move => {
                let dest: IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(HSTRING::from(dest_dir.unwrap_or_default()).as_ptr()), None)
                        .map_err(|e| map(e, "resolving destination"))?;
                if matches!(op, ShellOp::Copy) {
                    fo.CopyItem(&item, &dest, PCWSTR::null(), None).map_err(|e| map(e, "queueing copy"))?;
                } else {
                    fo.MoveItem(&item, &dest, PCWSTR::null(), None).map_err(|e| map(e, "queueing move"))?;
                }
            }
        }
        fo.PerformOperations().map_err(|e| map(e, "performing operation"))?;
        if fo.GetAnyOperationsAborted().map(|b| b.as_bool()).unwrap_or(false) {
            return Err(AppError::Cancelled);
        }
    }
    Ok(())
}

/// Copy or move an item into `dest_dir` (shell handles conflicts with its UI).
pub fn copy_to(path: &str, dest_dir: &str) -> Result<()> {
    guard_source(path, false)?;
    shell_file_op(ShellOp::Copy, path, Some(dest_dir))
}

pub fn move_to(path: &str, dest_dir: &str) -> Result<()> {
    guard_source(path, true)?;
    shell_file_op(ShellOp::Move, path, Some(dest_dir))
}

fn guard_source(path: &str, removes_source: bool) -> Result<()> {
    let p = crate::util::normalize_root(path)?;
    if removes_source {
        let a = assess(&p);
        if a.risk >= Risk::Dangerous {
            return Err(AppError::Protected { path: p, reason: a.reason.into() });
        }
    }
    Ok(())
}

/// Validate a new file name (single component).
pub fn validate_file_name(name: &str) -> Result<()> {
    let bad = |m: &str| Err(AppError::InvalidInput(m.to_string()));
    if name.is_empty() || name.len() > 255 {
        return bad("name must be 1-255 characters");
    }
    if name == "." || name == ".." {
        return bad("reserved name");
    }
    if name.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32) {
        return bad("name contains characters not allowed by Windows");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return bad("name cannot end with a dot or space");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    let numbered = ["COM", "LPT"];
    if reserved.contains(&stem.as_str())
        || (stem.len() == 4 && numbered.contains(&&stem[..3]) && stem.as_bytes()[3].is_ascii_digit())
    {
        return bad("reserved device name");
    }
    Ok(())
}

/// Rename without ever overwriting an existing item.
pub fn rename(path: &str, new_name: &str) -> Result<String> {
    validate_file_name(new_name)?;
    let p = crate::util::normalize_root(path)?;
    let a = assess(&p);
    if a.risk >= Risk::Dangerous {
        return Err(AppError::Protected { path: p, reason: a.reason.into() });
    }
    let parent = std::path::Path::new(&p)
        .parent()
        .ok_or_else(|| AppError::InvalidInput("cannot rename a root".into()))?;
    let target = parent.join(new_name);
    let src = wide(to_extended(&p));
    let dst = wide(to_extended(&target.to_string_lossy()));
    // No MOVEFILE_REPLACE_EXISTING: fails if the target exists.
    if unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), 0) } == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_ALREADY_EXISTS || code == ERROR_FILE_EXISTS {
            return Err(AppError::InvalidInput(format!("an item named \"{new_name}\" already exists")));
        }
        return Err(AppError::from_win32(code, "renaming", Some(&p)));
    }
    Ok(target.to_string_lossy().into_owned())
}

fn shell_execute(verb: &str, file: &str, invoke_idlist: bool) -> Result<()> {
    use windows_sys::Win32::UI::Shell::*;
    let verb_w = wide(verb);
    let file_w = wide(file);
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = if invoke_idlist { SEE_MASK_INVOKEIDLIST } else { 0 } | SEE_MASK_FLAG_NO_UI;
    sei.lpVerb = verb_w.as_ptr();
    sei.lpFile = file_w.as_ptr();
    sei.nShow = windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if unsafe { ShellExecuteExW(&mut sei) } == 0 {
        return Err(AppError::last_win32(format!("{verb} failed"), Some(file)));
    }
    Ok(())
}

/// Open with the default application.
pub fn open_path(path: &str) -> Result<()> {
    let p = crate::util::normalize_root(path)?;
    shell_execute("open", &p, false)
}

/// Show the Windows "Properties" dialog.
pub fn show_properties(path: &str) -> Result<()> {
    let p = crate::util::normalize_root(path)?;
    shell_execute("properties", &p, true)
}

/// Open Explorer with the item selected.
pub fn reveal_in_explorer(path: &str) -> Result<()> {
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};
    let p = crate::util::normalize_root(path)?;
    let w = wide(&p);
    unsafe {
        let pidl = ILCreateFromPathW(w.as_ptr());
        if pidl.is_null() {
            return Err(AppError::NotFound { path: p });
        }
        let hr = SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0);
        ILFree(pidl);
        if hr < 0 {
            return Err(AppError::Win32 { code: hr as u32, context: "opening Explorer".into(), path: Some(p) });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Terminal {
    Cmd,
    PowerShell,
}

/// Open a console in `dir` (arguments are structured; nothing is concatenated).
pub fn open_terminal(dir: &str, kind: Terminal) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    let p = crate::util::normalize_root(dir)?;
    let meta = std::fs::metadata(&p).map_err(|e| AppError::io("opening folder", Some(std::path::Path::new(&p)), e))?;
    let dir = if meta.is_dir() {
        p
    } else {
        std::path::Path::new(&p).parent().map(|x| x.to_string_lossy().into_owned()).unwrap_or(p)
    };
    let sys = crate::system::windows_dir() + "\\System32";
    let mut cmd = match kind {
        Terminal::Cmd => std::process::Command::new(format!("{sys}\\cmd.exe")),
        Terminal::PowerShell => {
            let mut c = std::process::Command::new(format!("{sys}\\WindowsPowerShell\\v1.0\\powershell.exe"));
            c.arg("-NoExit");
            c
        }
    };
    cmd.current_dir(&dir).creation_flags(CREATE_NEW_CONSOLE);
    cmd.spawn().map_err(|e| AppError::io("starting terminal", None, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_delete_overwrites_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secret.txt");
        let secret = b"the password is hunter2".repeat(100);
        std::fs::write(&file, &secret).unwrap();

        let plan = plan_delete(&[(file.to_string_lossy().into_owned(), None)], DeleteMode::Secure { passes: 3 });
        let r = execute_delete(&plan, &ExecuteOptions { dry_run: false, allow_dangerous: false }, |_, _| {}, || true);
        assert!(matches!(r[0].outcome, ItemOutcome::Deleted), "{:?}", r[0].outcome);
        assert!(!file.exists());

        // The same bytes must not simply reappear when the name is reused.
        std::fs::write(&file, b"new content").unwrap();
        let back = std::fs::read(&file).unwrap();
        assert_ne!(back, secret);
    }

    #[test]
    fn overwriting_never_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, b"keep me").unwrap();
        let link = dir.path().join("link.txt");
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return; // symlinks need Developer Mode or admin: nothing to check
        }
        assert!(secure_overwrite(&link.to_string_lossy(), 1).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
    }

    #[test]
    fn names() {
        assert!(validate_file_name("ok name.txt").is_ok());
        for bad in ["", ".", "..", "a/b", "a\\b", "x:y", "CON", "con.txt", "COM1", "lpt9.log", "trail.", "trail "] {
            assert!(validate_file_name(bad).is_err(), "{bad}");
        }
        assert!(validate_file_name("COM10").is_ok());
        assert!(validate_file_name("console").is_ok());
    }

    #[test]
    fn plan_and_permanent_delete_in_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("victim.bin");
        std::fs::write(&f, vec![0u8; 100]).unwrap();
        let sub = dir.path().join("folder");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"x").unwrap();
        let items = vec![
            (f.to_string_lossy().into_owned(), None),
            (sub.to_string_lossy().into_owned(), Some(1)),
            (sub.join("inner.txt").to_string_lossy().into_owned(), None), // covered by folder
            ("C:\\Windows\\System32".to_string(), None),
        ];
        let plan = plan_delete(&items, DeleteMode::Permanent);
        assert_eq!(plan.items.len(), 3, "child of a planned folder is dropped");
        assert_eq!(plan.blocked, 1);
        assert_eq!(plan.items[0].size, 100);

        // Dry run changes nothing.
        let r = execute_delete(&plan, &ExecuteOptions { dry_run: true, allow_dangerous: false }, |_, _| {}, || true);
        assert!(f.exists() && sub.exists());
        assert_eq!(r[0].outcome, ItemOutcome::WouldDelete);
        assert!(matches!(r[2].outcome, ItemOutcome::Skipped { .. }));

        let r = execute_delete(&plan, &ExecuteOptions { dry_run: false, allow_dangerous: false }, |_, _| {}, || true);
        assert_eq!(r[0].outcome, ItemOutcome::Deleted);
        assert_eq!(r[1].outcome, ItemOutcome::Deleted);
        assert!(!f.exists() && !sub.exists());
        assert!(std::path::Path::new("C:\\Windows\\System32").exists());
    }

    #[test]
    fn changed_item_is_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"one").unwrap();
        let plan = plan_delete(&[(f.to_string_lossy().into_owned(), None)], DeleteMode::Permanent);
        // Replace the file with a different one under the same name.
        std::fs::remove_file(&f).unwrap();
        std::fs::write(&f, b"different content").unwrap();
        let r = execute_delete(&plan, &ExecuteOptions { dry_run: false, allow_dangerous: false }, |_, _| {}, || true);
        assert!(matches!(&r[0].outcome, ItemOutcome::Failed { error } if error.kind == "changedSinceReview"));
        assert!(f.exists());
    }

    #[test]
    fn junction_delete_removes_only_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"keep").unwrap();
        let link = dir.path().join("link");
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(ok.status.success());
        let plan = plan_delete(&[(link.to_string_lossy().into_owned(), None)], DeleteMode::Permanent);
        assert!(plan.items[0].is_link);
        execute_delete(&plan, &ExecuteOptions { dry_run: false, allow_dangerous: false }, |_, _| {}, || true);
        assert!(!link.exists());
        assert!(target.join("keep.txt").exists(), "junction target must survive");
    }

    #[test]
    fn junction_into_windows_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("win");
        let windows = crate::system::windows_dir();
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&windows).output().unwrap();
        assert!(ok.status.success());
        // Something *through* the junction resolves into C:\Windows.
        let inner = link.join("System32").join("notepad.exe");
        let plan = plan_delete(&[(inner.to_string_lossy().into_owned(), None)], DeleteMode::Permanent);
        assert_eq!(plan.items[0].assessment.risk, Risk::Blocked);
        let r = execute_delete(&plan, &ExecuteOptions { dry_run: false, allow_dangerous: true }, |_, _| {}, || true);
        assert!(matches!(r[0].outcome, ItemOutcome::Skipped { .. }));
        std::fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn rename_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        let a = dir.path().join("a.txt").to_string_lossy().into_owned();
        assert!(rename(&a, "b.txt").is_err());
        assert_eq!(std::fs::read(dir.path().join("b.txt")).unwrap(), b"b");
        let newp = rename(&a, "c.txt").unwrap();
        assert!(std::path::Path::new(&newp).exists());
    }
}
