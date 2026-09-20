//! File operations: plan → confirm → execute, plus shell integration.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::fsops::{self, DeleteMode, DeletePlan, ExecuteOptions, ItemOutcome, ItemResult, Terminal};
use hdcleaner_core::AppError;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// Node inside a loaded scan (preferred: path and size come from the scan).
    pub node: Option<u32>,
    /// Or an explicit path (e.g. from a file dialog).
    pub path: Option<String>,
}

#[tauri::command]
pub async fn plan_delete(
    state: State<'_, AppState>,
    scan_id: Option<u32>,
    targets: Vec<Target>,
    mode: DeleteMode,
) -> CmdResult<DeletePlan> {
    if targets.is_empty() {
        return Err(AppError::InvalidInput("nothing selected".into()).to_payload());
    }
    let mut items: Vec<(String, Option<u64>)> = Vec::with_capacity(targets.len());
    {
        let tree = scan_id.map(|id| state.tree(id)).transpose()?;
        let guard = tree.as_ref().map(|t| t.read());
        for t in &targets {
            match (t.node, &t.path, guard.as_ref()) {
                (Some(n), _, Some(tree)) if tree.get(n).is_some() && !tree.is_detached(n) => {
                    let node = tree.node(n);
                    items.push((tree.path(n), Some(node.alloc)));
                }
                (_, Some(p), _) => items.push((p.clone(), None)),
                _ => return Err(AppError::InvalidInput("invalid target".into()).to_payload()),
            }
        }
    }
    let plan = fsops::plan_delete(&items, mode);
    state.plans.lock().insert(plan.id.clone(), (plan.clone(), scan_id));
    Ok(plan)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "event", content = "data")]
pub enum DeleteEvent {
    Item { index: usize, total: usize, result: ItemResult },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSummary {
    pub operation_id: i64,
    pub deleted: usize,
    pub would_delete: usize,
    pub skipped: usize,
    pub failed: usize,
    pub freed: u64,
    pub results: Vec<ItemResult>,
}

#[tauri::command]
pub async fn execute_delete(
    state: State<'_, AppState>,
    plan_id: String,
    dry_run: bool,
    allow_dangerous: bool,
    on_event: Channel<DeleteEvent>,
) -> CmdResult<DeleteSummary> {
    let (plan, scan_id) = state
        .plans
        .lock()
        .remove(&plan_id)
        .ok_or_else(|| AppError::InvalidInput("this deletion plan expired; review the selection again".into()).to_payload())?;
    let kind = match (plan.mode, dry_run) {
        (_, true) => "delete-dry-run",
        (DeleteMode::RecycleBin, _) => "recycle",
        (DeleteMode::Permanent, _) => "delete-permanent",
    };
    let paths: Vec<&str> = plan.items.iter().map(|i| i.path.as_str()).collect();
    // Journal first: if the app dies mid-way the history shows it interrupted.
    let op_id = state
        .db
        .lock()
        .begin_operation(kind, &format!("{} items", plan.items.len()), plan.items.len(), dry_run, &serde_json::json!({ "paths": paths, "mode": plan.mode }))
        .ui()?;
    let total = plan.items.len();
    let results = fsops::execute_delete(
        &plan,
        &ExecuteOptions { dry_run, allow_dangerous },
        |index, r| {
            let _ = on_event.send(DeleteEvent::Item { index, total, result: r.clone() });
        },
        || true,
    );
    let mut s = DeleteSummary { operation_id: op_id, deleted: 0, would_delete: 0, skipped: 0, failed: 0, freed: 0, results: Vec::new() };
    let mut deleted_paths = Vec::new();
    for (r, item) in results.iter().zip(plan.items.iter()) {
        match &r.outcome {
            ItemOutcome::Deleted => {
                s.deleted += 1;
                s.freed += item.size;
                deleted_paths.push(r.path.clone());
            }
            ItemOutcome::WouldDelete => {
                s.would_delete += 1;
                s.freed += item.size;
            }
            ItemOutcome::Skipped { .. } => s.skipped += 1,
            ItemOutcome::Failed { .. } => s.failed += 1,
        }
    }
    let status = if s.failed == 0 && s.skipped == 0 { "completed" } else if s.deleted + s.would_delete > 0 { "partial" } else { "failed" };
    let _ = state.db.lock().finish_operation(
        op_id,
        status,
        s.deleted + s.would_delete,
        s.failed,
        &serde_json::json!({ "mode": plan.mode, "results": results, "freed": s.freed }),
    );
    // Keep the loaded scan consistent without a rescan.
    if let (Some(scan_id), false) = (scan_id, deleted_paths.is_empty()) {
        if let Ok(tree) = state.tree(scan_id) {
            let mut t = tree.write();
            for p in &deleted_paths {
                if let Some(id) = t.find_path(p) {
                    t.detach(id);
                }
            }
        }
        for v in state.views.lock().values_mut() {
            if v.scan_id == scan_id {
                v.rows = None;
            }
        }
    }
    s.results = results;
    Ok(s)
}

#[tauri::command]
pub fn open_path(path: String) -> CmdResult<()> {
    fsops::open_path(&path).ui()
}

#[tauri::command]
pub fn reveal_path(path: String) -> CmdResult<()> {
    fsops::reveal_in_explorer(&path).ui()
}

#[tauri::command]
pub fn show_properties(path: String) -> CmdResult<()> {
    fsops::show_properties(&path).ui()
}

#[tauri::command]
pub fn open_terminal(path: String, kind: Terminal) -> CmdResult<()> {
    fsops::open_terminal(&path, kind).ui()
}

#[tauri::command]
pub async fn rename_path(state: State<'_, AppState>, path: String, new_name: String) -> CmdResult<String> {
    let new_path = fsops::rename(&path, &new_name).ui()?;
    let _ = state.db.lock().record_operation(
        "rename",
        "completed",
        &new_name,
        1,
        &serde_json::json!({ "from": path, "to": new_path }),
    );
    Ok(new_path)
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum TransferKind {
    Copy,
    Move,
}

#[tauri::command]
pub async fn transfer(state: State<'_, AppState>, paths: Vec<String>, dest: String, kind: TransferKind) -> CmdResult<Vec<ItemResult>> {
    let label = match kind {
        TransferKind::Copy => "copy",
        TransferKind::Move => "move",
    };
    let op = state
        .db
        .lock()
        .begin_operation(label, &format!("{} items → {dest}", paths.len()), paths.len(), false, &serde_json::json!({ "paths": paths, "dest": dest }))
        .ui()?;
    let mut out = Vec::new();
    for p in &paths {
        let r = match kind {
            TransferKind::Copy => fsops::copy_to(p, &dest),
            TransferKind::Move => fsops::move_to(p, &dest),
        };
        out.push(ItemResult {
            path: p.clone(),
            outcome: match r {
                Ok(()) => ItemOutcome::Deleted, // "done" for transfers
                Err(AppError::Cancelled) => ItemOutcome::Skipped { reason: "cancelled".into() },
                Err(e) => ItemOutcome::Failed { error: e.to_payload() },
            },
        });
    }
    let failed = out.iter().filter(|r| matches!(r.outcome, ItemOutcome::Failed { .. })).count();
    let _ = state.db.lock().finish_operation(
        op,
        if failed == 0 { "completed" } else { "partial" },
        out.len() - failed,
        failed,
        &serde_json::json!({ "results": out, "dest": dest }),
    );
    Ok(out)
}
