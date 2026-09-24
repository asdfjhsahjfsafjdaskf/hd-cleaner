//! Timeline: how a folder's size moved across the snapshots already on disk.
//!
//! Nothing new is scanned — every point comes from a snapshot the app saved
//! after a scan, so the answer is only as complete as the scans that were run.
//! A folder missing from a snapshot is reported as missing, never as zero.

use crate::scan::model::ScanTree;
use crate::scan::snapshot;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    /// When the scan behind this snapshot started.
    pub taken_ms: i64,
    pub size: u64,
    pub alloc: u64,
    pub files: u64,
    pub dirs: u64,
    /// The folder did not exist in this snapshot.
    pub missing: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub path: String,
    pub points: Vec<Point>,
    /// Newest minus oldest measured point (allocated bytes).
    pub delta: i64,
    /// Days between those two points.
    pub days: f64,
    /// Snapshots that could not be read (deleted, or from another version).
    pub unreadable: usize,
}

/// One snapshot to read: its file and the moment the scan started.
pub struct Source<'a> {
    pub file: &'a str,
    pub started_ms: i64,
}

/// The volume a path lives on (`C:\Users\me\x` → `C:\`), which is how scans
/// are recorded. A UNC path keeps `\\server\share`.
pub fn root_of(path: &str) -> String {
    let p = path.trim().replace('/', "\\");
    if let Some(rest) = p.strip_prefix(r"\\") {
        let mut it = rest.splitn(3, '\\');
        return match (it.next(), it.next()) {
            (Some(server), Some(share)) if !server.is_empty() && !share.is_empty() => format!(r"\\{server}\{share}"),
            _ => p,
        };
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return format!("{}:\\", (b[0] as char).to_ascii_uppercase());
    }
    p
}

fn point_of(tree: &ScanTree, path: &str, taken_ms: i64) -> Point {
    match tree.find_path(path) {
        Some(id) => {
            let n = tree.node(id);
            Point { taken_ms, size: n.size, alloc: n.alloc, files: n.files as u64, dirs: n.dirs as u64, missing: false }
        }
        None => Point { taken_ms, size: 0, alloc: 0, files: 0, dirs: 0, missing: true },
    }
}

/// Read `sources` (oldest first in the result) and follow `path` through them.
///
/// `should_stop` is polled between snapshots so a long read can be cancelled.
pub fn build(path: &str, sources: &[Source], should_stop: &dyn Fn() -> bool) -> Timeline {
    let mut points = Vec::new();
    let mut unreadable = 0;
    for s in sources {
        if should_stop() {
            break;
        }
        match snapshot::load(Path::new(s.file)) {
            Ok(tree) => points.push(point_of(&tree, path, s.started_ms)),
            Err(_) => unreadable += 1,
        }
    }
    points.sort_by_key(|p| p.taken_ms);
    let measured: Vec<&Point> = points.iter().filter(|p| !p.missing).collect();
    let (delta, days) = match (measured.first(), measured.last()) {
        (Some(first), Some(last)) if measured.len() > 1 => (
            last.alloc as i64 - first.alloc as i64,
            (last.taken_ms - first.taken_ms) as f64 / 86_400_000.0,
        ),
        _ => (0, 0.0),
    };
    Timeline { path: path.to_string(), points, delta, days, unreadable }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::{ScanMeta, ROOT};

    fn snapshot_with(dir_size: u64, file: &Path) {
        let mut b = TreeBuilder::new("C:\\", 8);
        let d = b.add(ROOT, "Games", &EntryInfo { is_dir: true, ..Default::default() });
        b.add(d, "data.bin", &EntryInfo { size: dir_size, alloc: dir_size, ..Default::default() });
        let tree = b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });
        snapshot::save(&tree, file).unwrap();
    }

    #[test]
    fn a_path_maps_to_its_volume() {
        assert_eq!(root_of(r"C:\Users\me\Games"), "C:\\");
        assert_eq!(root_of("c:/users/me"), "C:\\");
        assert_eq!(root_of(r"D:\"), "D:\\");
        assert_eq!(root_of(r"\\server\share\folder"), r"\\server\share");
    }

    #[test]
    fn follows_a_folder_across_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.hdcs");
        let b = tmp.path().join("b.hdcs");
        snapshot_with(1000, &a);
        snapshot_with(3000, &b);

        let day = 86_400_000;
        let sources = [
            Source { file: b.to_str().unwrap(), started_ms: 30 * day },
            Source { file: a.to_str().unwrap(), started_ms: 20 * day },
        ];
        let t = build(r"C:\Games", &sources, &|| false);
        assert_eq!(t.points.len(), 2);
        // Oldest first, whatever order the sources came in.
        assert_eq!(t.points[0].alloc, 1000);
        assert_eq!(t.points[1].alloc, 3000);
        assert_eq!(t.delta, 2000);
        assert!((t.days - 10.0).abs() < 0.01, "{}", t.days);
        assert_eq!(t.unreadable, 0);

        // A folder that is in no snapshot is missing, not zero.
        let gone = build(r"C:\Nowhere", &sources, &|| false);
        assert!(gone.points.iter().all(|p| p.missing));
        assert_eq!(gone.delta, 0);

        // Unreadable files are counted, never guessed.
        let bad = tmp.path().join("bad.hdcs");
        std::fs::write(&bad, b"not a snapshot").unwrap();
        let t = build(r"C:\Games", &[Source { file: bad.to_str().unwrap(), started_ms: 0 }], &|| false);
        assert_eq!(t.unreadable, 1);
        assert!(t.points.is_empty());
    }
}
