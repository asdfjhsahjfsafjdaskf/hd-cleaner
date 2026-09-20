//! Compact in-memory representation of a scanned file system tree.
//!
//! Design goals (millions of entries):
//! * one flat `Vec<Node>` (fixed size records, no per-node heap allocation);
//! * all names in a single UTF-8 pool addressed by offset/len;
//! * children stored in CSR form (`child_start..child_start+child_count` into
//!   `children`), pre-sorted by allocated size descending;
//! * extensions interned into a small table (u16 id).

use crate::category::{category_for_ext, Category};
use serde::{Deserialize, Serialize};

pub type NodeId = u32;
pub const ROOT: NodeId = 0;
pub const NO_NODE: NodeId = u32::MAX;

pub mod flags {
    pub const DIR: u16 = 1 << 0;
    /// Additional hard link of a file already counted elsewhere: shown with its
    /// size, but excluded from directory totals so space is never counted twice.
    pub const HARDLINK_DUP: u16 = 1 << 1;
    pub const REPARSE: u16 = 1 << 2;
    /// Cloud placeholder / offline (OneDrive "online-only" etc.).
    pub const CLOUD: u16 = 1 << 3;
    /// Directory could not be enumerated (access denied, I/O error...).
    pub const UNREADABLE: u16 = 1 << 4;
    /// NTFS metadata file ($MFT, $LogFile...).
    pub const METAFILE: u16 = 1 << 5;
    pub const SPARSE: u16 = 1 << 6;
    pub const COMPRESSED: u16 = 1 << 7;
    pub const SYMLINK: u16 = 1 << 8;
    pub const MOUNT_POINT: u16 = 1 << 9;
    /// Reparse directory that was intentionally not followed.
    pub const NOT_FOLLOWED: u16 = 1 << 10;
    /// File has more than one hard link.
    pub const MULTI_LINK: u16 = 1 << 11;
    /// Removed from the tree after a deletion (kept in the arena, unreachable).
    pub const DETACHED: u16 = 1 << 12;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Node {
    pub parent: NodeId,
    pub name_off: u32,
    /// Logical size (end of file). Aggregated for directories.
    pub size: u64,
    /// Allocated size on disk. Aggregated for directories.
    pub alloc: u64,
    /// FILETIME values (100 ns since 1601). For directories `modified` is the
    /// newest modification time of anything below it.
    pub created: u64,
    pub modified: u64,
    pub accessed: u64,
    /// Descendant file / directory counts (directories only).
    pub files: u32,
    pub dirs: u32,
    pub child_start: u32,
    pub child_count: u32,
    pub attributes: u32,
    pub name_len: u16,
    pub ext: u16,
    pub flags: u16,
    pub links: u16,
}

impl Node {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.flags & flags::DIR != 0
    }
    #[inline]
    pub fn counts_toward_totals(&self) -> bool {
        self.flags & flags::HARDLINK_DUP == 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanErrorSample {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanMeta {
    pub root_path: String,
    /// "ntfsMft" | "standard"
    pub method: String,
    pub file_system: String,
    pub started_ms: i64,
    pub duration_ms: u64,
    pub files: u64,
    pub dirs: u64,
    pub total_size: u64,
    pub total_alloc: u64,
    pub volume_total: u64,
    pub volume_free: u64,
    pub error_count: u64,
    pub error_samples: Vec<ScanErrorSample>,
    /// Notes the UI should show (e.g. fallbacks taken).
    pub notes: Vec<String>,
}

pub struct ScanTree {
    pub meta: ScanMeta,
    pub(crate) nodes: Vec<Node>,
    pub(crate) names: Vec<u8>,
    pub(crate) children: Vec<NodeId>,
    pub(crate) exts: Vec<String>,
    pub(crate) ext_cats: Vec<Category>,
}

impl ScanTree {
    pub(crate) fn from_parts(
        meta: ScanMeta,
        nodes: Vec<Node>,
        names: Vec<u8>,
        children: Vec<NodeId>,
        exts: Vec<String>,
    ) -> Self {
        let ext_cats = exts.iter().map(|e| category_for_ext(e)).collect();
        ScanTree { meta, nodes, names, children, exts, ext_cats }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id as usize)
    }
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    #[inline]
    pub fn name(&self, id: NodeId) -> &str {
        let n = &self.nodes[id as usize];
        let bytes = &self.names[n.name_off as usize..n.name_off as usize + n.name_len as usize];
        // Names are written from `&str`, so they are valid UTF-8.
        std::str::from_utf8(bytes).unwrap_or("?")
    }
    #[inline]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        let n = &self.nodes[id as usize];
        &self.children[n.child_start as usize..(n.child_start + n.child_count) as usize]
    }
    #[inline]
    pub fn ext(&self, id: NodeId) -> &str {
        &self.exts[self.nodes[id as usize].ext as usize]
    }
    pub fn ext_table(&self) -> &[String] {
        &self.exts
    }
    #[inline]
    pub fn category(&self, id: NodeId) -> Category {
        let n = &self.nodes[id as usize];
        if n.is_dir() {
            Category::Other
        } else {
            self.ext_cats[n.ext as usize]
        }
    }
    #[inline]
    pub fn ext_category(&self, ext_id: u16) -> Category {
        self.ext_cats[ext_id as usize]
    }

    /// Full display path of a node.
    pub fn path(&self, id: NodeId) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(16);
        let mut cur = id;
        while cur != ROOT {
            parts.push(self.name(cur));
            cur = self.nodes[cur as usize].parent;
        }
        let mut out = self.meta.root_path.clone();
        for p in parts.iter().rev() {
            if !out.ends_with('\\') {
                out.push('\\');
            }
            out.push_str(p);
        }
        out
    }

    /// Chain of ancestors from root to `id` (inclusive).
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = vec![id];
        let mut cur = id;
        while cur != ROOT {
            cur = self.nodes[cur as usize].parent;
            v.push(cur);
        }
        v.reverse();
        v
    }

    /// Resolve an absolute path (case-insensitive) to a node inside this tree.
    pub fn find_path(&self, path: &str) -> Option<NodeId> {
        let root = self.meta.root_path.trim_end_matches('\\');
        let p = path.replace('/', "\\");
        let p = p.trim_end_matches('\\');
        if p.len() < root.len() || !p[..root.len()].eq_ignore_ascii_case(root) {
            return None;
        }
        let rest = &p[root.len()..];
        if !rest.is_empty() && !rest.starts_with('\\') {
            return None;
        }
        let mut cur = ROOT;
        for comp in rest.split('\\').filter(|c| !c.is_empty()) {
            let next = self
                .children(cur)
                .iter()
                .copied()
                .find(|&c| self.name(c).eq_ignore_ascii_case(comp))?;
            cur = next;
        }
        Some(cur)
    }

    /// Is `id` equal to, or inside, `ancestor`?
    pub fn is_within(&self, id: NodeId, ancestor: NodeId) -> bool {
        let mut cur = id;
        loop {
            if cur == ancestor {
                return true;
            }
            if cur == ROOT {
                return false;
            }
            cur = self.nodes[cur as usize].parent;
        }
    }

    pub fn depth(&self, id: NodeId) -> u32 {
        let mut d = 0;
        let mut cur = id;
        while cur != ROOT {
            cur = self.nodes[cur as usize].parent;
            d += 1;
        }
        d
    }

    #[inline]
    pub fn is_detached(&self, id: NodeId) -> bool {
        self.nodes[id as usize].flags & flags::DETACHED != 0
    }

    /// Remove a subtree after it was deleted from disk: ancestors' totals are
    /// reduced, the node leaves its parent's child list and every node of the
    /// subtree is flagged `DETACHED` (skipped by search/duplicates).
    pub fn detach(&mut self, id: NodeId) {
        if id == ROOT || self.is_detached(id) {
            return;
        }
        let n = self.nodes[id as usize];
        let (size, alloc) = if n.counts_toward_totals() { (n.size, n.alloc) } else { (0, 0) };
        let (files, dirs) = if n.is_dir() { (n.files, n.dirs + 1) } else { (1, 0) };
        let mut cur = n.parent;
        while cur != NO_NODE {
            let a = &mut self.nodes[cur as usize];
            a.size = a.size.saturating_sub(size);
            a.alloc = a.alloc.saturating_sub(alloc);
            a.files = a.files.saturating_sub(files);
            a.dirs = a.dirs.saturating_sub(dirs);
            cur = a.parent;
        }
        let p = &mut self.nodes[n.parent as usize];
        let (start, count) = (p.child_start as usize, p.child_count as usize);
        let slice = &mut self.children[start..start + count];
        if let Some(pos) = slice.iter().position(|&c| c == id) {
            slice[pos..].rotate_left(1);
            p.child_count -= 1;
        }
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            self.nodes[x as usize].flags |= flags::DETACHED;
            let nx = self.nodes[x as usize];
            stack.extend_from_slice(&self.children[nx.child_start as usize..(nx.child_start + nx.child_count) as usize]);
        }
        self.meta.total_size = self.nodes[0].size;
        self.meta.total_alloc = self.nodes[0].alloc;
        self.meta.files = self.nodes[0].files as u64;
        self.meta.dirs = self.nodes[0].dirs as u64;
    }

    /// Approximate heap usage in bytes (for diagnostics).
    pub fn memory_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.names.capacity()
            + self.children.capacity() * 4
            + self.exts.iter().map(|e| e.capacity() + 24).sum::<usize>()
    }
}
