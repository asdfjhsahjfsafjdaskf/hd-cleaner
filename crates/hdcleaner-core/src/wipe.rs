//! Overwriting the free space of a volume.
//!
//! Deleting a file removes its entry, not its bytes: what was there stays on
//! the disk until something else lands on top. Filling the free space is how
//! that leftover data gets covered.
//!
//! Two honest limits, both shown to the user before anything runs:
//!
//! * On an SSD (or any flash) this is close to pointless — TRIM and wear
//!   levelling mean the blocks the app writes are usually not the blocks that
//!   held the old data. Full-disk encryption is the real answer there.
//! * Parts of a file can live outside the data area (very small files inside
//!   the MFT, alternate streams, shadow copies). This never touches those.
//!
//! The volume is never filled completely: a reserve is always left free, and
//! the filler file is deleted even when the run is cancelled or fails.

use crate::{AppError, Result};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Never leave the volume with less than this free.
pub const DEFAULT_RESERVE: u64 = 1 << 30; // 1 GiB
const CHUNK: usize = 8 << 20; // 8 MiB per write

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WipePlan {
    /// Free space right now.
    pub free: u64,
    /// What would be written (free minus the reserve, capped by `max_bytes`).
    pub to_write: u64,
    pub reserve: u64,
    /// The volume is flash: say so, loudly.
    pub is_ssd: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeReport {
    pub written: u64,
    pub stopped: bool,
    /// The run ended because the volume filled up to the reserve.
    pub reached_reserve: bool,
    pub duration_ms: u64,
}

/// The drive `root` names, by letter or by root path (`C:` or `C:\`).
fn find_drive(root: &str) -> Result<crate::disk::DriveInfo> {
    let letter = root.trim().chars().next().map(|c| c.to_ascii_uppercase());
    crate::disk::list_drives()
        .into_iter()
        .find(|d| Some(d.letter.to_ascii_uppercase()) == letter)
        .ok_or_else(|| AppError::NotFound { path: root.to_string() })
}

fn free_space(root: &str) -> Result<(u64, u64)> {
    let d = find_drive(root)?;
    Ok((d.free_bytes, d.total_bytes))
}

/// What a wipe would do right now, without doing anything.
pub fn plan(root: &str, max_bytes: Option<u64>, reserve: u64) -> Result<WipePlan> {
    let drive = find_drive(root)?;
    if drive.kind != crate::disk::DriveKind::Fixed {
        return Err(AppError::NotSupported("only fixed drives can have their free space wiped".into()));
    }
    let free = drive.free_bytes;
    let mut to_write = free.saturating_sub(reserve);
    if let Some(max) = max_bytes {
        to_write = to_write.min(max);
    }
    Ok(WipePlan { free, to_write, reserve, is_ssd: drive.media == crate::disk::MediaKind::Ssd })
}

/// The folder the filler file goes in: the root of the volume, or a folder
/// given by the caller (used by the tests).
///
/// A standard user cannot write to the drive root, so the filler file goes to
/// the temp folder (or the profile) when either lives on the same volume —
/// which covers the same free space just as well.
fn writable_folder(root: &str) -> Result<PathBuf> {
    let letter = root.trim().chars().next().map(|c| c.to_ascii_uppercase());
    let on_volume = |p: &Path| p.to_string_lossy().trim().chars().next().map(|c| c.to_ascii_uppercase()) == letter;
    let candidates = [
        std::env::temp_dir(),
        std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default(),
        std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_default(),
        PathBuf::from(format!("{}\\", root.trim_end_matches('\\'))),
    ];
    for dir in candidates.into_iter().filter(|d| !d.as_os_str().is_empty() && on_volume(d) && d.is_dir()) {
        // Only a folder that really accepts a file counts.
        let probe = dir.join(unique_name("hdcleaner-wipe-probe"));
        if std::fs::File::create(&probe).is_ok() {
            let _ = std::fs::remove_file(&probe);
            return Ok(dir);
        }
    }
    Err(AppError::AccessDenied { path: root.to_string() })
}

/// A name no other run can pick at the same moment (two apps, two threads).
fn unique_name(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}-{}.tmp",
        std::process::id(),
        crate::util::now_unix_ms(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn filler_path(root: &str) -> Result<PathBuf> {
    Ok(writable_folder(root)?.join(unique_name("hdcleaner-wipe")))
}

/// Fill the free space of `root`, then remove what was written.
///
/// `on_progress(written, target)` is called as it goes, and `should_stop`
/// is polled between chunks; either way the filler file is deleted.
pub fn run(
    root: &str,
    max_bytes: Option<u64>,
    reserve: u64,
    mut on_progress: impl FnMut(u64, u64),
    should_stop: &dyn Fn() -> bool,
) -> Result<WipeReport> {
    let plan = plan(root, max_bytes, reserve)?;
    let started = std::time::Instant::now();
    if plan.to_write == 0 {
        return Ok(WipeReport { written: 0, stopped: false, reached_reserve: true, duration_ms: 0 });
    }
    let file_path = filler_path(root)?;
    let mut file = std::fs::File::create(&file_path).map_err(|e| AppError::io("creating the filler file", Some(&file_path), e))?;

    // A fixed pattern is enough: the point is to put *something* on top of the
    // old bytes, and a constant one keeps the write at disk speed.
    let buf = vec![0u8; CHUNK];
    let mut written = 0u64;
    let mut stopped = false;
    let mut reached_reserve = false;
    let mut result = Ok(());

    while written < plan.to_write {
        if should_stop() {
            stopped = true;
            break;
        }
        // Recheck the free space: another program may be writing too.
        if let Ok((free, _)) = free_space(root) {
            if free <= reserve {
                reached_reserve = true;
                break;
            }
        }
        let n = ((plan.to_write - written) as usize).min(buf.len());
        match file.write_all(&buf[..n]) {
            Ok(()) => written += n as u64,
            Err(e) if e.raw_os_error() == Some(112) => {
                // ERROR_DISK_FULL: stop here, the reserve check is the guard.
                reached_reserve = true;
                break;
            }
            Err(e) => {
                result = Err(AppError::io("writing over the free space", Some(&file_path), e));
                break;
            }
        }
        on_progress(written, plan.to_write);
    }

    let flushed = file.flush().and_then(|_| file.sync_all());
    drop(file);
    // The filler file always goes, whatever happened above.
    let removed = std::fs::remove_file(&file_path);
    result?;
    flushed.map_err(|e| AppError::io("flushing", Some(&file_path), e))?;
    removed.map_err(|e| AppError::io("removing the filler file", Some(&file_path), e))?;
    Ok(WipeReport { written, stopped, reached_reserve, duration_ms: started.elapsed().as_millis() as u64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_leaves_the_reserve_alone() {
        let root = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let p = plan(&root, Some(4 << 20), DEFAULT_RESERVE).expect("the system drive is fixed");
        assert!(p.to_write <= 4 << 20, "the cap is respected");
        assert!(p.free > 0);
        assert_eq!(p.reserve, DEFAULT_RESERVE);
    }

    fn fillers_left(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("hdcleaner-wipe-"))
            .count()
    }

    #[test]
    fn writes_what_it_promised_stops_when_asked_and_never_leaves_a_filler() {
        // One test, run in order: two of these in parallel would see each
        // other's filler file in the same folder.
        let root = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let dir = writable_folder(&root).expect("some folder on this volume is writable");

        // A few MiB only: enough to prove it writes, small enough to be polite.
        let report = run(&root, Some(8 << 20), DEFAULT_RESERVE, |_, _| {}, &|| false).unwrap();
        assert!(report.written > 0 && report.written <= 8 << 20, "{report:?}");
        assert!(!report.stopped);
        assert_eq!(fillers_left(&dir), 0, "the filler file is gone");

        // Asked to stop before the first chunk: nothing written, nothing left.
        let report = run(&root, Some(64 << 20), DEFAULT_RESERVE, |_, _| {}, &|| true).unwrap();
        assert!(report.stopped);
        assert_eq!(report.written, 0);
        assert_eq!(fillers_left(&dir), 0);
    }
}
