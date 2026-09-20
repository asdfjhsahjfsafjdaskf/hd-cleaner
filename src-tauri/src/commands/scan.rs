//! Scanning, snapshots, statistics and treemap commands.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::scan::{ScanControl, ScanMeta, ScanMethod, ScanOptions, ScanProgress, ROOT};
use hdcleaner_core::{ErrorPayload, AppError};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "event", content = "data")]
pub enum ScanEvent {
    Progress(ScanProgress),
    Done { scan_id: u32, meta: ScanMeta },
    Cancelled,
    Error(ErrorPayload),
}

const MAX_LOADED_SCANS: usize = 4;
const SNAPSHOTS_PER_ROOT: usize = 12;

#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    root: String,
    method: ScanMethod,
    elevate: bool,
    follow_junctions: bool,
    on_event: Channel<ScanEvent>,
) -> CmdResult<u32> {
    let root = hdcleaner_core::util::normalize_root(&root).ui()?;
    let job_id = state.next_id();
    let ctl = Arc::new(ScanControl::new());
    state.jobs.lock().insert(job_id, ctl.clone());
    tracing::info!(%root, ?method, elevate, "scan requested");

    std::thread::Builder::new()
        .name(format!("scan-{job_id}"))
        .spawn(move || {
            let done = Arc::new(AtomicBool::new(false));
            let pump = {
                let (ctl, done, ch) = (ctl.clone(), done.clone(), on_event.clone());
                std::thread::spawn(move || {
                    while !done.load(Ordering::Relaxed) {
                        let _ = ch.send(ScanEvent::Progress(ctl.snapshot()));
                        std::thread::sleep(Duration::from_millis(150));
                    }
                })
            };
            let is_volume = root.len() == 3 && root.ends_with(":\\");
            let result = if elevate && is_volume && !hdcleaner_core::system::is_elevated() {
                hdcleaner_core::elevation::scan_ntfs_elevated(root.chars().next().unwrap_or('C'), &ctl).map(|mut t| {
                    t.meta.notes.push("elevatedHelper".into());
                    t
                })
            } else {
                hdcleaner_core::scan::run_scan(&root, &ScanOptions { method, follow_junctions }, &ctl)
            };
            done.store(true, Ordering::Relaxed);
            let _ = pump.join();
            let state = app.state::<AppState>();
            state.jobs.lock().remove(&job_id);
            match result {
                Ok(tree) => {
                    let meta = tree.meta.clone();
                    let save = state.setting_bool("saveSnapshots", true);
                    let snapshot = if save { save_snapshot_file(&state, &tree) } else { None };
                    if let Err(e) = state.db.lock().add_scan(&meta, snapshot.as_deref()) {
                        tracing::warn!("recording scan failed: {e}");
                    }
                    prune_snapshots(&state, &meta.root_path);
                    let _ = state.db.lock().record_operation(
                        "scan",
                        "completed",
                        &meta.root_path,
                        meta.files as usize,
                        &serde_json::json!({
                            "root": meta.root_path, "method": meta.method, "files": meta.files, "dirs": meta.dirs,
                            "allocated": meta.total_alloc, "durationMs": meta.duration_ms, "unreadableFolders": meta.error_count,
                        }),
                    );
                    let scan_id = state.insert_scan(tree, MAX_LOADED_SCANS);
                    let _ = on_event.send(ScanEvent::Done { scan_id, meta });
                }
                Err(AppError::Cancelled) => {
                    let _ = on_event.send(ScanEvent::Cancelled);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "scan failed");
                    let _ = state.db.lock().record_operation(
                        "scan",
                        "failed",
                        &root,
                        0,
                        &serde_json::json!({ "root": root, "error": e.to_payload() }),
                    );
                    let _ = on_event.send(ScanEvent::Error(e.to_payload()));
                }
            }
        })
        .map_err(|e| AppError::io("starting scan thread", None, e).to_payload())?;
    Ok(job_id)
}

fn save_snapshot_file(state: &AppState, tree: &hdcleaner_core::scan::ScanTree) -> Option<String> {
    let dir = state.data_dir.join("snapshots");
    hdcleaner_core::util::ensure_dir(&dir).ok()?;
    let safe: String = tree
        .meta
        .root_path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(60)
        .collect();
    let path = dir.join(format!("{safe}-{}.hdcs", tree.meta.started_ms));
    match hdcleaner_core::scan::snapshot::save(tree, &path) {
        Ok(()) => Some(path.to_string_lossy().into_owned()),
        Err(e) => {
            tracing::warn!("snapshot not saved: {e}");
            None
        }
    }
}

fn prune_snapshots(state: &AppState, root: &str) {
    let db = state.db.lock();
    let Ok(list) = db.list_scans(Some(root), 1000) else { return };
    for rec in list.into_iter().skip(SNAPSHOTS_PER_ROOT) {
        if let Ok(Some(path)) = db.delete_scan(rec.id) {
            let p = std::path::Path::new(&path);
            // Only ever delete files inside our own snapshot folder.
            if p.starts_with(state.data_dir.join("snapshots")) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, job_id: u32) -> bool {
    match state.jobs.lock().get(&job_id) {
        Some(ctl) => {
            ctl.cancel();
            true
        }
        None => false,
    }
}

#[tauri::command]
pub fn close_scan(state: State<'_, AppState>, scan_id: u32) {
    state.scans.write().remove(&scan_id);
    state.views.lock().retain(|_, v| v.scan_id != scan_id);
    state.results.lock().retain(|_, r| r.scan_id != scan_id);
}

#[tauri::command]
pub fn loaded_scans(state: State<'_, AppState>) -> Vec<(u32, ScanMeta)> {
    let mut v: Vec<(u32, ScanMeta)> = state.scans.read().iter().map(|(k, t)| (*k, t.read().meta.clone())).collect();
    v.sort_by_key(|(k, _)| *k);
    v
}

#[tauri::command]
pub fn node_details(state: State<'_, AppState>, scan_id: u32, node: u32) -> CmdResult<NodeDetails> {
    let t = state.tree(scan_id)?;
    let t = t.read();
    if t.get(node).is_none() || t.is_detached(node) {
        return Err(AppError::NotFound { path: format!("node {node}") }.to_payload());
    }
    Ok(crate::dto::node_details(&t, node))
}

#[tauri::command]
pub fn find_path(state: State<'_, AppState>, scan_id: u32, path: String) -> CmdResult<Option<u32>> {
    let t = state.tree(scan_id)?;
    let t = t.read();
    Ok(t.find_path(&path))
}

#[tauri::command]
pub fn scan_stats(state: State<'_, AppState>, scan_id: u32, scope: Option<u32>) -> CmdResult<hdcleaner_core::stats::ScanStats> {
    let t = state.tree(scan_id)?;
    let t = t.read();
    Ok(hdcleaner_core::stats::compute(&t, scope.unwrap_or(ROOT), 60))
}

#[tauri::command]
pub fn treemap(
    state: State<'_, AppState>,
    scan_id: u32,
    root: u32,
    options: hdcleaner_core::treemap::TreemapOptions,
) -> CmdResult<Response> {
    let t = state.tree(scan_id)?;
    let t = t.read();
    if t.get(root).is_none() {
        return Err(AppError::InvalidInput("unknown treemap root".into()).to_payload());
    }
    let layout = hdcleaner_core::treemap::layout(&t, root, &options);
    Ok(Response::new(layout.bytes))
}

// ---- snapshots ------------------------------------------------------------

#[tauri::command]
pub fn list_snapshots(state: State<'_, AppState>, root: Option<String>) -> CmdResult<Vec<hdcleaner_core::db::ScanRecord>> {
    state.db.lock().list_scans(root.as_deref(), 200).ui()
}

#[tauri::command]
pub async fn open_snapshot(state: State<'_, AppState>, path: String) -> CmdResult<(u32, ScanMeta)> {
    let tree = hdcleaner_core::scan::snapshot::load(std::path::Path::new(&path)).ui()?;
    let meta = tree.meta.clone();
    let id = state.insert_scan(tree, MAX_LOADED_SCANS);
    Ok((id, meta))
}

#[tauri::command]
pub async fn export_snapshot(state: State<'_, AppState>, scan_id: u32, path: String) -> CmdResult<()> {
    let t = state.tree(scan_id)?;
    let t = t.read();
    hdcleaner_core::scan::snapshot::save(&t, std::path::Path::new(&path)).ui()
}

/// Compare a stored snapshot with a loaded scan (or two snapshots).
#[tauri::command]
pub async fn compare_snapshot(
    state: State<'_, AppState>,
    old_path: String,
    scan_id: u32,
    limit: Option<usize>,
) -> CmdResult<hdcleaner_core::diff::DiffReport> {
    let old = hdcleaner_core::scan::snapshot::load(std::path::Path::new(&old_path)).ui()?;
    let t = state.tree(scan_id)?;
    let t = t.read();
    Ok(hdcleaner_core::diff::diff(&old, &t, limit.unwrap_or(200)))
}

#[tauri::command]
pub async fn export_scan(
    state: State<'_, AppState>,
    scan_id: u32,
    scope: Option<u32>,
    format: hdcleaner_core::export::ExportFormat,
    path: String,
) -> CmdResult<u64> {
    let t = state.tree(scan_id)?;
    let t = t.read();
    let rows = hdcleaner_core::export::export_to_file(
        &t,
        &hdcleaner_core::export::ExportSet::Subtree(scope.unwrap_or(ROOT)),
        format,
        std::path::Path::new(&path),
    )
    .ui()?;
    let _ = state.db.lock().record_operation(
        "export",
        "completed",
        &format!("{rows} rows"),
        rows as usize,
        &serde_json::json!({ "format": format!("{format:?}"), "file": path }),
    );
    Ok(rows)
}
