//! Disk scanning. The UI and the rest of the program only see
//! [`FileSystemScanner`] and [`ScanTree`]; the method used is an implementation
//! detail chosen by [`run_scan`].

pub mod builder;
pub mod mft;
pub mod model;
pub mod snapshot;
pub mod standard;

use crate::{AppError, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

pub use model::{NodeId, ScanMeta, ScanTree, ROOT};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ScanMethod {
    /// NTFS MFT when possible, otherwise standard.
    #[default]
    Auto,
    Standard,
    NtfsMft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOptions {
    #[serde(default)]
    pub method: ScanMethod,
    /// Descend into junctions / directory symlinks (off by default: following
    /// them double counts space and can loop).
    #[serde(default)]
    pub follow_junctions: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { method: ScanMethod::Auto, follow_junctions: false }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[repr(u8)]
pub enum ScanPhase {
    Starting = 0,
    ReadingMft = 1,
    Enumerating = 2,
    BuildingTree = 3,
    Done = 4,
}

/// Shared progress + cancellation for a running scan. Cheap to poll from
/// another thread.
#[derive(Default)]
pub struct ScanControl {
    cancel: AtomicBool,
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    /// For MFT scans: records processed / total.
    pub records_done: AtomicU64,
    pub records_total: AtomicU64,
    phase: AtomicU8,
    current: Mutex<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub records_done: u64,
    pub records_total: u64,
    pub current: String,
}

impl ScanControl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    #[inline]
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn set_phase(&self, p: ScanPhase) {
        self.phase.store(p as u8, Ordering::Relaxed);
    }
    pub fn set_current(&self, s: &str) {
        if let Some(mut g) = self.current.try_lock() {
            g.clear();
            g.push_str(s);
        }
    }
    pub fn snapshot(&self) -> ScanProgress {
        let phase = match self.phase.load(Ordering::Relaxed) {
            1 => ScanPhase::ReadingMft,
            2 => ScanPhase::Enumerating,
            3 => ScanPhase::BuildingTree,
            4 => ScanPhase::Done,
            _ => ScanPhase::Starting,
        };
        ScanProgress {
            phase,
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            records_done: self.records_done.load(Ordering::Relaxed),
            records_total: self.records_total.load(Ordering::Relaxed),
            current: self.current.lock().clone(),
        }
    }
}

pub trait FileSystemScanner: Send + Sync {
    fn name(&self) -> &'static str;
    fn scan(&self, root: &str, opts: &ScanOptions, ctl: &ScanControl) -> Result<ScanTree>;
}

/// Decide which scanner applies to `root` and run it.
///
/// * `NtfsMft` requested explicitly: errors are returned (e.g. access denied
///   so the caller can offer elevation).
/// * `Auto`: MFT for NTFS volume roots when the volume can be opened, else the
///   standard scanner; the fallback reason is recorded in `meta.notes`.
pub fn run_scan(root: &str, opts: &ScanOptions, ctl: &ScanControl) -> Result<ScanTree> {
    let root = crate::util::normalize_root(root)?;
    let started = crate::util::now_unix_ms();
    let t0 = std::time::Instant::now();
    ctl.set_phase(ScanPhase::Starting);

    let is_volume_root = root.len() == 3 && root.ends_with(":\\");
    let (fs, _) = crate::disk::volume_fs_for_path(&root);
    let ntfs = fs.eq_ignore_ascii_case("NTFS");

    let mut notes = Vec::new();
    let mut tree = match opts.method {
        ScanMethod::NtfsMft => {
            if !is_volume_root || !ntfs {
                return Err(AppError::NotSupported(
                    "the MFT fast scan requires the root of an NTFS volume".into(),
                ));
            }
            mft::NtfsFastScanner.scan(&root, opts, ctl)?
        }
        ScanMethod::Standard => standard::WindowsApiScanner.scan(&root, opts, ctl)?,
        ScanMethod::Auto => {
            if is_volume_root && ntfs {
                match mft::NtfsFastScanner.scan(&root, opts, ctl) {
                    Ok(t) => t,
                    Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                    Err(e) => {
                        tracing::info!("MFT scan unavailable for {root}: {e}; using standard scan");
                        notes.push(format!("fastScanUnavailable:{}", e.kind()));
                        standard::WindowsApiScanner.scan(&root, opts, ctl)?
                    }
                }
            } else {
                standard::WindowsApiScanner.scan(&root, opts, ctl)?
            }
        }
    };
    tree.meta.started_ms = started;
    tree.meta.duration_ms = t0.elapsed().as_millis() as u64;
    tree.meta.file_system = fs;
    tree.meta.notes.extend(notes);
    let vol = crate::disk::volume_root_of(&root);
    let vol_w = crate::util::wide(&vol);
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            vol_w.as_ptr(),
            &mut avail,
            &mut total,
            &mut free,
        );
    }
    tree.meta.volume_total = total;
    tree.meta.volume_free = free;
    ctl.set_phase(ScanPhase::Done);
    tracing::info!(
        root = %tree.meta.root_path,
        method = %tree.meta.method,
        files = tree.meta.files,
        dirs = tree.meta.dirs,
        ms = tree.meta.duration_ms,
        mem_mb = tree.memory_bytes() / (1024 * 1024),
        "scan finished"
    );
    Ok(tree)
}
