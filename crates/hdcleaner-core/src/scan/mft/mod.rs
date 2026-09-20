//! NTFS fast scanner: reads the Master File Table directly from the volume
//! instead of enumerating directories.
//!
//! 1. Open `\\.\X:` (requires administrator rights).
//! 2. `FSCTL_GET_NTFS_VOLUME_DATA` → cluster size, record size, MFT location.
//! 3. Read MFT record 0 (`$MFT` itself) and decode its `$DATA` runlist.
//! 4. Stream the MFT extents in large sequential reads on a reader thread,
//!    parse records in parallel (rayon) into compact per-record info.
//! 5. Merge extension records into their base record, then build the
//!    directory tree breadth-first from the root (record 5).
//!
//! Hard links (several `$FILE_NAME` in different directories) produce one node
//! per link; only the first is counted in totals. Junction targets are never
//! double counted because the MFT only stores each file once.

pub mod parse;

use super::builder::{EntryInfo, TreeBuilder};
use super::model::{flags, NodeId, ScanMeta, ScanTree, ROOT};
use super::{FileSystemScanner, ScanControl, ScanOptions, ScanPhase};
use crate::util::{wide, OwnedHandle};
use crate::{AppError, Result};
use parse::*;
use rayon::prelude::*;
use std::sync::atomic::Ordering;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};
use windows_sys::Win32::System::IO::DeviceIoControl;

pub struct NtfsFastScanner;

const ROOT_RECORD: u32 = 5;
const READ_CHUNK: usize = 8 * 1024 * 1024;
const PARSE_BATCH: usize = 256;

// Per-record information after merging extension records.
const RI_IN_USE: u8 = 1;
const RI_DIR: u8 = 2;
const RI_SPARSE: u8 = 4;
const RI_COMPRESSED: u8 = 8;
const RI_HAS_SIZE: u8 = 16;

#[derive(Clone, Copy, Default)]
struct RecInfo {
    size: u64,
    alloc: u64,
    created: u64,
    modified: u64,
    accessed: u64,
    attributes: u32,
    flags: u8,
}

#[derive(Clone, Copy)]
struct NameEntry {
    record: u32,
    parent: u32,
    name_off: u32,
    name_len: u16,
    namespace: u8,
}

#[derive(Default)]
struct BatchOut {
    infos: Vec<(u32, ParsedRecordLite)>,
    names: Vec<NameEntry>,
    pool: String,
}

/// `ParsedRecord` without the name vector (names go to the batch pool).
struct ParsedRecordLite {
    base: Option<u64>,
    in_use: bool,
    is_dir: bool,
    has_std_info: bool,
    created: u64,
    modified: u64,
    accessed: u64,
    attributes: u32,
    data_size: Option<u64>,
    alloc: u64,
    sparse: bool,
    compressed: bool,
}

pub struct VolumeGeometry {
    pub bytes_per_sector: u32,
    pub bytes_per_cluster: u32,
    pub record_size: u32,
    pub mft_start_lcn: u64,
    pub mft_valid_len: u64,
}

pub fn open_volume(letter: char) -> Result<OwnedHandle> {
    let path = wide(format!(r"\\.\{letter}:"));
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    OwnedHandle::new(h).ok_or_else(|| {
        let e = AppError::last_win32("opening volume", Some(&format!("{letter}:")));
        match e {
            AppError::AccessDenied { .. } if !crate::system::is_elevated() => AppError::ElevationRequired(
                "reading the NTFS Master File Table requires administrator rights".into(),
            ),
            other => other,
        }
    })
}

pub fn volume_geometry(h: &OwnedHandle) -> Result<VolumeGeometry> {
    let mut data: NTFS_VOLUME_DATA_BUFFER = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h.raw(),
            FSCTL_GET_NTFS_VOLUME_DATA,
            std::ptr::null(),
            0,
            &mut data as *mut _ as *mut _,
            std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    } != 0;
    if !ok {
        return Err(AppError::last_win32("querying NTFS volume data", None));
    }
    let g = VolumeGeometry {
        bytes_per_sector: data.BytesPerSector,
        bytes_per_cluster: data.BytesPerCluster,
        record_size: data.BytesPerFileRecordSegment,
        mft_start_lcn: data.MftStartLcn as u64,
        mft_valid_len: data.MftValidDataLength as u64,
    };
    if g.record_size < 512 || g.record_size > 65536 || !g.record_size.is_power_of_two() || g.bytes_per_cluster == 0 {
        return Err(AppError::Corrupt(format!("unexpected NTFS geometry: record size {}", g.record_size)));
    }
    Ok(g)
}

fn read_at(h: &OwnedHandle, offset: u64, buf: &mut [u8]) -> Result<()> {
    let mut pos = 0usize;
    while pos < buf.len() {
        let mut ov: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
        let off = offset + pos as u64;
        ov.Anonymous.Anonymous.Offset = off as u32;
        ov.Anonymous.Anonymous.OffsetHigh = (off >> 32) as u32;
        let want = (buf.len() - pos).min(64 * 1024 * 1024) as u32;
        let mut read = 0u32;
        let ok = unsafe { ReadFile(h.raw(), buf[pos..].as_mut_ptr(), want, &mut read, &mut ov) } != 0;
        if !ok {
            return Err(AppError::last_win32("reading the MFT", None));
        }
        if read == 0 {
            return Err(AppError::Corrupt("unexpected end of volume while reading the MFT".into()));
        }
        pos += read as usize;
    }
    Ok(())
}

fn parse_batch(first_record: u64, data: &mut [u8], record_size: usize, cluster: u64) -> BatchOut {
    let mut out = BatchOut::default();
    for (i, rec) in data.chunks_exact_mut(record_size).enumerate() {
        let number = first_record + i as u64;
        if number > u32::MAX as u64 || !apply_fixup(rec) {
            continue;
        }
        let Some(p) = parse_record(rec) else { continue };
        if !p.in_use {
            continue;
        }
        let owner = p.base.unwrap_or(number);
        if owner > u32::MAX as u64 {
            continue;
        }
        for n in &p.names {
            if n.parent > u32::MAX as u64 || n.name.len() > u16::MAX as usize {
                continue;
            }
            out.names.push(NameEntry {
                record: owner as u32,
                parent: n.parent as u32,
                name_off: out.pool.len() as u32,
                name_len: n.name.len() as u16,
                namespace: n.namespace,
            });
            out.pool.push_str(&n.name);
        }
        out.infos.push((
            number as u32,
            ParsedRecordLite {
                base: p.base,
                in_use: p.in_use,
                is_dir: p.is_dir,
                has_std_info: p.has_std_info,
                created: p.created,
                modified: p.modified,
                accessed: p.accessed,
                attributes: p.attributes,
                data_size: p.data_size,
                alloc: p.alloc_clusters * cluster + p.alloc_extra_bytes,
                sparse: p.sparse,
                compressed: p.compressed,
            },
        ));
    }
    out
}

/// Accumulated MFT content, independent of how it was read (volume or test data).
struct MftContent {
    infos: Vec<RecInfo>,
    names: Vec<NameEntry>,
    pool: String,
}

impl MftContent {
    fn with_records(n: usize) -> Self {
        MftContent { infos: vec![RecInfo::default(); n], names: Vec::new(), pool: String::new() }
    }

    fn merge(&mut self, batch: BatchOut, ctl: &ScanControl) {
        let base_off = self.pool.len() as u32;
        self.pool.push_str(&batch.pool);
        self.names.extend(batch.names.into_iter().map(|mut n| {
            n.name_off += base_off;
            n
        }));
        let mut files = 0u64;
        let mut dirs = 0u64;
        let mut bytes = 0u64;
        for (number, p) in batch.infos {
            let owner = p.base.map(|b| b as usize).unwrap_or(number as usize);
            let Some(info) = self.infos.get_mut(owner) else { continue };
            if p.base.is_none() {
                if p.in_use {
                    info.flags |= RI_IN_USE;
                    if p.is_dir {
                        info.flags |= RI_DIR;
                        dirs += 1;
                    } else {
                        files += 1;
                    }
                }
            }
            if p.has_std_info {
                info.created = p.created;
                info.modified = p.modified;
                info.accessed = p.accessed;
                info.attributes = p.attributes;
            }
            if let Some(s) = p.data_size {
                info.size = s;
                info.flags |= RI_HAS_SIZE;
                bytes += s;
            }
            info.alloc += p.alloc;
            if p.sparse {
                info.flags |= RI_SPARSE;
            }
            if p.compressed {
                info.flags |= RI_COMPRESSED;
            }
        }
        ctl.files.fetch_add(files, Ordering::Relaxed);
        ctl.dirs.fetch_add(dirs, Ordering::Relaxed);
        ctl.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Build the directory tree breadth-first from the root record.
    fn build(self, root_path: &str, ctl: &ScanControl) -> Result<ScanTree> {
        let MftContent { infos, names, pool } = self;
        let n_rec = infos.len();
        if n_rec <= ROOT_RECORD as usize || infos[ROOT_RECORD as usize].flags & RI_DIR == 0 {
            return Err(AppError::Corrupt("NTFS root directory record not found".into()));
        }

        // Prefer long names: drop DOS-only (8.3) aliases when a record has a
        // Win32/POSIX name.
        let mut has_long = vec![false; n_rec];
        for n in &names {
            if n.namespace != NS_DOS {
                if let Some(h) = has_long.get_mut(n.record as usize) {
                    *h = true;
                }
            }
        }
        let valid = |n: &NameEntry| -> bool {
            let r = n.record as usize;
            r < n_rec
                && (n.parent as usize) < n_rec
                && n.record != n.parent
                && infos[r].flags & RI_IN_USE != 0
                && (n.namespace != NS_DOS || !has_long[r])
        };

        // Link counts and child lists keyed by parent record.
        let mut link_count = vec![0u16; n_rec];
        let mut head = vec![u32::MAX; n_rec];
        let mut next = vec![u32::MAX; names.len()];
        for (i, n) in names.iter().enumerate().rev() {
            if !valid(n) {
                continue;
            }
            link_count[n.record as usize] = link_count[n.record as usize].saturating_add(1);
            next[i] = head[n.parent as usize];
            head[n.parent as usize] = i as u32;
        }
        drop(has_long);

        let mut builder = TreeBuilder::new(root_path, names.len() + 1);
        {
            let r = &infos[ROOT_RECORD as usize];
            let root = builder.root_mut();
            root.created = r.created;
            root.modified = r.modified;
            root.accessed = r.accessed;
            root.attributes = r.attributes;
        }
        let mut counted = vec![false; n_rec];
        let mut visited_dir = vec![false; n_rec];
        visited_dir[ROOT_RECORD as usize] = true;
        let mut queue: std::collections::VecDeque<(u32, NodeId)> = std::collections::VecDeque::new();
        queue.push_back((ROOT_RECORD, ROOT));
        let mut placed = 0u64;
        while let Some((rec, node)) = queue.pop_front() {
            if placed & 0xFFFF == 0 {
                ctl.check()?;
            }
            let mut cur = head[rec as usize];
            while cur != u32::MAX {
                let n = names[cur as usize];
                cur = next[cur as usize];
                let r = n.record as usize;
                let info = &infos[r];
                let is_dir = info.flags & RI_DIR != 0;
                if is_dir && visited_dir[r] {
                    continue; // corrupt: directory reachable twice
                }
                let name = &pool[n.name_off as usize..n.name_off as usize + n.name_len as usize];
                let mut f = 0u16;
                if !is_dir {
                    if counted[r] {
                        f |= flags::HARDLINK_DUP;
                    }
                    counted[r] = true;
                    if link_count[r] > 1 {
                        f |= flags::MULTI_LINK;
                    }
                }
                if info.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    f |= flags::REPARSE;
                }
                if info.flags & RI_SPARSE != 0 {
                    f |= flags::SPARSE;
                }
                if info.flags & RI_COMPRESSED != 0 {
                    f |= flags::COMPRESSED;
                }
                if info.attributes & (FILE_ATTRIBUTE_OFFLINE | 0x0004_0000 | 0x0040_0000) != 0 {
                    f |= flags::CLOUD;
                }
                if r < 24 && rec == ROOT_RECORD || name.starts_with('$') && rec == ROOT_RECORD {
                    f |= flags::METAFILE;
                }
                let id = builder.add(
                    node,
                    name,
                    &EntryInfo {
                        is_dir,
                        size: info.size,
                        alloc: info.alloc,
                        created: info.created,
                        modified: info.modified,
                        accessed: info.accessed,
                        attributes: info.attributes,
                        flags: f,
                        links: link_count[r],
                    },
                );
                placed += 1;
                if is_dir {
                    visited_dir[r] = true;
                    queue.push_back((n.record, id));
                }
            }
        }
        let orphans = infos
            .iter()
            .enumerate()
            .filter(|(i, inf)| inf.flags & RI_IN_USE != 0 && inf.flags & RI_DIR == 0 && !counted[*i] && *i > 26)
            .count();
        let mut meta = ScanMeta { root_path: root_path.to_string(), method: "ntfsMft".into(), ..Default::default() };
        if orphans > 0 {
            tracing::info!(orphans, "MFT records not reachable from the root were skipped");
            meta.notes.push(format!("orphanRecords:{orphans}"));
        }
        ctl.set_phase(ScanPhase::BuildingTree);
        Ok(builder.finish(meta))
    }
}

impl FileSystemScanner for NtfsFastScanner {
    fn name(&self) -> &'static str {
        "ntfsMft"
    }

    fn scan(&self, root: &str, _opts: &ScanOptions, ctl: &ScanControl) -> Result<ScanTree> {
        let root = crate::util::normalize_root(root)?;
        let letter = root.chars().next().unwrap_or('C');
        if !(root.len() == 3 && root.ends_with(":\\")) {
            return Err(AppError::NotSupported("MFT scan needs a volume root".into()));
        }
        let vol = open_volume(letter)?;
        let g = volume_geometry(&vol)?;
        ctl.set_phase(ScanPhase::ReadingMft);
        let rs = g.record_size as usize;
        let cluster = g.bytes_per_cluster as u64;

        // Record 0 describes the MFT itself.
        let mut rec0 = vec![0u8; rs.max(g.bytes_per_sector as usize)];
        read_at(&vol, g.mft_start_lcn * cluster, &mut rec0)?;
        let rec0 = &mut rec0[..rs];
        if !apply_fixup(rec0) {
            return Err(AppError::Corrupt("MFT record 0 is invalid".into()));
        }
        let (runs, mft_size) = unnamed_data_runs(rec0).ok_or_else(|| {
            AppError::NotSupported("MFT data runs are stored in an attribute list (highly fragmented MFT)".into())
        })?;
        let total_bytes = g.mft_valid_len.min(mft_size);
        let total_records = (total_bytes / rs as u64) as usize;
        ctl.records_total.store(total_records as u64, Ordering::Relaxed);

        // Reader thread: sequential reads of whole records, carrying partial
        // records across extent boundaries (possible when record > cluster).
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(u64, Vec<u8>)>>(3);
        let mut content = MftContent::with_records(total_records);
        std::thread::scope(|scope| -> Result<()> {
            let vol = &vol;
            scope.spawn(move || {
                let mut carry: Vec<u8> = Vec::new();
                let mut next_record: u64 = 0;
                let mut remaining = total_bytes;
                'outer: for run in &runs {
                    let run_bytes = run.clusters * cluster;
                    let mut done = 0u64;
                    while done < run_bytes && remaining > 0 {
                        if ctl.is_cancelled() {
                            let _ = tx.send(Err(AppError::Cancelled));
                            break 'outer;
                        }
                        let take = (run_bytes - done).min(READ_CHUNK as u64).min(remaining.div_ceil(cluster) * cluster);
                        let mut buf = std::mem::take(&mut carry);
                        let start = buf.len();
                        buf.resize(start + take as usize, 0);
                        match run.lcn {
                            Some(lcn) => {
                                if let Err(e) = read_at(vol, lcn * cluster + done, &mut buf[start..]) {
                                    let _ = tx.send(Err(e));
                                    break 'outer;
                                }
                            }
                            None => {} // sparse run: zeros, records will be rejected
                        }
                        done += take;
                        let usable = (buf.len() as u64).min(remaining + start as u64) as usize;
                        let whole = usable / rs * rs;
                        carry = buf[whole..usable].to_vec();
                        buf.truncate(whole);
                        remaining = remaining.saturating_sub(take);
                        let count = (whole / rs) as u64;
                        if tx.send(Ok((next_record, buf))).is_err() {
                            break 'outer;
                        }
                        next_record += count;
                    }
                }
            });
            for msg in rx {
                let (first, mut buf) = msg?;
                let batches: Vec<BatchOut> = buf
                    .par_chunks_mut(rs * PARSE_BATCH)
                    .enumerate()
                    .map(|(i, chunk)| parse_batch(first + (i * PARSE_BATCH) as u64, chunk, rs, cluster))
                    .collect();
                let n = (buf.len() / rs) as u64;
                for b in batches {
                    content.merge(b, ctl);
                }
                ctl.records_done.fetch_add(n, Ordering::Relaxed);
            }
            Ok(())
        })?;
        ctl.check()?;
        ctl.set_phase(ScanPhase::BuildingTree);
        drop(vol);
        content.build(&root, ctl)
    }
}

#[cfg(test)]
mod tests {
    use super::parse::tests::RecordBuilder;
    use super::*;

    fn run(records: Vec<(u64, Vec<u8>)>, n: usize) -> ScanTree {
        let ctl = ScanControl::new();
        let rs = 1024;
        let mut img = vec![0u8; n * rs];
        for (i, r) in records {
            img[i as usize * rs..(i as usize + 1) * rs].copy_from_slice(&r);
        }
        let mut content = MftContent::with_records(n);
        let batches: Vec<BatchOut> = img
            .chunks_mut(rs * 4)
            .enumerate()
            .map(|(i, c)| parse_batch((i * 4) as u64, c, rs, 4096))
            .collect();
        for b in batches {
            content.merge(b, &ctl);
        }
        content.build("X:\\", &ctl).unwrap()
    }

    #[test]
    fn builds_tree_from_synthetic_mft() {
        let dir = RECORD_IN_USE | RECORD_IS_DIR;
        let recs = vec![
            (5, RecordBuilder::new(dir, 0).std_info(1, 1, 0x16).file_name(5, NS_WIN32_DOS, ".").build()),
            (30, RecordBuilder::new(dir, 0).std_info(1, 50, 0x10).file_name(5, NS_WIN32, "Users").build()),
            (
                31,
                RecordBuilder::new(RECORD_IN_USE, 0)
                    .std_info(1, 40, 0x20)
                    .file_name(30, NS_DOS, "MOVIE~1.MP4")
                    .file_name(30, NS_WIN32, "movie file.mp4")
                    .file_name(5, NS_POSIX, "hardlink.mp4")
                    .nonresident_data("", 0, 1 << 20, 1_000_000, None, &[0x11, 0x01, 0x10])
                    .build(),
            ),
            // Extension record of 32 carrying the data attribute.
            (
                33,
                RecordBuilder::new(RECORD_IN_USE, 32)
                    .nonresident_data("", 0, 8192, 8000, None, &[0x11, 0x02, 0x10])
                    .build(),
            ),
            (
                32,
                RecordBuilder::new(RECORD_IN_USE, 0)
                    .std_info(1, 60, 0x20)
                    .file_name(30, NS_WIN32, "extended.bin")
                    .build(),
            ),
            // Deleted record: not in use.
            (34, RecordBuilder::new(0, 0).file_name(30, NS_WIN32, "deleted.txt").build()),
        ];
        let t = run(recs, 40);
        let users = t.find_path("X:\\Users").unwrap();
        assert_eq!(t.children(users).len(), 2);
        let movie = t.find_path("X:\\Users\\movie file.mp4").unwrap();
        assert_eq!(t.node(movie).size, 1_000_000);
        assert_eq!(t.node(movie).links, 2);
        let link = t.find_path("X:\\hardlink.mp4").unwrap();
        // Exactly one of the two links is counted.
        let dup_count = [movie, link]
            .iter()
            .filter(|&&n| t.node(n).flags & flags::HARDLINK_DUP != 0)
            .count();
        assert_eq!(dup_count, 1);
        let ext = t.find_path("X:\\Users\\extended.bin").unwrap();
        assert_eq!(t.node(ext).size, 8000);
        assert_eq!(t.node(ext).alloc, 8192);
        assert!(t.find_path("X:\\Users\\deleted.txt").is_none());
        assert!(t.find_path("X:\\Users\\MOVIE~1.MP4").is_none(), "8.3 alias hidden");
        assert_eq!(t.meta.total_size, 1_000_000 + 8000);
        assert_eq!(t.meta.files, 3);
        assert_eq!(t.node(users).modified, 60);
    }
}
