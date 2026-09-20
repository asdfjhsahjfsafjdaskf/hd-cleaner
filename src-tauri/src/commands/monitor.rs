//! Installation monitor: snapshot before, watch during, compare after.
//! Saved traces are later used by the uninstall wizard.

use crate::dto::*;
use crate::state::{AppState, MonitorSession};
use hdcleaner_core::monitor::{self, InstallTrace};
use hdcleaner_core::AppError;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorStatus {
    pub running: bool,
    pub started_ms: i64,
    /// Folders being watched.
    pub roots: Vec<String>,
    /// Files and folders recorded in the snapshot taken before.
    pub baseline: usize,
    /// Changes seen live so far.
    pub seen: usize,
}

fn status_of(session: Option<&MonitorSession>) -> MonitorStatus {
    match session {
        Some(s) => MonitorStatus {
            running: true,
            started_ms: s.before.taken_ms,
            roots: s.before.roots.clone(),
            baseline: s.before.files.len(),
            seen: s.watcher.count(),
        },
        None => MonitorStatus { running: false, started_ms: 0, roots: Vec::new(), baseline: 0, seen: 0 },
    }
}

/// Take the "before" snapshot and start watching. Read-only.
#[tauri::command]
pub async fn monitor_start(app: AppHandle) -> CmdResult<MonitorStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if state.monitor.lock().is_some() {
            return Err(AppError::InvalidInput("a monitoring session is already running".into()).to_payload());
        }
        let roots = monitor::default_roots();
        let before = monitor::snapshot(&roots);
        let watcher = monitor::watch(&roots).ui()?;
        let session = MonitorSession { before, watcher };
        let status = status_of(Some(&session));
        *state.monitor.lock() = Some(session);
        Ok(status)
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

#[tauri::command]
pub fn monitor_status(app: AppHandle) -> MonitorStatus {
    status_of(app.state::<AppState>().monitor.lock().as_ref())
}

/// Stop watching and forget everything (no trace is saved).
#[tauri::command]
pub fn monitor_cancel(app: AppHandle) -> MonitorStatus {
    if let Some(s) = app.state::<AppState>().monitor.lock().take() {
        s.watcher.stop();
    }
    MonitorStatus { running: false, started_ms: 0, roots: Vec::new(), baseline: 0, seen: 0 }
}

/// Take the "after" snapshot, compare, and save the trace.
#[tauri::command]
pub async fn monitor_finish(app: AppHandle, name: String) -> CmdResult<InstallTrace> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let session = state.monitor.lock().take().ok_or_else(|| AppError::InvalidInput("no monitoring session is running".into()).to_payload())?;
        let watched = session.watcher.stop();
        let roots: Vec<PathBuf> = session.before.roots.iter().map(PathBuf::from).collect();
        let after = monitor::snapshot(&roots);
        let name = name.trim().chars().take(120).collect::<String>();
        let mut trace = monitor::diff(if name.is_empty() { "Instalação" } else { &name }, &session.before, &after, &watched);
        // The program list is refreshed: a new program may have appeared.
        *state.programs.write() = None;
        let db = state.db.lock();
        trace.id = db.add_trace(&trace).ui()?;
        let _ = db.record_operation(
            "install-trace",
            "completed",
            &trace.name,
            trace.files.len(),
            &serde_json::json!({ "traceId": trace.id, "program": trace.program_name, "files": trace.files.len(), "registry": trace.registry.len(), "bytes": trace.bytes }),
        );
        Ok(trace)
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

/// Saved traces, newest first (without their item lists).
#[tauri::command]
pub fn traces_list(app: AppHandle) -> CmdResult<Vec<InstallTrace>> {
    app.state::<AppState>().db.lock().list_traces(200).ui()
}

#[tauri::command]
pub fn trace_get(app: AppHandle, id: i64) -> CmdResult<InstallTrace> {
    app.state::<AppState>().db.lock().trace(id).ui()
}

#[tauri::command]
pub fn trace_delete(app: AppHandle, id: i64) -> CmdResult<bool> {
    app.state::<AppState>().db.lock().delete_trace(id).ui()
}

/// Write a trace as JSON so it can be kept or moved to another machine.
#[tauri::command]
pub fn trace_export(app: AppHandle, id: i64, path: String) -> CmdResult<usize> {
    let trace = app.state::<AppState>().db.lock().trace(id).ui()?;
    let json = serde_json::to_vec_pretty(&trace).map_err(|e| AppError::Corrupt(e.to_string()).to_payload())?;
    std::fs::write(&path, &json).map_err(|e| AppError::io("writing the trace", Some(std::path::Path::new(&path)), e).to_payload())?;
    Ok(json.len())
}

/// Read a trace exported before (from this or another machine).
#[tauri::command]
pub fn trace_import(app: AppHandle, path: String) -> CmdResult<InstallTrace> {
    let data = std::fs::read(&path).map_err(|e| AppError::io("reading the trace", Some(std::path::Path::new(&path)), e).to_payload())?;
    if data.len() > 64 * 1024 * 1024 {
        return Err(AppError::InvalidInput("trace file too large".into()).to_payload());
    }
    let mut trace: InstallTrace = serde_json::from_slice(&data).map_err(|e| AppError::Corrupt(format!("invalid trace file: {e}")).to_payload())?;
    trace.name = format!("{} (importado)", trace.name.trim());
    let state = app.state::<AppState>();
    let id = state.db.lock().add_trace(&trace).ui()?;
    trace.id = id;
    Ok(trace)
}
