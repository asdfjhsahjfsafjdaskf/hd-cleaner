//! Duplicate file detection over a scan tree.
//!
//! Precise mode pipeline (never hashes more than necessary):
//! 1. group by exact size, drop unique sizes;
//! 2. BLAKE3 of the first + last 64 KiB, drop unique partial hashes;
//! 3. full BLAKE3 only for the remaining candidates.
//!
//! Quick mode groups by (lowercase name, size[, modified]) without reading.
//! Cloud placeholders are skipped so hashing never triggers downloads, and
//! additional hard links are skipped because they are the same file.
//! The engine never decides which copy to delete.

use crate::scan::model::{flags, NodeId, ScanTree, ROOT};
use crate::scan::ScanControl;
use crate::util::to_extended;
use crate::{AppError, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::Ordering;

const PARTIAL: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DupMode {
    Quick,
    QuickWithDate,
    Precise,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DupOptions {
    pub mode: DupMode,
    #[serde(default = "default_min")]
    pub min_size: u64,
    #[serde(default)]
    pub scope: Option<NodeId>,
}

fn default_min() -> u64 {
    1024 * 1024
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
    pub id: u32,
    pub size: u64,
    /// Hex BLAKE3 (precise mode) or empty.
    pub hash: String,
    pub files: Vec<NodeId>,
    pub wasted: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DupReport {
    pub groups: Vec<DupGroup>,
    pub total_wasted: u64,
    pub files_hashed: u64,
    pub bytes_hashed: u64,
    pub unreadable: u64,
}

fn eligible(tree: &ScanTree, id: NodeId, min: u64) -> bool {
    let n = tree.node(id);
    !n.is_dir()
        && n.size >= min.max(1)
        && n.flags & (flags::HARDLINK_DUP | flags::CLOUD | flags::REPARSE | flags::METAFILE | flags::DETACHED) == 0
}

fn hash_file(path: &str, size: u64, partial: bool, ctl: &ScanControl) -> std::io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(to_extended(path))?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    if partial {
        let head = size.min(PARTIAL) as usize;
        f.read_exact(&mut buf[..head])?;
        h.update(&buf[..head]);
        if size > 2 * PARTIAL {
            f.seek(SeekFrom::Start(size - PARTIAL))?;
            f.read_exact(&mut buf[..PARTIAL as usize])?;
            h.update(&buf[..PARTIAL as usize]);
        }
        ctl.bytes.fetch_add(head as u64 + if size > 2 * PARTIAL { PARTIAL } else { 0 }, Ordering::Relaxed);
    } else {
        loop {
            if ctl.is_cancelled() {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            ctl.bytes.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
    Ok(*h.finalize().as_bytes())
}

pub fn find_duplicates(tree: &ScanTree, opts: &DupOptions, ctl: &ScanControl) -> Result<DupReport> {
    let scope = opts.scope.unwrap_or(ROOT);
    let candidates: Vec<NodeId> = (1..tree.len() as NodeId)
        .into_par_iter()
        .filter(|&id| eligible(tree, id, opts.min_size) && (scope == ROOT || tree.is_within(id, scope)))
        .collect();

    let mut report = DupReport::default();
    let mut groups: Vec<(u64, String, Vec<NodeId>)> = match opts.mode {
        DupMode::Quick | DupMode::QuickWithDate => {
            let mut map: HashMap<(String, u64, u64), Vec<NodeId>> = HashMap::new();
            for id in candidates {
                let n = tree.node(id);
                let date = if opts.mode == DupMode::QuickWithDate { n.modified } else { 0 };
                map.entry((tree.name(id).to_lowercase(), n.size, date)).or_default().push(id);
            }
            map.into_iter().filter(|(_, v)| v.len() > 1).map(|((_, s, _), v)| (s, String::new(), v)).collect()
        }
        DupMode::Precise => {
            // 1. size
            let mut by_size: HashMap<u64, Vec<NodeId>> = HashMap::new();
            for id in candidates {
                by_size.entry(tree.node(id).size).or_default().push(id);
            }
            let same_size: Vec<(u64, Vec<NodeId>)> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
            ctl.records_total.store(same_size.iter().map(|(_, v)| v.len() as u64).sum(), Ordering::Relaxed);
            ctl.set_phase(crate::scan::ScanPhase::Enumerating);

            // 2. partial hash
            let hash_group = |size: u64, ids: &[NodeId], partial: bool| -> Vec<Vec<(NodeId, [u8; 32])>> {
                let hashed: Vec<Option<(NodeId, [u8; 32])>> = ids
                    .par_iter()
                    .map(|&id| {
                        if ctl.is_cancelled() {
                            return None;
                        }
                        let r = hash_file(&tree.path(id), size, partial, ctl).ok().map(|h| (id, h));
                        if partial {
                            ctl.records_done.fetch_add(1, Ordering::Relaxed);
                        } else {
                            ctl.files.fetch_add(1, Ordering::Relaxed);
                        }
                        if r.is_none() {
                            ctl.errors.fetch_add(1, Ordering::Relaxed);
                        }
                        r
                    })
                    .collect();
                let mut m: HashMap<[u8; 32], Vec<(NodeId, [u8; 32])>> = HashMap::new();
                for (id, h) in hashed.into_iter().flatten() {
                    m.entry(h).or_default().push((id, h));
                }
                m.into_values().filter(|v| v.len() > 1).collect()
            };

            let mut partial_groups: Vec<(u64, Vec<NodeId>)> = Vec::new();
            for (size, ids) in &same_size {
                ctl.check()?;
                for g in hash_group(*size, ids, true) {
                    partial_groups.push((*size, g.into_iter().map(|(id, _)| id).collect()));
                }
            }
            // 3. full hash (files up to 2*PARTIAL were fully covered already)
            ctl.set_phase(crate::scan::ScanPhase::BuildingTree);
            ctl.records_total.store(partial_groups.iter().map(|(_, v)| v.len() as u64).sum(), Ordering::Relaxed);
            let mut out = Vec::new();
            for (size, ids) in partial_groups {
                ctl.check()?;
                if size <= 2 * PARTIAL {
                    out.push((size, String::new(), ids));
                    continue;
                }
                for g in hash_group(size, &ids, false) {
                    let hash = hex(&g[0].1);
                    out.push((size, hash, g.into_iter().map(|(id, _)| id).collect()));
                }
            }
            out
        }
    };
    ctl.check().map_err(|_| AppError::Cancelled)?;

    for g in groups.iter_mut() {
        g.2.sort_by(|&a, &b| tree.node(a).modified.cmp(&tree.node(b).modified).then(a.cmp(&b)));
    }
    groups.sort_by(|a, b| (b.0 * (b.2.len() as u64 - 1)).cmp(&(a.0 * (a.2.len() as u64 - 1))));
    for (i, (size, hash, files)) in groups.into_iter().enumerate() {
        let wasted = size * (files.len() as u64 - 1);
        report.total_wasted += wasted;
        report.groups.push(DupGroup { id: i as u32, size, hash, files, wasted });
    }
    report.files_hashed = ctl.files.load(Ordering::Relaxed);
    report.bytes_hashed = ctl.bytes.load(Ordering::Relaxed);
    report.unreadable = ctl.errors.load(Ordering::Relaxed);
    Ok(report)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Selection helpers for the UI ("select all but the oldest", ...). They only
/// return suggestions; deletion still goes through plan + confirmation.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoSelect {
    KeepOldest,
    KeepNewest,
    KeepShortestPath,
    KeepInFolder,
}

pub fn auto_select(tree: &ScanTree, group: &DupGroup, rule: AutoSelect, folder: Option<&str>) -> Vec<NodeId> {
    let keep = match rule {
        AutoSelect::KeepOldest => group.files.iter().copied().min_by_key(|&id| (tree.node(id).modified, id)),
        AutoSelect::KeepNewest => group.files.iter().copied().max_by_key(|&id| (tree.node(id).modified, id)),
        AutoSelect::KeepShortestPath => group.files.iter().copied().min_by_key(|&id| (tree.path(id).len(), id)),
        AutoSelect::KeepInFolder => {
            let f = folder.unwrap_or("").to_lowercase();
            group.files.iter().copied().find(|&id| !f.is_empty() && tree.path(id).to_lowercase().starts_with(&f))
        }
    };
    match keep {
        Some(k) => group.files.iter().copied().filter(|&id| id != k).collect(),
        None => Vec::new(), // no file matched the rule: select nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::standard::WindowsApiScanner;
    use crate::scan::{FileSystemScanner, ScanOptions};

    #[test]
    fn precise_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let big: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
        let mut big_diff_middle = big.clone();
        big_diff_middle[200_000] ^= 1; // same size, same head/tail, different middle
        std::fs::write(dir.path().join("a.bin"), &big).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("a copy.bin"), &big).unwrap();
        std::fs::write(dir.path().join("c.bin"), &big_diff_middle).unwrap();
        std::fs::write(dir.path().join("small1.txt"), b"same small content").unwrap();
        std::fs::write(dir.path().join("small2.txt"), b"same small content").unwrap();
        std::fs::write(dir.path().join("unique.txt"), b"unique content....").unwrap();
        // hard link must not be reported as a duplicate
        std::fs::hard_link(dir.path().join("a.bin"), dir.path().join("a-link.bin")).unwrap();

        let ctl = ScanControl::new();
        let tree = WindowsApiScanner.scan(dir.path().to_str().unwrap(), &ScanOptions::default(), &ctl).unwrap();
        let ctl = ScanControl::new();
        let r = find_duplicates(&tree, &DupOptions { mode: DupMode::Precise, min_size: 1, scope: None }, &ctl).unwrap();
        assert_eq!(r.groups.len(), 2, "{:?}", r.groups);
        let big_group = &r.groups[0];
        assert_eq!(big_group.size, 400_000);
        assert_eq!(big_group.files.len(), 2);
        assert_eq!(big_group.wasted, 400_000);
        assert_eq!(big_group.hash.len(), 64);
        let names: Vec<&str> = big_group.files.iter().map(|&i| tree.name(i)).collect();
        assert!(!names.contains(&"c.bin"));
        assert_eq!(r.groups[1].files.len(), 2);

        let sel = auto_select(&tree, big_group, AutoSelect::KeepShortestPath, None);
        assert_eq!(sel.len(), 1);
        let none = auto_select(&tree, big_group, AutoSelect::KeepInFolder, Some("Z:\\nowhere"));
        assert!(none.is_empty());
    }

    #[test]
    fn quick_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("x")).unwrap();
        std::fs::write(dir.path().join("Photo.jpg"), b"12345").unwrap();
        std::fs::write(dir.path().join("x").join("photo.JPG"), b"abcde").unwrap();
        let ctl = ScanControl::new();
        let tree = WindowsApiScanner.scan(dir.path().to_str().unwrap(), &ScanOptions::default(), &ctl).unwrap();
        let r = find_duplicates(&tree, &DupOptions { mode: DupMode::Quick, min_size: 1, scope: None }, &ScanControl::new())
            .unwrap();
        assert_eq!(r.groups.len(), 1, "quick mode matches by name+size only");
    }
}
