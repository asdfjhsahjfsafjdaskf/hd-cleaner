//! Cleaner: analysis (kept in memory), item pages and cleaning of the
//! categories the user selected from that analysis.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::cleaner::{self, Category, CategoryResult, CleanItem, CleanOutcome};
use hdcleaner_core::elevation::{run_elevated_ops, ElevatedOp};
use hdcleaner_core::{ErrorPayload, AppError};
use serde::Serialize;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySummary {
    pub category: Category,
    pub count: u64,
    pub bytes: u64,
    pub running: bool,
    pub recent_skipped: u64,
    pub admin_pending: bool,
    pub truncated: bool,
    pub error: Option<ErrorPayload>,
}

fn summary(r: &CategoryResult) -> CategorySummary {
    CategorySummary {
        category: r.category.clone(),
        count: r.count,
        bytes: r.bytes,
        running: r.running,
        recent_skipped: r.recent_skipped,
        admin_pending: r.admin_pending,
        truncated: r.truncated,
        error: r.error.clone(),
    }
}

/// List the machine-wide categories through the elevated helper: a standard
/// user cannot even read `C:\Windows\Temp`. One UAC prompt, read-only.
#[tauri::command]
pub async fn cleaner_analyze_admin(app: AppHandle) -> CmdResult<Vec<CategorySummary>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let current = state.clean_analysis.lock().clone().ok_or_else(|| AppError::InvalidInput("analyze first".into()).to_payload())?;
        let ids: Vec<String> = current.iter().filter(|r| r.admin_pending).map(|r| r.category.id.clone()).collect();
        if ids.is_empty() {
            return Ok(current.iter().map(summary).collect());
        }
        let r = run_elevated_ops(&[ElevatedOp::AnalyzeClean { categories: ids }]).ui()?;
        let first = r.into_iter().next().ok_or_else(|| AppError::Helper("no result from helper".into()).to_payload())?;
        if !first.ok {
            return Err(AppError::Helper(first.message.unwrap_or_default()).to_payload());
        }
        let listed: Vec<hdcleaner_core::cleaner::AdminAnalysis> =
            first.data.and_then(|d| serde_json::from_value(d).ok()).ok_or_else(|| AppError::Corrupt("invalid analysis from helper".into()).to_payload())?;
        let mut merged: Vec<CategoryResult> = current.as_ref().clone();
        for a in listed {
            if let Some(r) = merged.iter_mut().find(|r| r.category.id == a.id) {
                r.count = a.items.len() as u64;
                r.bytes = a.items.iter().map(|i| i.size).sum();
                r.items = a.items;
                r.recent_skipped = a.recent_skipped;
                r.admin_pending = false;
            }
        }
        let out: Vec<CategorySummary> = merged.iter().map(summary).collect();
        *state.clean_analysis.lock() = Some(Arc::new(merged));
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

/// Analyze every category. Nothing is removed.
#[tauri::command]
pub async fn cleaner_analyze(app: AppHandle) -> CmdResult<Vec<CategorySummary>> {
    let results = tauri::async_runtime::spawn_blocking(cleaner::analyze).await.map_err(|e| AppError::Helper(e.to_string()).to_payload())?;
    let out = results.iter().map(summary).collect();
    *app.state::<AppState>().clean_analysis.lock() = Some(Arc::new(results));
    Ok(out)
}

/// Ask the browser/app of a category to close, so its data can be cleaned.
/// Nothing is forced: processes still running afterwards are returned.
#[tauri::command]
pub async fn cleaner_close_program(id: String) -> CmdResult<Vec<hdcleaner_core::processes::ProcInfo>> {
    tauri::async_runtime::spawn_blocking(move || cleaner::close_program(&id, std::time::Duration::from_secs(8)).ui())
        .await
        .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

/// Items of one analysed category (what exactly would be removed).
#[tauri::command]
pub fn cleaner_items(app: AppHandle, id: String, offset: usize, limit: usize) -> CmdResult<Page<CleanItem>> {
    let state = app.state::<AppState>();
    let a = state.clean_analysis.lock().clone().ok_or_else(|| AppError::InvalidInput("analyze first".into()).to_payload())?;
    let r = a.iter().find(|r| r.category.id == id).ok_or_else(|| AppError::NotFound { path: id.clone() }.to_payload())?;
    let limit = limit.min(2000);
    Ok(Page { total: r.items.len(), offset, rows: r.items.iter().skip(offset).take(limit).cloned().collect() })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "event", content = "data")]
pub enum CleanEvent {
    Category { id: String, index: usize, total: usize },
    Progress { id: String, done: u64, total: u64 },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryOutcome {
    pub id: String,
    pub outcome: Option<CleanOutcome>,
    pub error: Option<ErrorPayload>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub results: Vec<CategoryOutcome>,
    pub removed: u64,
    pub freed: u64,
    pub dry_run: bool,
    pub backup_dir: Option<String>,
}

/// Clean the selected categories of the last analysis. Machine-wide
/// categories go through the elevated helper together (one UAC prompt).
#[tauri::command]
pub async fn cleaner_run(app: AppHandle, ids: Vec<String>, dry_run: bool, on_event: Channel<CleanEvent>) -> CmdResult<CleanReport> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let a = state.clean_analysis.lock().clone().ok_or_else(|| AppError::InvalidInput("analyze first".into()).to_payload())?;
        let chosen: Vec<&CategoryResult> = ids.iter().filter_map(|id| a.iter().find(|r| &r.category.id == id)).collect();
        if chosen.is_empty() {
            return Err(AppError::InvalidInput("no category selected".into()).to_payload());
        }
        let backup = state.data_dir.join("backups").join(format!("{}-cleanup", hdcleaner_core::util::now_unix_ms()));
        let op = if dry_run {
            None
        } else {
            state
                .db
                .lock()
                .begin_operation("cleanup", &format!("{} categories", chosen.len()), chosen.len(), false, &serde_json::json!({ "categories": ids }))
                .ok()
        };
        let mut results = Vec::new();
        let mut elevated: Vec<(String, ElevatedOp)> = Vec::new();
        let total = chosen.len();
        for (index, r) in chosen.iter().enumerate() {
            let id = r.category.id.clone();
            let _ = on_event.send(CleanEvent::Category { id: id.clone(), index, total });
            if r.category.admin && !dry_run && !hdcleaner_core::system::is_elevated() {
                elevated.push((id, ElevatedOp::Clean { category: r.category.id.clone(), items: r.items.clone() }));
                continue;
            }
            let count = r.count;
            let res = cleaner::clean_category(&id, &r.items, &backup, dry_run, &mut |done| {
                let _ = on_event.send(CleanEvent::Progress { id: id.clone(), done, total: count });
            });
            results.push(match res {
                Ok(o) => CategoryOutcome { id, outcome: Some(o), error: None },
                Err(e) => CategoryOutcome { id, outcome: None, error: Some(e.to_payload()) },
            });
        }
        if !elevated.is_empty() {
            let ops: Vec<ElevatedOp> = elevated.iter().map(|(_, op)| op.clone()).collect();
            match run_elevated_ops(&ops) {
                Ok(rs) => {
                    for ((id, _), r) in elevated.iter().zip(rs) {
                        let o = r.data.and_then(|d| serde_json::from_value::<CleanOutcome>(d).ok());
                        results.push(match (r.ok, o) {
                            (true, Some(o)) => CategoryOutcome { id: id.clone(), outcome: Some(o), error: None },
                            _ => CategoryOutcome { id: id.clone(), outcome: None, error: Some(AppError::Helper(r.message.unwrap_or_default()).to_payload()) },
                        });
                    }
                }
                Err(e) => {
                    for (id, _) in &elevated {
                        results.push(CategoryOutcome { id: id.clone(), outcome: None, error: Some(e.to_payload()) });
                    }
                }
            }
        }
        let removed = results.iter().filter_map(|r| r.outcome.as_ref()).map(|o| o.removed).sum();
        let freed = results.iter().filter_map(|r| r.outcome.as_ref()).map(|o| o.freed).sum();
        let backup_dir = backup.exists().then(|| backup.to_string_lossy().to_string());
        if let Some(op) = op {
            let failed = results.iter().filter(|r| r.error.is_some()).count();
            let status = if failed == 0 { "completed" } else if failed < results.len() { "partial" } else { "failed" };
            let _ = state.db.lock().finish_operation(
                op,
                status,
                results.len() - failed,
                failed,
                &serde_json::json!({ "categories": ids, "removed": removed, "freed": freed, "backupDir": backup_dir, "results": results }),
            );
            // The analysis is stale after cleaning.
            *state.clean_analysis.lock() = None;
        }
        Ok(CleanReport { results, removed, freed, dry_run, backup_dir })
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}
