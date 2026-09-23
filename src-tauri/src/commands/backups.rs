//! Backups page: what was saved before a removal, and putting it back.
//!
//! Restoring never overwrites a file that exists again, and registry values
//! go back through the same allowlist that guards deletion.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::backups::{self, BackupSet, Manifest, RestoreResult};
use hdcleaner_core::AppError;
use tauri::{AppHandle, Manager};

fn root(state: &AppState) -> std::path::PathBuf {
    state.data_dir.join("backups")
}

#[tauri::command]
pub async fn backups_list(app: AppHandle) -> CmdResult<Vec<BackupSet>> {
    tauri::async_runtime::spawn_blocking(move || backups::list(&root(&app.state::<AppState>())))
        .await
        .map_err(|e| AppError::Helper(e.to_string()).to_payload())
}

/// Everything one backup holds (what it would put back).
#[tauri::command]
pub async fn backup_read(app: AppHandle, id: String) -> CmdResult<Manifest> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = backups::dir_of(&root(&app.state::<AppState>()), &id).ui()?;
        backups::read(&dir).ui()
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

#[tauri::command]
pub async fn backup_restore(app: AppHandle, id: String, indexes: Vec<usize>) -> CmdResult<Vec<RestoreResult>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let dir = backups::dir_of(&root(&state), &id).ui()?;
        let results = backups::restore(&dir, &indexes).ui()?;
        let ok = results.iter().filter(|r| r.status == "restored" || r.status == "partial").count();
        let status = if ok == results.len() {
            "completed"
        } else if ok == 0 {
            "failed"
        } else {
            "partial"
        };
        let label = backups::read(&dir).map(|m| m.label).unwrap_or_else(|_| id.clone());
        let _ = state.db.lock().record_operation(
            "restore",
            status,
            &label,
            results.len(),
            &serde_json::json!({ "backup": id, "restored": ok, "results": results }),
        );
        Ok(results)
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

#[tauri::command]
pub async fn backup_delete(app: AppHandle, id: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let label = backups::dir_of(&root(&state), &id).and_then(|d| backups::read(&d)).map(|m| m.label).unwrap_or_else(|_| id.clone());
        backups::delete(&root(&state), &id).ui()?;
        let _ = state.db.lock().record_operation("backup-delete", "completed", &label, 1, &serde_json::json!({ "backup": id }));
        Ok(())
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

/// Create a System Restore point on demand (one UAC prompt unless elevated).
/// Used before a large cleanup; Windows itself may refuse when System
/// Protection is off or one was created minutes ago.
#[tauri::command]
pub async fn create_restore_point(app: AppHandle, description: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let desc = description.chars().take(120).collect::<String>();
        let r = if hdcleaner_core::system::is_elevated() {
            hdcleaner_core::uninstall::create_restore_point(&desc)
        } else {
            hdcleaner_core::elevation::run_elevated_ops(&[hdcleaner_core::elevation::ElevatedOp::CreateRestorePoint { description: desc.clone() }]).and_then(|v| {
                match v.first() {
                    Some(r) if r.ok => Ok(()),
                    Some(r) => Err(AppError::Helper(r.message.clone().unwrap_or_default())),
                    None => Err(AppError::Helper("no result from helper".into())),
                }
            })
        };
        let _ = state.db.lock().record_operation(
            "restore-point",
            if r.is_ok() { "completed" } else { "failed" },
            &desc,
            1,
            &serde_json::json!({ "error": r.as_ref().err().map(|e| e.to_payload()) }),
        );
        r.ui()
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}
