//! Backups: what the app saved before removing something, and how to put it
//! back.
//!
//! Every destructive operation writes one folder under `<data>\backups`
//! holding a `backup.json` manifest plus the saved material: a `.reg` export
//! for registry items, and — for small files and settings — a copy of the file
//! itself (the quarantine). Restoring reads that manifest; it never overwrites
//! something that exists again, and registry values go back through
//! [`crate::regops::import`], which only writes what the app is allowed to
//! delete.
//!
//! Folders written by older versions (no manifest) are still listed, with what
//! can be read from them, so nothing that was saved becomes invisible.

use crate::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::os::windows::fs::MetadataExt;

pub const MANIFEST: &str = "backup.json";
/// Files up to this size are copied into the backup before removal.
pub const QUARANTINE_FILE_LIMIT: u64 = 16 * 1024 * 1024;
/// …and a single operation never quarantines more than this in total.
pub const QUARANTINE_TOTAL_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Folder,
    Registry,
    Task,
}

/// One thing that was backed up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub kind: EntryKind,
    /// Where it came from (a path, or the registry target as displayed).
    pub path: String,
    /// File inside the backup folder holding the saved copy, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<String>,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    /// "uninstall" | "forced" | "cleanup" | "startup"
    pub kind: String,
    /// Program or category the backup belongs to.
    pub label: String,
    pub created_ms: i64,
    pub entries: Vec<Entry>,
}

impl Manifest {
    pub fn new(kind: &str, label: &str) -> Self {
        Manifest { version: 2, kind: kind.into(), label: label.into(), created_ms: crate::util::now_unix_ms(), entries: Vec::new() }
    }

    /// Write (or rewrite) the manifest of a backup folder.
    pub fn save(&self, dir: &Path) -> Result<()> {
        crate::util::ensure_dir(dir)?;
        let file = dir.join(MANIFEST);
        let json = serde_json::to_vec_pretty(self).map_err(|e| AppError::Corrupt(e.to_string()))?;
        std::fs::write(&file, json).map_err(|e| AppError::io("writing the backup manifest", Some(&file), e))
    }
}

/// A backup as the Backups page lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSet {
    /// Folder name, which identifies the backup.
    pub id: String,
    pub path: String,
    pub created_ms: i64,
    pub kind: String,
    pub label: String,
    pub entries: usize,
    /// What the backup itself takes on disk.
    pub bytes: u64,
    /// Files and registry values that can be put back from here.
    pub restorable: usize,
    /// No manifest: written by an older version, or damaged.
    pub legacy: bool,
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

fn is_reparse(path: &Path) -> std::io::Result<bool> {
    Ok(std::fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

fn valid_stored(stored: &str) -> bool {
    !stored.is_empty() && Path::new(stored).components().all(|c| matches!(c, Component::Normal(_)))
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for e in rd.flatten() {
        match e.path().symlink_metadata() {
            Ok(m) if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {},
            Ok(m) if m.is_dir() => total += dir_size(&e.path()),
            Ok(m) => total += m.len(),
            Err(_) => {}
        }
    }
    total
}

fn restorable(dir: &Path, entries: &[Entry]) -> usize {
    entries
        .iter()
        .filter(|e| match e.kind {
            EntryKind::Registry => dir.join(e.stored.as_deref().unwrap_or("registry.reg")).exists(),
            EntryKind::File | EntryKind::Folder => e.stored.as_ref().is_some_and(|s| dir.join(s).exists()),
            EntryKind::Task => false,
        })
        .count()
}

/// Read the manifest of one backup folder, or rebuild what can be told from
/// the folder itself (older versions wrote no manifest).
pub fn read(dir: &Path) -> Result<Manifest> {
    let file = dir.join(MANIFEST);
    if let Ok(raw) = std::fs::read(&file) {
        if let Ok(m) = serde_json::from_slice::<Manifest>(&raw) {
            if m.entries.iter().any(|e| e.stored.as_deref().is_some_and(|s| !valid_stored(s))) {
                return Err(AppError::Corrupt("backup contains an invalid stored path".into()));
            }
            return Ok(m);
        }
    }
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (created_ms, label) = match name.split_once('-') {
        Some((ms, rest)) => (ms.parse().unwrap_or(0), rest.replace('_', " ")),
        None => (0, name.clone()),
    };
    let kind = if label.contains("forced") {
        "forced"
    } else if label.contains("cleanup") {
        "cleanup"
    } else if label.contains("startup") {
        "startup"
    } else {
        "uninstall"
    };
    let mut m = Manifest { version: 1, kind: kind.into(), label, created_ms, entries: Vec::new() };
    // Older uninstalls wrote what they removed into `manifest.json`. The files
    // themselves were not copied back then, so they are listed (it is the
    // record of the operation) but carry no saved copy to put back.
    if let Ok(raw) = std::fs::read(dir.join("manifest.json")) {
        if let Ok(old) = serde_json::from_slice::<serde_json::Value>(&raw) {
            for it in old.get("items").and_then(|i| i.as_array()).into_iter().flatten() {
                let Some(path) = it.get("path").and_then(|p| p.as_str()) else { continue };
                let kind = match it.get("kind").and_then(|k| k.as_str()) {
                    Some("folder") => EntryKind::Folder,
                    Some("registryKey") | Some("registryValue") => EntryKind::Registry,
                    Some("task") => EntryKind::Task,
                    _ => EntryKind::File,
                };
                let stored = (kind == EntryKind::Registry && dir.join("registry.reg").exists()).then(|| "registry.reg".to_string());
                m.entries.push(Entry { kind, path: path.to_string(), stored, size: it.get("size").and_then(|s| s.as_u64()).unwrap_or(0) });
            }
        }
    }
    // What is in the folder is what can be restored.
    for e in std::fs::read_dir(dir).map_err(|e| AppError::io("reading the backup", Some(dir), e))?.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n == MANIFEST || n == "manifest.json" {
            continue;
        }
        // Already covered by the registry items read above.
        if n == "registry.reg" && m.entries.iter().any(|x| x.stored.as_deref() == Some("registry.reg")) {
            continue;
        }
        let size = e.metadata().map(|md| md.len()).unwrap_or(0);
        if n.to_lowercase().ends_with(".reg") {
            m.entries.push(Entry { kind: EntryKind::Registry, path: n.clone(), stored: Some(n), size });
        } else if e.path().is_file() {
            // The original location is unknown in these old folders: the file
            // is listed, and restoring it asks where to put it.
            m.entries.push(Entry { kind: EntryKind::File, path: n.clone(), stored: Some(n), size });
        }
    }
    Ok(m)
}

/// Every backup under `root`, newest first.
pub fn list(root: &Path) -> Vec<BackupSet> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else { return out };
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() || is_reparse(&dir).unwrap_or(true) {
            continue;
        }
        let Ok(m) = read(&dir) else { continue };
        let id = e.file_name().to_string_lossy().into_owned();
        let created_ms = if m.created_ms > 0 {
            m.created_ms
        } else {
            e.metadata().ok().and_then(|md| md.modified().ok()).map(crate::util::system_time_to_unix_ms).unwrap_or(0)
        };
        out.push(BackupSet {
            id,
            path: dir.to_string_lossy().into_owned(),
            created_ms,
            kind: m.kind.clone(),
            label: m.label.clone(),
            entries: m.entries.len(),
            bytes: dir_size(&dir),
            restorable: restorable(&dir, &m.entries),
            legacy: m.version < 2,
        });
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.created_ms));
    out
}

/// Resolve a backup id inside `root`. Anything that is not a direct child of
/// the backups folder is refused, so an id can never point elsewhere.
pub fn dir_of(root: &Path, id: &str) -> Result<PathBuf> {
    if id.is_empty() || id.contains('\\') || id.contains('/') || id.contains("..") || id.contains(':') {
        return Err(AppError::InvalidInput("invalid backup id".into()));
    }
    let dir = root.join(id);
    if !dir.is_dir() || is_reparse(&dir).unwrap_or(true) {
        return Err(AppError::NotFound { path: dir.to_string_lossy().into_owned() });
    }
    Ok(dir)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub index: usize,
    pub path: String,
    /// "restored" | "exists" | "unknownOrigin" | "missing" | "failed" | "partial"
    pub status: &'static str,
    /// Registry values written back, for registry entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::ErrorPayload>,
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if is_reparse(from)? {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "backup contains a link or junction"));
    }
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Put back the chosen entries (by index in the manifest). Nothing is
/// overwritten: an entry whose original path exists again is reported as
/// `exists` and left alone.
pub fn restore(dir: &Path, indexes: &[usize]) -> Result<Vec<RestoreResult>> {
    let m = read(dir)?;
    let mut out = Vec::new();
    for &i in indexes {
        let Some(entry) = m.entries.get(i) else {
            out.push(RestoreResult { index: i, path: String::new(), status: "missing", values: None, error: None });
            continue;
        };
        let saved = entry.stored.as_ref().map(|s| dir.join(s));
        let result = match entry.kind {
            EntryKind::Registry => {
                let file = saved.unwrap_or_else(|| dir.join("registry.reg"));
                if !file.exists() {
                    RestoreResult { index: i, path: entry.path.clone(), status: "missing", values: None, error: None }
                } else {
                    match crate::regops::import(&file) {
                        Ok(r) => RestoreResult {
                            index: i,
                            path: entry.path.clone(),
                            status: if r.skipped.is_empty() { "restored" } else { "partial" },
                            values: Some(r.values),
                            error: None,
                        },
                        Err(e) => RestoreResult { index: i, path: entry.path.clone(), status: "failed", values: None, error: Some(e.to_payload()) },
                    }
                }
            }
            EntryKind::File | EntryKind::Folder => {
                let target = Path::new(&entry.path);
                match saved {
                    _ if !target.is_absolute() => {
                        RestoreResult { index: i, path: entry.path.clone(), status: "unknownOrigin", values: None, error: None }
                    }
                    Some(file) if !file.exists() => {
                        RestoreResult { index: i, path: entry.path.clone(), status: "missing", values: None, error: None }
                    }
                    Some(file) => {
                        if target.symlink_metadata().is_ok() {
                            RestoreResult { index: i, path: entry.path.clone(), status: "exists", values: None, error: None }
                        } else {
                            match copy_tree(&file, target) {
                                Ok(()) => RestoreResult { index: i, path: entry.path.clone(), status: "restored", values: None, error: None },
                                Err(e) => RestoreResult {
                                    index: i,
                                    path: entry.path.clone(),
                                    status: "failed",
                                    values: None,
                                    error: Some(AppError::io("restoring", Some(target), e).to_payload()),
                                },
                            }
                        }
                    }
                    None => RestoreResult { index: i, path: entry.path.clone(), status: "missing", values: None, error: None },
                }
            }
            // Scheduled tasks are saved as XML for the user to re-import.
            EntryKind::Task => RestoreResult { index: i, path: entry.path.clone(), status: "unknownOrigin", values: None, error: None },
        };
        out.push(result);
    }
    Ok(out)
}

/// Delete one backup folder (only ever a direct child of `root`).
pub fn delete(root: &Path, id: &str) -> Result<()> {
    let dir = dir_of(root, id)?;
    std::fs::remove_dir_all(&dir).map_err(|e| AppError::io("deleting the backup", Some(&dir), e))
}

/// Copy a file or a small folder into `dir` before it is removed. Returns the
/// name it was stored under, or `None` when it is too big or cannot be read.
pub fn quarantine(source: &Path, dir: &Path, index: usize, budget: &mut u64) -> Option<(String, u64)> {
    let meta = std::fs::symlink_metadata(source).ok()?;
    // A junction or symlink holds nothing of its own to save.
    if meta.file_type().is_symlink() {
        return None;
    }
    let size = if meta.is_dir() { dir_size(source) } else { meta.len() };
    if size > QUARANTINE_FILE_LIMIT || size > *budget {
        return None;
    }
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "item".into());
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).take(60).collect();
    let stored = format!("files\\{index:03}-{}", safe.trim());
    let target = dir.join(&stored);
    match copy_tree(source, &target) {
        Ok(()) => {
            *budget -= size;
            Some((stored, size))
        }
        Err(_) => {
            let _ = std::fs::remove_dir_all(&target);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_manifest_that_points_outside_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::new("test", "test");
        manifest.entries.push(Entry {
            kind: EntryKind::File,
            path: tmp.path().join("restored.txt").to_string_lossy().into_owned(),
            stored: Some(r"..\outside.txt".into()),
            size: 1,
        });
        manifest.save(tmp.path()).unwrap();
        assert!(read(tmp.path()).is_err());
        assert!(restore(tmp.path(), &[0]).is_err());
        assert!(!tmp.path().join("restored.txt").exists());
    }

    #[test]
    fn quarantines_and_restores_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let file = work.join("settings.json");
        std::fs::write(&file, br#"{"theme":"dark"}"#).unwrap();

        let dir = tmp.path().join("backups").join("1-test");
        let mut budget = QUARANTINE_TOTAL_LIMIT;
        let (stored, size) = quarantine(&file, &dir, 0, &mut budget).expect("saved");
        assert_eq!(size, 16);
        assert!(dir.join(&stored).exists());

        let mut m = Manifest::new("uninstall", "Test");
        m.entries.push(Entry { kind: EntryKind::File, path: file.to_string_lossy().into_owned(), stored: Some(stored), size });
        m.save(&dir).unwrap();
        std::fs::remove_file(&file).unwrap();

        let sets = list(&tmp.path().join("backups"));
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].restorable, 1);
        assert!(!sets[0].legacy);

        let r = restore(&dir, &[0]).unwrap();
        assert_eq!(r[0].status, "restored");
        assert_eq!(std::fs::read(&file).unwrap(), br#"{"theme":"dark"}"#);

        // A second restore must not overwrite what is there now.
        std::fs::write(&file, b"changed by the user").unwrap();
        let r = restore(&dir, &[0]).unwrap();
        assert_eq!(r[0].status, "exists");
        assert_eq!(std::fs::read(&file).unwrap(), b"changed by the user");

        delete(&tmp.path().join("backups"), "1-test").unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn reads_a_folder_written_before_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("backups");
        let dir = root.join("1700000000000-Old_Program");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("registry.reg"), b"\xff\xfe").unwrap();
        let sets = list(&root);
        assert_eq!(sets.len(), 1);
        assert!(sets[0].legacy);
        assert_eq!(sets[0].label, "Old Program");
        assert_eq!(sets[0].created_ms, 1700000000000);
        assert_eq!(sets[0].restorable, 1);
    }

    #[test]
    fn an_id_cannot_point_outside_the_backups_folder() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in ["..", r"..\other", "C:\\Windows", "a/b", ""] {
            assert!(dir_of(tmp.path(), bad).is_err(), "{bad}");
        }
    }
}
