//! Application-level commands: info, drives, settings, history, tools.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::db::OperationRecord;
use hdcleaner_core::AppError;
use serde::Serialize;
use tauri::{AppHandle, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub os: hdcleaner_core::system::OsInfo,
    pub data_dir: String,
    pub interrupted: Vec<OperationRecord>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        name: hdcleaner_core::branding::APP_NAME,
        version: env!("CARGO_PKG_VERSION"),
        os: hdcleaner_core::system::os_info(),
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        interrupted: state.interrupted.lock().clone(),
    }
}

#[tauri::command]
pub async fn list_drives() -> Vec<hdcleaner_core::disk::DriveInfo> {
    hdcleaner_core::disk::list_drives()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<serde_json::Map<String, serde_json::Value>> {
    state.db.lock().all_settings().ui()
}

const SETTING_KEYS: &[&str] = &[
    "theme",
    "language",
    "fontScale",
    "scanMethod",
    "preferFastScan",
    "followJunctions",
    "showHidden",
    "showSystem",
    "treemapMetric",
    "treemapMaxRects",
    "saveSnapshots",
    "useRecycleBin",
    "defaultLeftoverLevel",
    "createRegistryBackup",
    "createRestorePoint",
    "cleanerDefaults",
    "experimental",
    "debugLogs",
    "dryRunDefault",
];

#[tauri::command]
pub fn set_setting(state: State<'_, AppState>, key: String, value: serde_json::Value) -> CmdResult<()> {
    if !SETTING_KEYS.contains(&key.as_str()) {
        return Err(AppError::InvalidInput(format!("unknown setting: {key}")).to_payload());
    }
    if value.to_string().len() > 64 * 1024 {
        return Err(AppError::InvalidInput("setting value too large".into()).to_payload());
    }
    state.db.lock().set_setting(&key, &value).ui()
}

#[tauri::command]
pub fn list_history(state: State<'_, AppState>, limit: Option<usize>) -> CmdResult<Vec<OperationRecord>> {
    state.db.lock().list_operations(limit.unwrap_or(500).min(10_000)).ui()
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> CmdResult<usize> {
    state.db.lock().clear_history().ui()
}

#[tauri::command]
pub async fn export_history(state: State<'_, AppState>, path: String) -> CmdResult<usize> {
    let ops = state.db.lock().list_operations(100_000).ui()?;
    let json = serde_json::to_vec_pretty(&ops).map_err(|e| AppError::Corrupt(e.to_string()).to_payload())?;
    std::fs::write(&path, json).map_err(|e| AppError::io("writing history", Some(std::path::Path::new(&path)), e).to_payload())?;
    Ok(ops.len())
}

/// The user reviewed an interrupted operation. `discard` marks it handled;
/// `resume` returns the paths it was working on so the UI can build a *new*
/// plan (re-verified from scratch — nothing is replayed blindly).
#[tauri::command]
pub fn resolve_interrupted(state: State<'_, AppState>, id: i64, action: String) -> CmdResult<Vec<String>> {
    let mut list = state.interrupted.lock();
    let Some(pos) = list.iter().position(|o| o.id == id) else {
        return Err(AppError::InvalidInput("unknown interrupted operation".into()).to_payload());
    };
    let op = list.remove(pos);
    let db = state.db.lock();
    match action.as_str() {
        "discard" => {
            db.set_operation_status(id, "interrupted").ui()?;
            Ok(vec![])
        }
        "resume" => {
            db.set_operation_status(id, "interrupted").ui()?;
            Ok(op.details.get("paths").and_then(|p| p.as_array()).map(|a| {
                a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
            }).unwrap_or_default())
        }
        _ => Err(AppError::InvalidInput("action must be resume or discard".into()).to_payload()),
    }
}

#[tauri::command]
pub fn list_tools() -> Vec<hdcleaner_core::tools::ToolInfo> {
    hdcleaner_core::tools::list()
}

#[tauri::command]
pub fn launch_tool(id: String) -> CmdResult<()> {
    hdcleaner_core::tools::launch(&id).ui()
}

#[tauri::command]
pub fn restart_elevated(app: AppHandle) -> CmdResult<()> {
    hdcleaner_core::tools::restart_elevated().ui()?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn protection_assess(path: String) -> hdcleaner_core::protection::Assessment {
    hdcleaner_core::protection::assess(&path)
}
