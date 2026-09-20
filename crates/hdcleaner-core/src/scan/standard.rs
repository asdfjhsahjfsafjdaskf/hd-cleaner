//! Portable scanner built on documented Win32 directory enumeration.
//!
//! Works on any file system Windows exposes (NTFS without admin rights, FAT,
//! FAT32, exFAT, ReFS, USB drives, SMB shares, individual folders).
//!
//! Per directory it uses `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)`,
//! which returns names, sizes, *allocation sizes*, timestamps, attributes and
//! file IDs in large batches. If a file system rejects that information class
//! it falls back to `FindFirstFileExW(FIND_FIRST_EX_LARGE_FETCH)`.
//! Directories are processed in parallel on the rayon pool.

use super::builder::{EntryInfo, TreeBuilder};
use super::model::{flags, NodeId, ScanErrorSample, ScanMeta, ScanTree, ROOT};
use super::{FileSystemScanner, ScanControl, ScanOptions, ScanPhase};
use crate::util::{from_wide, join, to_extended, wide, OwnedHandle};
use crate::{AppError, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;

pub struct WindowsApiScanner;

const MAX_ERROR_SAMPLES: usize = 200;
const MAX_DEPTH: u32 = 512;
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
const FILE_ATTRIBUTE_RECALL_ON_OPEN_: u32 = 0x0004_0000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS_: u32 = 0x0040_0000;

struct Shared {
    builder: TreeBuilder,
    /// file id -> first node seen, used to detect additional hard links.
    /// `None` on file systems without hard links (FAT/exFAT...).
    file_ids: Option<HashMap<u64, NodeId>>,
}

struct Ctx<'a> {
    shared: Mutex<Shared>,
    ctl: &'a ScanControl,
    follow_reparse: bool,
    cluster: u64,
    errors: Mutex<Vec<ScanErrorSample>>,
    find_fallback: AtomicBool,
}

struct RawEntry {
    name: String,
    info: EntryInfo,
    file_id: u64,
    descend: bool,
}

impl FileSystemScanner for WindowsApiScanner {
    fn name(&self) -> &'static str {
        "standard"
    }

    fn scan(&self, root: &str, opts: &ScanOptions, ctl: &ScanControl) -> Result<ScanTree> {
        let root = crate::util::normalize_root(root)?;
        let (fs, cluster) = crate::disk::volume_fs_for_path(&root);
        let hard_links = fs.eq_ignore_ascii_case("NTFS") || fs.eq_ignore_ascii_case("ReFS");

        // Validate the root up-front for a precise error.
        let root_w = wide(to_extended(&root));
        let attrs = unsafe { GetFileAttributesW(root_w.as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES {
            return Err(AppError::last_win32("opening scan root", Some(&root)));
        }
        if attrs & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(AppError::InvalidInput(format!("not a directory: {root}")));
        }

        ctl.set_phase(ScanPhase::Enumerating);
        let ctx = Ctx {
            shared: Mutex::new(Shared {
                builder: TreeBuilder::new(&root, 1 << 16),
                file_ids: hard_links.then(HashMap::new),
            }),
            ctl,
            follow_reparse: opts.follow_junctions,
            cluster: cluster as u64,
            errors: Mutex::new(Vec::new()),
            find_fallback: AtomicBool::new(false),
        };
        let root_ext = to_extended(&root);
        rayon::scope(|s| scan_dir(&ctx, root_ext, ROOT, 0, s));
        ctl.check()?;

        ctl.set_phase(ScanPhase::BuildingTree);
        let Ctx { shared, errors, .. } = ctx;
        let Shared { builder, file_ids } = shared.into_inner();
        drop(file_ids);
        let meta = ScanMeta {
            root_path: root,
            method: "standard".into(),
            error_count: ctl.errors.load(Ordering::Relaxed),
            error_samples: errors.into_inner(),
            ..Default::default()
        };
        Ok(builder.finish(meta))
    }
}

fn scan_dir<'s>(ctx: &'s Ctx<'s>, path: String, node: NodeId, depth: u32, s: &rayon::Scope<'s>) {
    if ctx.ctl.is_cancelled() {
        return;
    }
    ctx.ctl.dirs.fetch_add(1, Ordering::Relaxed);
    ctx.ctl.set_current(&path);

    let entries = if ctx.find_fallback.load(Ordering::Relaxed) {
        read_dir_find(&path, ctx)
    } else {
        match read_dir_by_handle(&path, ctx) {
            Err(AppError::NotSupported(_)) => {
                ctx.find_fallback.store(true, Ordering::Relaxed);
                read_dir_find(&path, ctx)
            }
            other => other,
        }
    };
    let entries = match entries {
        Ok(e) => e,
        Err(e) => {
            ctx.ctl.errors.fetch_add(1, Ordering::Relaxed);
            let mut errs = ctx.errors.lock();
            if errs.len() < MAX_ERROR_SAMPLES {
                errs.push(ScanErrorSample { path: crate::util::from_extended(&path), message: e.to_string() });
            }
            drop(errs);
            ctx.shared.lock().builder.node_mut(node).flags |= flags::UNREADABLE;
            return;
        }
    };

    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut subdirs: Vec<(NodeId, String)> = Vec::new();
    {
        let mut guard = ctx.shared.lock();
        let Shared { builder, file_ids } = &mut *guard;
        for mut e in entries {
            if !e.info.is_dir {
                files += 1;
                bytes += e.info.size;
                if let (Some(ids), true) = (file_ids.as_mut(), e.file_id != 0) {
                    if let Some(&first) = ids.get(&e.file_id) {
                        e.info.flags |= flags::HARDLINK_DUP | flags::MULTI_LINK;
                        builder.node_mut(first).flags |= flags::MULTI_LINK;
                    }
                }
            }
            let id = builder.add(node, &e.name, &e.info);
            if !e.info.is_dir {
                if let (Some(ids), true) = (file_ids.as_mut(), e.file_id != 0) {
                    ids.entry(e.file_id).or_insert(id);
                }
            }
            if e.descend && depth < MAX_DEPTH {
                subdirs.push((id, join(&path, &e.name)));
            }
        }
    }
    ctx.ctl.files.fetch_add(files, Ordering::Relaxed);
    ctx.ctl.bytes.fetch_add(bytes, Ordering::Relaxed);

    for (id, p) in subdirs {
        s.spawn(move |s| scan_dir(ctx, p, id, depth + 1, s));
    }
}

fn classify(attributes: u32, reparse_tag: u32, is_dir: bool, follow: bool) -> (u16, bool) {
    let mut f = 0u16;
    let mut descend = is_dir;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        f |= flags::REPARSE;
        // Name-surrogate tags (junctions, symlinks...) point somewhere else.
        // Other tags (cloud files, dedup, containers) hold real content.
        let surrogate = reparse_tag & 0x2000_0000 != 0;
        if reparse_tag == IO_REPARSE_TAG_MOUNT_POINT {
            f |= flags::MOUNT_POINT;
        } else if reparse_tag == IO_REPARSE_TAG_SYMLINK {
            f |= flags::SYMLINK;
        }
        if is_dir && surrogate && !follow {
            descend = false;
            f |= flags::NOT_FOLLOWED;
        }
    }
    if attributes & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN_ | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS_) != 0 {
        f |= flags::CLOUD;
    }
    if attributes & FILE_ATTRIBUTE_SPARSE_FILE != 0 {
        f |= flags::SPARSE;
    }
    if attributes & FILE_ATTRIBUTE_COMPRESSED != 0 {
        f |= flags::COMPRESSED;
    }
    (f, descend)
}

fn read_dir_by_handle(path: &str, ctx: &Ctx) -> Result<Vec<RawEntry>> {
    let w = wide(path);
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    let display = crate::util::from_extended(path);
    let Some(h) = OwnedHandle::new(h) else {
        return Err(AppError::last_win32("opening directory", Some(&display)));
    };

    let mut buf: Vec<u64> = vec![0; 128 * 1024 / 8];
    let buf_bytes = (buf.len() * 8) as u32;
    let mut out = Vec::new();
    let mut class = FileIdBothDirectoryRestartInfo;
    let mut first = true;
    loop {
        let ok = unsafe { GetFileInformationByHandleEx(h.raw(), class, buf.as_mut_ptr().cast(), buf_bytes) };
        class = FileIdBothDirectoryInfo;
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NO_MORE_FILES {
                break;
            }
            if first && matches!(err, ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION) {
                return Err(AppError::NotSupported("FileIdBothDirectoryInfo".into()));
            }
            return Err(AppError::from_win32(err, "enumerating directory", Some(&display)));
        }
        first = false;
        let base = buf.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            // SAFETY: the API fills the buffer with a chain of properly aligned
            // FILE_ID_BOTH_DIR_INFO records linked by NextEntryOffset.
            let rec = unsafe { &*(base.add(offset) as *const FILE_ID_BOTH_DIR_INFO) };
            let name_len = rec.FileNameLength as usize / 2;
            let name_ptr = std::ptr::addr_of!(rec.FileName) as *const u16;
            let name_w = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
            let skip = name_w == [b'.' as u16] || name_w == [b'.' as u16, b'.' as u16];
            if !skip {
                let is_dir = rec.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                let tag = if rec.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { rec.EaSize } else { 0 };
                let (f, descend) = classify(rec.FileAttributes, tag, is_dir, ctx.follow_reparse);
                out.push(RawEntry {
                    name: from_wide(name_w),
                    info: EntryInfo {
                        is_dir,
                        size: if is_dir { 0 } else { rec.EndOfFile.max(0) as u64 },
                        alloc: if is_dir { 0 } else { rec.AllocationSize.max(0) as u64 },
                        created: rec.CreationTime as u64,
                        modified: rec.LastWriteTime as u64,
                        accessed: rec.LastAccessTime as u64,
                        attributes: rec.FileAttributes,
                        flags: f,
                        links: 0,
                    },
                    file_id: rec.FileId as u64,
                    descend,
                });
            }
            if rec.NextEntryOffset == 0 {
                break;
            }
            offset += rec.NextEntryOffset as usize;
        }
        if ctx.ctl.is_cancelled() {
            return Err(AppError::Cancelled);
        }
    }
    Ok(out)
}

fn read_dir_find(path: &str, ctx: &Ctx) -> Result<Vec<RawEntry>> {
    let pattern = wide(join(path, "*"));
    let display = crate::util::from_extended(path);
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let h = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            &mut data as *mut _ as *mut _,
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        let err = unsafe { GetLastError() };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(Vec::new());
        }
        return Err(AppError::from_win32(err, "enumerating directory", Some(&display)));
    }
    struct FindGuard(HANDLE);
    impl Drop for FindGuard {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }
    let _g = FindGuard(h);
    let ft = |f: &FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    let mut out = Vec::new();
    loop {
        let n = data.cFileName.iter().position(|&c| c == 0).unwrap_or(data.cFileName.len());
        let name_w = &data.cFileName[..n];
        let skip = name_w == [b'.' as u16] || name_w == [b'.' as u16, b'.' as u16];
        if !skip {
            let is_dir = data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let tag = if data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { data.dwReserved0 } else { 0 };
            let (f, descend) = classify(data.dwFileAttributes, tag, is_dir, ctx.follow_reparse);
            let size = ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
            let cloud = f & flags::CLOUD != 0;
            out.push(RawEntry {
                name: from_wide(name_w),
                info: EntryInfo {
                    is_dir,
                    size: if is_dir { 0 } else { size },
                    // FindFirstFile does not report allocation; estimate by clusters.
                    alloc: if is_dir || cloud { 0 } else { size.div_ceil(ctx.cluster) * ctx.cluster },
                    created: ft(&data.ftCreationTime),
                    modified: ft(&data.ftLastWriteTime),
                    accessed: ft(&data.ftLastAccessTime),
                    attributes: data.dwFileAttributes,
                    flags: f,
                    links: 0,
                },
                file_id: 0,
                descend,
            });
        }
        if unsafe { FindNextFileW(h, &mut data) } == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NO_MORE_FILES {
                break;
            }
            return Err(AppError::from_win32(err, "enumerating directory", Some(&display)));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::ScanControl;
    use std::fs;

    #[test]
    fn scans_temp_tree_with_hardlinks_and_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        fs::write(root.join("a.txt"), vec![1u8; 1000]).unwrap();
        fs::write(root.join("sub").join("b.bin"), vec![2u8; 50_000]).unwrap();
        fs::write(root.join("sub").join("deep").join("ção 日本.mp4"), vec![3u8; 7]).unwrap();
        // Hard link to b.bin must not be counted twice.
        fs::hard_link(root.join("sub").join("b.bin"), root.join("b-link.bin")).unwrap();

        let ctl = ScanControl::new();
        let tree = WindowsApiScanner
            .scan(root.to_str().unwrap(), &ScanOptions::default(), &ctl)
            .unwrap();
        assert_eq!(tree.meta.files, 4);
        assert_eq!(tree.meta.dirs, 2);
        assert_eq!(tree.meta.total_size, 1000 + 50_000 + 7);
        let uni = tree.find_path(&format!("{}\\sub\\deep\\ção 日本.mp4", root.display())).unwrap();
        assert_eq!(tree.node(uni).size, 7);
        assert_eq!(tree.ext(uni), "mp4");
    }

    #[test]
    fn junctions_are_not_followed_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("target").join("f.bin"), vec![0u8; 4096]).unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("jn"))
            .arg(root.join("target"))
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J failed");
        let ctl = ScanControl::new();
        let tree = WindowsApiScanner
            .scan(root.to_str().unwrap(), &ScanOptions::default(), &ctl)
            .unwrap();
        assert_eq!(tree.meta.total_size, 4096, "junction target must be counted once");
        let jn = tree.find_path(&format!("{}\\jn", root.display())).unwrap();
        assert!(tree.node(jn).flags & flags::MOUNT_POINT != 0);
        assert!(tree.node(jn).flags & flags::NOT_FOLLOWED != 0);
    }

    #[test]
    fn long_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = dir.path().to_path_buf();
        for i in 0..12 {
            p.push(format!("very_long_directory_name_segment_number_{i:02}"));
        }
        let ext = crate::util::to_extended(p.to_str().unwrap());
        fs::create_dir_all(&ext).unwrap();
        fs::write(format!("{ext}\\file.dat"), b"12345").unwrap();
        assert!(p.to_str().unwrap().len() > 260);
        let ctl = ScanControl::new();
        let tree = WindowsApiScanner
            .scan(dir.path().to_str().unwrap(), &ScanOptions::default(), &ctl)
            .unwrap();
        assert_eq!(tree.meta.files, 1);
        assert_eq!(tree.meta.total_size, 5);
    }

    #[test]
    fn cancellation() {
        let ctl = ScanControl::new();
        ctl.cancel();
        let dir = tempfile::tempdir().unwrap();
        let r = WindowsApiScanner.scan(dir.path().to_str().unwrap(), &ScanOptions::default(), &ctl);
        assert!(matches!(r, Err(AppError::Cancelled)));
    }
}
