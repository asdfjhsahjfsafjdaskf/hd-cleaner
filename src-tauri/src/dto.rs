//! Serializable shapes sent to the UI.

use hdcleaner_core::scan::model::flags;
use hdcleaner_core::scan::{NodeId, ScanTree, ROOT};
use hdcleaner_core::util::filetime_to_unix_ms;
use serde::Serialize;

pub type CmdResult<T> = Result<T, hdcleaner_core::ErrorPayload>;

pub trait IntoPayload<T> {
    fn ui(self) -> CmdResult<T>;
}

impl<T> IntoPayload<T> for hdcleaner_core::Result<T> {
    fn ui(self) -> CmdResult<T> {
        self.map_err(|e| {
            tracing::warn!(error = %e, "command failed");
            e.to_payload()
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRow {
    pub id: NodeId,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub alloc: u64,
    pub files: u32,
    pub dirs: u32,
    pub modified: i64,
    pub created: i64,
    pub accessed: i64,
    pub ext: String,
    pub category: &'static str,
    pub attributes: u32,
    pub flags: u16,
    pub links: u16,
    pub child_count: u32,
    /// Share of the parent's allocated size (0..1).
    pub parent_share: f32,
    pub depth: u16,
    pub expanded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub fn node_row(tree: &ScanTree, id: NodeId, with_path: bool) -> NodeRow {
    let n = tree.node(id);
    let parent_alloc = if id == ROOT { n.alloc } else { tree.node(n.parent).alloc };
    NodeRow {
        id,
        name: if id == ROOT { tree.meta.root_path.clone() } else { tree.name(id).to_string() },
        is_dir: n.is_dir(),
        size: n.size,
        alloc: n.alloc,
        files: n.files,
        dirs: n.dirs,
        modified: filetime_to_unix_ms(n.modified),
        created: filetime_to_unix_ms(n.created),
        accessed: filetime_to_unix_ms(n.accessed),
        ext: tree.ext(id).to_string(),
        category: tree.category(id).name(),
        attributes: n.attributes,
        flags: n.flags,
        links: n.links,
        child_count: n.child_count,
        parent_share: if parent_alloc > 0 { (n.alloc as f64 / parent_alloc as f64) as f32 } else { 0.0 },
        depth: 0,
        expanded: false,
        path: with_path.then(|| tree.path(id)),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDetails {
    #[serde(flatten)]
    pub row: NodeRow,
    pub path: String,
    pub ancestors: Vec<NodeId>,
    pub hardlink_duplicate: bool,
    pub cloud: bool,
    pub reparse: bool,
    pub unreadable: bool,
}

pub fn node_details(tree: &ScanTree, id: NodeId) -> NodeDetails {
    let n = tree.node(id);
    NodeDetails {
        row: node_row(tree, id, false),
        path: tree.path(id),
        ancestors: tree.ancestors(id),
        hardlink_duplicate: n.flags & flags::HARDLINK_DUP != 0,
        cloud: n.flags & flags::CLOUD != 0,
        reparse: n.flags & flags::REPARSE != 0,
        unreadable: n.flags & flags::UNREADABLE != 0,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<T>,
}
