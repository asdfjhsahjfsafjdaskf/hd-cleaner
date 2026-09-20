//! Snapshot comparison ("what filled my disk?"). Walks both trees in parallel
//! by matching child names per directory (no global path map), so memory use
//! stays proportional to the size of individual directories.

use crate::scan::model::{NodeId, ScanTree, ROOT};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Removed,
    Grew,
    Shrank,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    pub is_dir: bool,
    pub old_size: u64,
    pub new_size: u64,
    pub delta: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiffReport {
    pub old_root: String,
    pub new_root: String,
    pub old_started_ms: i64,
    pub new_started_ms: i64,
    pub old_total: u64,
    pub new_total: u64,
    pub total_delta: i64,
    pub added_files: u64,
    pub removed_files: u64,
    pub grown_files: u64,
    pub shrunk_files: u64,
    /// Largest file changes by absolute delta.
    pub files: Vec<Change>,
    /// Largest folder changes by absolute delta (net growth per folder).
    pub folders: Vec<Change>,
}

struct Walker<'a> {
    old: &'a ScanTree,
    new: &'a ScanTree,
    report: DiffReport,
    files: Vec<Change>,
    folders: Vec<Change>,
    limit: usize,
}

impl Walker<'_> {
    fn size(t: &ScanTree, id: NodeId) -> u64 {
        t.node(id).alloc
    }

    fn push(list: &mut Vec<Change>, c: Change, limit: usize) {
        list.push(c);
        // Keep memory bounded: prune to the top `limit` periodically.
        if list.len() >= limit * 4 {
            list.sort_unstable_by_key(|c| std::cmp::Reverse(c.delta.unsigned_abs()));
            list.truncate(limit);
        }
    }

    fn whole(&mut self, t: &ScanTree, id: NodeId, added: bool) {
        // Entire subtree added/removed: count files, record the top item.
        let n = t.node(id);
        let size = Self::size(t, id);
        let kind = if added { ChangeKind::Added } else { ChangeKind::Removed };
        let change = Change {
            path: t.path(id),
            kind,
            is_dir: n.is_dir(),
            old_size: if added { 0 } else { size },
            new_size: if added { size } else { 0 },
            delta: if added { size as i64 } else { -(size as i64) },
        };
        let count = if n.is_dir() { n.files as u64 } else { 1 };
        if added {
            self.report.added_files += count;
        } else {
            self.report.removed_files += count;
        }
        if n.is_dir() {
            Self::push(&mut self.folders, change, self.limit);
            // Also surface the biggest files inside a new/removed folder.
            let mut stack = vec![id];
            while let Some(d) = stack.pop() {
                for &c in t.children(d) {
                    let cn = t.node(c);
                    if cn.is_dir() {
                        stack.push(c);
                    } else if cn.alloc > 0 {
                        let s = Self::size(t, c);
                        Self::push(
                            &mut self.files,
                            Change {
                                path: t.path(c),
                                kind,
                                is_dir: false,
                                old_size: if added { 0 } else { s },
                                new_size: if added { s } else { 0 },
                                delta: if added { s as i64 } else { -(s as i64) },
                            },
                            self.limit,
                        );
                    }
                }
            }
        } else {
            Self::push(&mut self.files, change, self.limit);
        }
    }

    fn walk(&mut self, o: NodeId, n: NodeId) {
        let (old, new) = (self.old, self.new);
        let old_kids: HashMap<String, NodeId> =
            old.children(o).iter().map(|&c| (old.name(c).to_lowercase(), c)).collect();
        let mut seen: Vec<bool> = vec![false; old.children(o).len()];
        let idx_of: HashMap<NodeId, usize> = old.children(o).iter().enumerate().map(|(i, &c)| (c, i)).collect();
        for &nc in new.children(n) {
            match old_kids.get(&new.name(nc).to_lowercase()) {
                Some(&oc) => {
                    seen[idx_of[&oc]] = true;
                    let (on, nn) = (old.node(oc), new.node(nc));
                    if on.is_dir() != nn.is_dir() {
                        self.whole(old, oc, false);
                        self.whole(new, nc, true);
                        continue;
                    }
                    let (os, ns) = (Self::size(old, oc), Self::size(new, nc));
                    if nn.is_dir() {
                        if os != ns {
                            let c = Change {
                                path: new.path(nc),
                                kind: if ns > os { ChangeKind::Grew } else { ChangeKind::Shrank },
                                is_dir: true,
                                old_size: os,
                                new_size: ns,
                                delta: ns as i64 - os as i64,
                            };
                            Self::push(&mut self.folders, c, self.limit);
                        }
                        // Unchanged totals and counts: skip the subtree.
                        if os != ns || on.files != nn.files || on.modified != nn.modified {
                            self.walk(oc, nc);
                        }
                    } else if os != ns {
                        if ns > os {
                            self.report.grown_files += 1;
                        } else {
                            self.report.shrunk_files += 1;
                        }
                        let c = Change {
                            path: new.path(nc),
                            kind: if ns > os { ChangeKind::Grew } else { ChangeKind::Shrank },
                            is_dir: false,
                            old_size: os,
                            new_size: ns,
                            delta: ns as i64 - os as i64,
                        };
                        Self::push(&mut self.files, c, self.limit);
                    }
                }
                None => self.whole(new, nc, true),
            }
        }
        for (i, &oc) in old.children(o).iter().enumerate() {
            if !seen[i] {
                self.whole(old, oc, false);
            }
        }
    }
}

pub fn diff(old: &ScanTree, new: &ScanTree, limit: usize) -> DiffReport {
    let limit = limit.clamp(10, 100_000);
    let mut w = Walker { old, new, report: DiffReport::default(), files: Vec::new(), folders: Vec::new(), limit };
    w.walk(ROOT, ROOT);
    let Walker { mut report, mut files, mut folders, .. } = w;
    files.sort_unstable_by_key(|c| std::cmp::Reverse(c.delta.unsigned_abs()));
    files.truncate(limit);
    folders.sort_unstable_by_key(|c| std::cmp::Reverse(c.delta.unsigned_abs()));
    folders.truncate(limit);
    report.old_root = old.meta.root_path.clone();
    report.new_root = new.meta.root_path.clone();
    report.old_started_ms = old.meta.started_ms;
    report.new_started_ms = new.meta.started_ms;
    report.old_total = old.meta.total_alloc;
    report.new_total = new.meta.total_alloc;
    report.total_delta = new.meta.total_alloc as i64 - old.meta.total_alloc as i64;
    report.files = files;
    report.folders = folders;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::ScanMeta;

    fn f(size: u64) -> EntryInfo {
        EntryInfo { size, alloc: size, ..Default::default() }
    }
    fn d() -> EntryInfo {
        EntryInfo { is_dir: true, ..Default::default() }
    }

    #[test]
    fn detects_changes() {
        let mut a = TreeBuilder::new("C:\\", 8);
        let games = a.add(ROOT, "Games", &d());
        a.add(games, "old.pak", &f(100));
        a.add(games, "grow.pak", &f(100));
        a.add(ROOT, "gone.iso", &f(500));
        let old = a.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });

        let mut b = TreeBuilder::new("C:\\", 8);
        let games = b.add(ROOT, "games", &d());
        b.add(games, "old.pak", &f(100));
        b.add(games, "grow.pak", &f(1000));
        let newdir = b.add(ROOT, "NewApp", &d());
        b.add(newdir, "big.bin", &f(2000));
        let new = b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });

        let r = diff(&old, &new, 100);
        assert_eq!(r.total_delta, 3100 - 700);
        assert_eq!(r.added_files, 1);
        assert_eq!(r.removed_files, 1);
        assert_eq!(r.grown_files, 1);
        assert_eq!(r.files[0].path, "C:\\NewApp\\big.bin");
        assert_eq!(r.files[0].kind, ChangeKind::Added);
        assert!(r.files.iter().any(|c| c.path == "C:\\games\\grow.pak" && c.delta == 900));
        assert!(r.files.iter().any(|c| c.path == "C:\\gone.iso" && c.kind == ChangeKind::Removed));
        assert!(r.folders.iter().any(|c| c.path == "C:\\games" && c.delta == 900));
    }
}
