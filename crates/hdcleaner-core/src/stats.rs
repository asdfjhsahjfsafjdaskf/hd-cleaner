//! Aggregations over a scan: space by category, by extension, size histogram.

use crate::category::Category;
use crate::scan::model::{NodeId, ScanTree, ROOT};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub key: String,
    pub files: u64,
    pub size: u64,
    pub alloc: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    pub categories: Vec<Bucket>,
    pub extensions: Vec<Bucket>,
    /// Buckets: <1MB, 1-10MB, 10-100MB, 100MB-1GB, 1-10GB, >=10GB
    pub histogram: Vec<Bucket>,
}

const HIST: [(&str, u64); 6] = [
    ("<1MB", 1 << 20),
    ("1MB-10MB", 10 << 20),
    ("10MB-100MB", 100 << 20),
    ("100MB-1GB", 1 << 30),
    ("1GB-10GB", 10 << 30),
    (">=10GB", u64::MAX),
];

/// One linear pass over the files below `scope`.
pub fn compute(tree: &ScanTree, scope: NodeId, top_extensions: usize) -> ScanStats {
    let mut cats = vec![Bucket::default(); Category::ALL.len()];
    let mut exts = vec![Bucket::default(); tree.ext_table().len()];
    let mut hist = vec![Bucket::default(); HIST.len()];
    let mut stack = vec![scope];
    while let Some(id) = stack.pop() {
        for &c in tree.children(id) {
            let n = tree.node(c);
            if n.is_dir() {
                stack.push(c);
                continue;
            }
            if !n.counts_toward_totals() {
                continue;
            }
            let cat = tree.category(c) as usize;
            for b in [&mut cats[cat], &mut exts[n.ext as usize]] {
                b.files += 1;
                b.size += n.size;
                b.alloc += n.alloc;
            }
            let h = HIST.iter().position(|&(_, lim)| n.size < lim).unwrap_or(HIST.len() - 1);
            hist[h].files += 1;
            hist[h].size += n.size;
            hist[h].alloc += n.alloc;
        }
    }
    for (i, b) in cats.iter_mut().enumerate() {
        b.key = Category::ALL[i].name().to_string();
    }
    for (i, b) in exts.iter_mut().enumerate() {
        b.key = tree.ext_table()[i].clone();
    }
    for (i, b) in hist.iter_mut().enumerate() {
        b.key = HIST[i].0.to_string();
    }
    cats.retain(|b| b.files > 0);
    cats.sort_by(|a, b| b.alloc.cmp(&a.alloc));
    exts.retain(|b| b.files > 0);
    exts.sort_by(|a, b| b.alloc.cmp(&a.alloc));
    exts.truncate(top_extensions);
    ScanStats { categories: cats, extensions: exts, histogram: hist }
}

pub fn compute_root(tree: &ScanTree) -> ScanStats {
    compute(tree, ROOT, 50)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::builder::{EntryInfo, TreeBuilder};
    use crate::scan::model::ScanMeta;

    #[test]
    fn buckets() {
        let mut b = TreeBuilder::new("C:\\", 8);
        let d = b.add(ROOT, "v", &EntryInfo { is_dir: true, ..Default::default() });
        b.add(d, "a.mp4", &EntryInfo { size: 2 << 30, alloc: 2 << 30, ..Default::default() });
        b.add(d, "b.MP4", &EntryInfo { size: 5 << 20, alloc: 5 << 20, ..Default::default() });
        b.add(ROOT, "c.txt", &EntryInfo { size: 10, alloc: 4096, ..Default::default() });
        let t = b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });
        let s = compute_root(&t);
        assert_eq!(s.categories[0].key, "video");
        assert_eq!(s.categories[0].files, 2);
        assert_eq!(s.extensions[0].key, "mp4");
        assert_eq!(s.histogram[0].files, 1);
        assert_eq!(s.histogram[1].files, 1);
        assert_eq!(s.histogram[4].files, 1);
    }
}
