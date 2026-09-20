//! Search / largest files / duplicates.

use crate::dto::*;
use crate::state::{AppState, DupEntry, ResultSet};
use hdcleaner_core::duplicates::{AutoSelect, DupOptions};
use hdcleaner_core::scan::{ScanControl, ScanProgress, ROOT};
use hdcleaner_core::search::{Query, SortKey};
use hdcleaner_core::{ErrorPayload, AppError};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSummary {
    pub result_id: u32,
    pub total: usize,
    pub total_size: u64,
    pub total_alloc: u64,
    pub elapsed_ms: u64,
}

#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    scan_id: u32,
    query: String,
    scope: Option<u32>,
    sort: Option<SortKey>,
    desc: Option<bool>,
    limit: Option<usize>,
) -> CmdResult<SearchSummary> {
    let t0 = std::time::Instant::now();
    let q = Query::parse(&query).ui()?;
    let tree = state.tree(scan_id)?;
    let r = {
        let t = tree.read();
        hdcleaner_core::search::search(&t, &q, scope.unwrap_or(ROOT), sort.unwrap_or(SortKey::Size), desc.unwrap_or(true), limit.unwrap_or(0))
    };
    let id = state.next_id();
    let summary = SearchSummary {
        result_id: id,
        total: r.ids.len(),
        total_size: r.total_size,
        total_alloc: r.total_alloc,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    };
    let mut results = state.results.lock();
    // Bound memory: keep only the most recent result sets.
    while results.len() >= 16 {
        let Some(&oldest) = results.keys().min() else { break };
        results.remove(&oldest);
    }
    results.insert(id, ResultSet { scan_id, ids: r.ids });
    Ok(summary)
}

#[tauri::command]
pub fn result_rows(state: State<'_, AppState>, result_id: u32, offset: usize, limit: usize) -> CmdResult<Page<NodeRow>> {
    let results = state.results.lock();
    let rs = results
        .get(&result_id)
        .ok_or_else(|| AppError::InvalidInput("result set expired; search again".into()).to_payload())?;
    let tree = state.tree(rs.scan_id)?;
    let t = tree.read();
    let end = (offset + limit.min(2000)).min(rs.ids.len());
    let start = offset.min(end);
    let rows = rs.ids[start..end]
        .iter()
        .filter(|&&id| !t.is_detached(id))
        .map(|&id| node_row(&t, id, true))
        .collect();
    Ok(Page { total: rs.ids.len(), offset: start, rows })
}

#[tauri::command]
pub async fn result_sort(state: State<'_, AppState>, result_id: u32, sort: SortKey, desc: bool) -> CmdResult<()> {
    let mut results = state.results.lock();
    let rs = results
        .get_mut(&result_id)
        .ok_or_else(|| AppError::InvalidInput("result set expired; search again".into()).to_payload())?;
    let tree = state.tree(rs.scan_id)?;
    let t = tree.read();
    hdcleaner_core::search::sort_ids(&t, &mut rs.ids, sort, desc, 0);
    Ok(())
}

#[tauri::command]
pub async fn export_results(
    state: State<'_, AppState>,
    result_id: u32,
    format: hdcleaner_core::export::ExportFormat,
    path: String,
) -> CmdResult<u64> {
    let results = state.results.lock();
    let rs = results
        .get(&result_id)
        .ok_or_else(|| AppError::InvalidInput("result set expired; search again".into()).to_payload())?;
    let tree = state.tree(rs.scan_id)?;
    let t = tree.read();
    hdcleaner_core::export::export_to_file(&t, &hdcleaner_core::export::ExportSet::Ids(&rs.ids), format, std::path::Path::new(&path)).ui()
}

// ---- duplicates -------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "event", content = "data")]
pub enum DupEvent {
    Progress(ScanProgress),
    Done { dup_id: u32, groups: usize, total_wasted: u64, bytes_hashed: u64, unreadable: u64 },
    Cancelled,
    Error(ErrorPayload),
}

#[tauri::command]
pub fn dup_start(
    app: AppHandle,
    state: State<'_, AppState>,
    scan_id: u32,
    options: DupOptions,
    on_event: Channel<DupEvent>,
) -> CmdResult<u32> {
    let tree = state.tree(scan_id)?;
    let job_id = state.next_id();
    let ctl = Arc::new(ScanControl::new());
    state.jobs.lock().insert(job_id, ctl.clone());
    std::thread::spawn(move || {
        let done = Arc::new(AtomicBool::new(false));
        let pump = {
            let (ctl, done, ch) = (ctl.clone(), done.clone(), on_event.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    let _ = ch.send(DupEvent::Progress(ctl.snapshot()));
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            })
        };
        let result = {
            let t = tree.read();
            hdcleaner_core::duplicates::find_duplicates(&t, &options, &ctl)
        };
        done.store(true, Ordering::Relaxed);
        let _ = pump.join();
        let state = app.state::<AppState>();
        state.jobs.lock().remove(&job_id);
        match result {
            Ok(report) => {
                let ev = DupEvent::Done {
                    dup_id: job_id,
                    groups: report.groups.len(),
                    total_wasted: report.total_wasted,
                    bytes_hashed: report.bytes_hashed,
                    unreadable: report.unreadable,
                };
                state.dups.lock().insert(job_id, DupEntry { scan_id, report: Arc::new(report) });
                let _ = on_event.send(ev);
            }
            Err(AppError::Cancelled) => {
                let _ = on_event.send(DupEvent::Cancelled);
            }
            Err(e) => {
                let _ = on_event.send(DupEvent::Error(e.to_payload()));
            }
        }
    });
    Ok(job_id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroupDto {
    pub id: u32,
    pub size: u64,
    pub hash: String,
    pub wasted: u64,
    pub files: Vec<NodeRow>,
}

#[tauri::command]
pub fn dup_groups(state: State<'_, AppState>, dup_id: u32, offset: usize, limit: usize) -> CmdResult<Page<DupGroupDto>> {
    let dups = state.dups.lock();
    let entry = dups
        .get(&dup_id)
        .ok_or_else(|| AppError::InvalidInput("duplicate results expired; run the search again".into()).to_payload())?;
    let tree = state.tree(entry.scan_id)?;
    let t = tree.read();
    let groups = &entry.report.groups;
    let end = (offset + limit.min(500)).min(groups.len());
    let start = offset.min(end);
    let rows = groups[start..end]
        .iter()
        .map(|g| DupGroupDto {
            id: g.id,
            size: g.size,
            hash: g.hash.clone(),
            wasted: g.wasted,
            files: g.files.iter().filter(|&&f| !t.is_detached(f)).map(|&f| node_row(&t, f, true)).collect(),
        })
        .collect();
    Ok(Page { total: groups.len(), offset: start, rows })
}

/// Suggest a selection (never deletes). Returns node ids to pre-select.
#[tauri::command]
pub fn dup_autoselect(state: State<'_, AppState>, dup_id: u32, rule: AutoSelect, folder: Option<String>) -> CmdResult<Vec<u32>> {
    let dups = state.dups.lock();
    let entry = dups
        .get(&dup_id)
        .ok_or_else(|| AppError::InvalidInput("duplicate results expired".into()).to_payload())?;
    let tree = state.tree(entry.scan_id)?;
    let t = tree.read();
    Ok(entry
        .report
        .groups
        .iter()
        .flat_map(|g| hdcleaner_core::duplicates::auto_select(&t, g, rule, folder.as_deref()))
        .filter(|&id| !t.is_detached(id))
        .collect())
}
