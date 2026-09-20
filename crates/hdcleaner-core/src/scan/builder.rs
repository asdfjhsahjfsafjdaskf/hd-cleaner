//! Incremental construction of a [`ScanTree`]. Scanners append nodes (parent
//! always before child); `finish` aggregates sizes bottom-up and builds the
//! sorted CSR child index.

use super::model::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default)]
pub struct EntryInfo {
    pub is_dir: bool,
    pub size: u64,
    pub alloc: u64,
    pub created: u64,
    pub modified: u64,
    pub accessed: u64,
    pub attributes: u32,
    pub flags: u16,
    pub links: u16,
}

pub struct TreeBuilder {
    nodes: Vec<Node>,
    names: Vec<u8>,
    exts: Vec<String>,
    ext_map: HashMap<String, u16>,
}

impl TreeBuilder {
    /// Creates the builder with the root directory as node 0.
    pub fn new(root_name: &str, capacity: usize) -> Self {
        let mut b = TreeBuilder {
            nodes: Vec::with_capacity(capacity.max(16)),
            names: Vec::with_capacity(capacity.max(16) * 16),
            exts: vec![String::new()],
            ext_map: HashMap::new(),
        };
        b.push_node(NO_NODE, root_name, &EntryInfo { is_dir: true, ..Default::default() });
        b
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn root_mut(&mut self) -> &mut Node {
        &mut self.nodes[0]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id as usize]
    }

    pub fn add(&mut self, parent: NodeId, name: &str, info: &EntryInfo) -> NodeId {
        debug_assert!((parent as usize) < self.nodes.len());
        self.push_node(parent, name, info)
    }

    fn intern_ext(&mut self, name: &str) -> u16 {
        let Some(dot) = name.rfind('.') else { return 0 };
        let ext = &name[dot + 1..];
        if ext.is_empty() || ext.len() > 16 {
            return 0;
        }
        let lower = ext.to_ascii_lowercase();
        if let Some(&id) = self.ext_map.get(&lower) {
            return id;
        }
        if self.exts.len() >= u16::MAX as usize {
            return 0;
        }
        let id = self.exts.len() as u16;
        self.exts.push(lower.clone());
        self.ext_map.insert(lower, id);
        id
    }

    fn push_node(&mut self, parent: NodeId, name: &str, info: &EntryInfo) -> NodeId {
        let id = self.nodes.len() as NodeId;
        // Names longer than u16::MAX bytes cannot exist on Windows (max 255
        // UTF-16 units per component); clamp defensively on a char boundary.
        let mut name = name;
        if name.len() > u16::MAX as usize {
            let mut end = u16::MAX as usize;
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            name = &name[..end];
        }
        let name_off = self.names.len() as u32;
        self.names.extend_from_slice(name.as_bytes());
        let ext = if info.is_dir { 0 } else { self.intern_ext(name) };
        let mut flags = info.flags;
        if info.is_dir {
            flags |= flags::DIR;
        }
        self.nodes.push(Node {
            parent,
            name_off,
            name_len: name.len() as u16,
            size: info.size,
            alloc: info.alloc,
            created: info.created,
            modified: info.modified,
            accessed: info.accessed,
            attributes: info.attributes,
            ext,
            flags,
            links: info.links,
            ..Default::default()
        });
        id
    }

    /// Aggregate totals and build the child index. `meta` totals are filled in.
    pub fn finish(mut self, mut meta: ScanMeta) -> ScanTree {
        let n = self.nodes.len();
        // Directories start with zero totals: their own size is not meaningful.
        for node in self.nodes.iter_mut() {
            if node.is_dir() {
                node.size = 0;
                node.alloc = 0;
                node.files = 0;
                node.dirs = 0;
            }
        }
        // Bottom-up aggregation relies on parent index < child index.
        for i in (1..n).rev() {
            let child = self.nodes[i];
            let p = child.parent as usize;
            debug_assert!(p < i, "builder invariant: parent before child");
            let parent = &mut self.nodes[p];
            if child.counts_toward_totals() {
                parent.size += child.size;
                parent.alloc += child.alloc;
            }
            if child.is_dir() {
                parent.dirs += 1 + child.dirs;
                parent.files += child.files;
            } else {
                parent.files += 1;
            }
            if child.modified > parent.modified {
                parent.modified = child.modified;
            }
        }

        // CSR child index.
        let mut counts = vec![0u32; n];
        for node in self.nodes.iter().skip(1) {
            counts[node.parent as usize] += 1;
        }
        let mut start = 0u32;
        for (i, node) in self.nodes.iter_mut().enumerate() {
            node.child_start = start;
            node.child_count = counts[i];
            start += counts[i];
        }
        let mut fill = vec![0u32; n];
        let mut children = vec![0 as NodeId; n.saturating_sub(1)];
        for i in 1..n {
            let p = self.nodes[i].parent as usize;
            let pos = self.nodes[p].child_start + fill[p];
            children[pos as usize] = i as NodeId;
            fill[p] += 1;
        }
        drop(fill);
        drop(counts);
        {
            use rayon::prelude::*;
            let nodes = &self.nodes;
            // Sort every sibling range by allocated size desc, then logical size.
            let mut ranges: Vec<&mut [NodeId]> = Vec::new();
            let mut rest: &mut [NodeId] = &mut children;
            for node in nodes.iter() {
                if node.child_count == 0 {
                    continue;
                }
                let (head, tail) = std::mem::take(&mut rest).split_at_mut(node.child_count as usize);
                ranges.push(head);
                rest = tail;
            }
            ranges.par_iter_mut().for_each(|r| {
                r.sort_unstable_by(|&a, &b| {
                    let (na, nb) = (&nodes[a as usize], &nodes[b as usize]);
                    nb.alloc.cmp(&na.alloc).then(nb.size.cmp(&na.size)).then(a.cmp(&b))
                })
            });
        }

        let root = &self.nodes[0];
        meta.files = root.files as u64;
        meta.dirs = root.dirs as u64;
        meta.total_size = root.size;
        meta.total_alloc = root.alloc;
        self.names.shrink_to_fit();
        self.nodes.shrink_to_fit();
        ScanTree::from_parts(meta, self.nodes, self.names, children, self.exts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size: u64) -> EntryInfo {
        EntryInfo { size, alloc: size.div_ceil(4096) * 4096, modified: size, ..Default::default() }
    }

    #[test]
    fn aggregates_and_sorts() {
        let mut b = TreeBuilder::new("C:\\", 8);
        let a = b.add(ROOT, "a", &EntryInfo { is_dir: true, ..Default::default() });
        let _f1 = b.add(a, "small.txt", &file(10));
        let _f2 = b.add(a, "big.MP4", &file(10_000));
        let _f3 = b.add(ROOT, "root.bin", &file(5000));
        let hl = b.add(ROOT, "link.bin", &EntryInfo { flags: flags::HARDLINK_DUP, ..file(5000) });
        let t = b.finish(ScanMeta { root_path: "C:\\".into(), ..Default::default() });

        assert_eq!(t.node(ROOT).size, 10 + 10_000 + 5000);
        assert_eq!(t.node(ROOT).files, 4);
        assert_eq!(t.node(ROOT).dirs, 1);
        assert_eq!(t.node(a).size, 10_010);
        assert_eq!(t.node(a).modified, 10_000);
        // children sorted by allocated size desc
        let kids = t.children(ROOT);
        assert_eq!(t.name(kids[0]), "a");
        assert_eq!(t.children(a).iter().map(|&c| t.name(c)).collect::<Vec<_>>(), vec!["big.MP4", "small.txt"]);
        assert_eq!(t.ext(t.children(a)[0]), "mp4");
        assert_eq!(t.path(t.children(a)[0]), "C:\\a\\big.MP4");
        assert_eq!(t.find_path("c:\\A\\BIG.mp4"), Some(t.children(a)[0]));
        assert_eq!(t.find_path("C:\\"), Some(ROOT));
        assert_eq!(t.find_path("D:\\a"), None);
        assert!(t.is_within(t.children(a)[1], a));
        // hard link duplicate keeps its size but is not counted
        assert_eq!(t.node(hl).size, 5000);
        assert_eq!(t.meta.total_size, 15_010);

        let mut t = t;
        let big = t.find_path("C:\\a\\big.MP4").unwrap();
        t.detach(big);
        assert_eq!(t.node(a).size, 10);
        assert_eq!(t.node(ROOT).files, 3);
        assert_eq!(t.children(a).len(), 1);
        assert!(t.find_path("C:\\a\\big.MP4").is_none());
        t.detach(a);
        assert_eq!(t.meta.total_size, 5000);
        assert_eq!(t.node(ROOT).dirs, 0);
    }
}
