//! Virtualized tree view: the UI asks for pages of already-flattened rows.

use crate::dto::*;
use crate::state::{AppState, TreeView};
use hdcleaner_core::scan::{NodeId, ScanTree};
use hdcleaner_core::search::SortKey;
use hdcleaner_core::AppError;
use std::collections::HashSet;
use tauri::State;

fn sorted_children(tree: &ScanTree, id: NodeId, sort: SortKey, desc: bool) -> Vec<NodeId> {
    let kids = tree.children(id);
    if sort == SortKey::Alloc && desc {
        return kids.to_vec(); // CSR order is already allocated-size descending
    }
    let mut v = kids.to_vec();
    hdcleaner_core::search::sort_ids(tree, &mut v, sort, desc, 0);
    // Folders first for name sorting, like file managers do.
    if sort == SortKey::Name {
        v.sort_by_key(|&c| !tree.node(c).is_dir());
    }
    v
}

fn flatten(view: &mut TreeView, tree: &ScanTree) {
    if view.rows.is_some() {
        return;
    }
    let mut rows = Vec::new();
    // Iterative DFS; children pushed in reverse to keep display order.
    let mut stack: Vec<(NodeId, u16)> = vec![(view.root, 0)];
    while let Some((id, depth)) = stack.pop() {
        rows.push((id, depth));
        if view.expanded.contains(&id) && tree.node(id).is_dir() {
            let kids = sorted_children(tree, id, view.sort, view.desc);
            for &c in kids.iter().rev() {
                stack.push((c, depth + 1));
            }
        }
    }
    view.rows = Some(rows);
}

fn with_view<R>(
    state: &AppState,
    view_id: u32,
    f: impl FnOnce(&mut TreeView, &ScanTree) -> R,
) -> CmdResult<R> {
    let mut views = state.views.lock();
    let view = views
        .get_mut(&view_id)
        .ok_or_else(|| AppError::InvalidInput(format!("tree view {view_id} is closed")).to_payload())?;
    let tree = state.tree(view.scan_id)?;
    let tree = tree.read();
    Ok(f(view, &tree))
}

#[tauri::command]
pub fn view_open(state: State<'_, AppState>, scan_id: u32, root: Option<u32>) -> CmdResult<u32> {
    let tree = state.tree(scan_id)?;
    let root = root.unwrap_or(0);
    if tree.read().get(root).is_none() {
        return Err(AppError::InvalidInput("unknown root".into()).to_payload());
    }
    let id = state.next_id();
    let mut expanded = HashSet::new();
    expanded.insert(root);
    state.views.lock().insert(
        id,
        TreeView { scan_id, root, expanded, sort: SortKey::Alloc, desc: true, rows: None },
    );
    Ok(id)
}

#[tauri::command]
pub fn view_close(state: State<'_, AppState>, view_id: u32) {
    state.views.lock().remove(&view_id);
}

#[tauri::command]
pub fn view_rows(state: State<'_, AppState>, view_id: u32, offset: usize, limit: usize) -> CmdResult<Page<NodeRow>> {
    with_view(&state, view_id, |view, tree| {
        flatten(view, tree);
        let rows = view.rows.as_ref().unwrap();
        let end = (offset + limit.min(2000)).min(rows.len());
        let start = offset.min(end);
        let page = rows[start..end]
            .iter()
            .map(|&(id, depth)| {
                let mut r = node_row(tree, id, false);
                r.depth = depth;
                r.expanded = view.expanded.contains(&id);
                r
            })
            .collect();
        Page { total: rows.len(), offset: start, rows: page }
    })
}

#[tauri::command]
pub fn view_set_expanded(state: State<'_, AppState>, view_id: u32, node: u32, expanded: bool) -> CmdResult<usize> {
    with_view(&state, view_id, |view, tree| {
        if expanded {
            view.expanded.insert(node);
        } else {
            view.expanded.remove(&node);
        }
        view.rows = None;
        flatten(view, tree);
        view.rows.as_ref().map(|r| r.len()).unwrap_or(0)
    })
}

/// Expand every directory below `node` (default: the view root), up to
/// `max_nodes` directories to keep the row list bounded.
#[tauri::command]
pub fn view_expand_all(state: State<'_, AppState>, view_id: u32, node: Option<u32>, max_dirs: Option<usize>) -> CmdResult<usize> {
    with_view(&state, view_id, |view, tree| {
        let start = node.unwrap_or(view.root);
        let limit = max_dirs.unwrap_or(200_000);
        let mut stack = vec![start];
        let mut n = 0;
        while let Some(id) = stack.pop() {
            if n >= limit {
                break;
            }
            if tree.node(id).is_dir() && tree.node(id).child_count > 0 {
                view.expanded.insert(id);
                n += 1;
                stack.extend(tree.children(id).iter().filter(|&&c| tree.node(c).is_dir()));
            }
        }
        view.rows = None;
        flatten(view, tree);
        view.rows.as_ref().map(|r| r.len()).unwrap_or(0)
    })
}

#[tauri::command]
pub fn view_collapse_all(state: State<'_, AppState>, view_id: u32) -> CmdResult<usize> {
    with_view(&state, view_id, |view, tree| {
        view.expanded.clear();
        view.expanded.insert(view.root);
        view.rows = None;
        flatten(view, tree);
        view.rows.as_ref().map(|r| r.len()).unwrap_or(0)
    })
}

#[tauri::command]
pub fn view_sort(state: State<'_, AppState>, view_id: u32, sort: SortKey, desc: bool) -> CmdResult<usize> {
    with_view(&state, view_id, |view, tree| {
        view.sort = sort;
        view.desc = desc;
        view.rows = None;
        flatten(view, tree);
        view.rows.as_ref().map(|r| r.len()).unwrap_or(0)
    })
}

/// Expand ancestors of `node` and return its row index.
#[tauri::command]
pub fn view_reveal(state: State<'_, AppState>, view_id: u32, node: u32) -> CmdResult<Option<usize>> {
    with_view(&state, view_id, |view, tree| {
        if tree.get(node).is_none() || !tree.is_within(node, view.root) {
            return None;
        }
        for a in tree.ancestors(node) {
            if a != node {
                view.expanded.insert(a);
            }
        }
        view.rows = None;
        flatten(view, tree);
        view.rows.as_ref().and_then(|r| r.iter().position(|&(id, _)| id == node))
    })
}

/// Expanded folders as paths, so the state survives a rescan (node ids change).
#[tauri::command]
pub fn view_expanded_paths(state: State<'_, AppState>, view_id: u32) -> CmdResult<Vec<String>> {
    with_view(&state, view_id, |view, tree| {
        let mut v: Vec<String> = view.expanded.iter().filter(|&&id| !tree.is_detached(id)).map(|&id| tree.path(id)).collect();
        v.sort();
        v
    })
}

#[tauri::command]
pub fn view_restore(state: State<'_, AppState>, view_id: u32, paths: Vec<String>) -> CmdResult<usize> {
    with_view(&state, view_id, |view, tree| {
        for p in paths.iter().take(100_000) {
            if let Some(id) = tree.find_path(p) {
                view.expanded.insert(id);
            }
        }
        view.rows = None;
        flatten(view, tree);
        view.rows.as_ref().map(|r| r.len()).unwrap_or(0)
    })
}
