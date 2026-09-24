//! Windows Apps (Microsoft Store packages): list, repair and remove.
//!
//! Packages Windows itself needs are listed but never removed — the refusal
//! also lives in the elevated helper, so "remove for everyone" cannot get
//! around it either.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::appx;
use hdcleaner_core::elevation::{run_elevated_ops, ElevatedOp};
use hdcleaner_core::programs::Program;
use hdcleaner_core::AppError;
use serde::Serialize;
use tauri::{AppHandle, Manager};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsApp {
    #[serde(flatten)]
    pub program: Program,
    /// Windows needs it: removal is refused.
    pub critical: bool,
    /// Size of the install folder, when it could be measured.
    pub bytes: Option<u64>,
}

#[tauri::command]
pub async fn windows_apps_list(measure: bool) -> CmdResult<Vec<WindowsApp>> {
    tauri::async_runtime::spawn_blocking(move || {
        let ctl = hdcleaner_core::scan::ScanControl::new();
        let mut out: Vec<WindowsApp> = appx::packages()
            .ui()?
            .into_iter()
            .map(|program| {
                let bytes = measure
                    .then(|| program.install_location.as_deref().and_then(|p| hdcleaner_core::appsize::measure_live(p, &ctl)).map(|m| m.total))
                    .flatten();
                WindowsApp { critical: appx::is_critical(&program), bytes, program }
            })
            .collect();
        out.sort_by(|a, b| b.bytes.unwrap_or(0).cmp(&a.bytes.unwrap_or(0)).then(a.program.name.cmp(&b.program.name)));
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

/// Re-register the package for the current user: the same repair Windows
/// does for an app that stopped opening. App data is kept.
#[tauri::command]
pub async fn windows_app_repair(app: AppHandle, full_name: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let r = appx::repair(&full_name);
        let _ = state.db.lock().record_operation(
            "appx-repair",
            if r.is_ok() { "completed" } else { "failed" },
            &full_name,
            1,
            &serde_json::json!({ "package": full_name, "error": r.as_ref().err().map(|e| e.to_payload()) }),
        );
        r.ui()
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

#[tauri::command]
pub async fn windows_app_remove(app: AppHandle, full_name: String, all_users: bool) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let package = appx::packages()
            .ui()?
            .into_iter()
            .find(|p| p.package_full_name.as_deref() == Some(full_name.as_str()))
            .ok_or_else(|| AppError::NotFound { path: full_name.clone() }.to_payload())?;
        if appx::is_critical(&package) {
            return Err(AppError::Protected { path: package.name.clone(), reason: "windowsComponent".into() }.to_payload());
        }
        let r = if all_users && !hdcleaner_core::system::is_elevated() {
            run_elevated_ops(&[ElevatedOp::RemovePackageAllUsers { full_name: full_name.clone() }]).and_then(|v| match v.first() {
                Some(r) if r.ok => Ok(()),
                Some(r) => Err(AppError::Helper(r.message.clone().unwrap_or_default())),
                None => Err(AppError::Helper("no result from helper".into())),
            })
        } else {
            appx::remove(&full_name, all_users)
        };
        let _ = state.db.lock().record_operation(
            "appx-remove",
            if r.is_ok() { "completed" } else { "failed" },
            &package.name,
            1,
            &serde_json::json!({ "package": full_name, "allUsers": all_users, "error": r.as_ref().err().map(|e| e.to_payload()) }),
        );
        r.ui()
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

// ---- browser extensions -----------------------------------------------------

#[tauri::command]
pub async fn extensions_list() -> CmdResult<Vec<hdcleaner_core::extensions::Extension>> {
    tauri::async_runtime::spawn_blocking(hdcleaner_core::extensions::list)
        .await
        .map_err(|e| AppError::Helper(e.to_string()).to_payload())
}

/// Remove one extension (Recycle Bin by default). Refused while the browser
/// is open, and the item must still be the one that was listed.
#[tauri::command]
pub async fn extension_remove(app: AppHandle, id: String, browser: String, recycle: bool) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let ext = hdcleaner_core::extensions::list()
            .into_iter()
            .find(|e| e.id == id && e.browser == browser)
            .ok_or_else(|| AppError::NotFound { path: id.clone() }.to_payload())?;
        let r = hdcleaner_core::extensions::remove(&ext, recycle);
        let _ = state.db.lock().record_operation(
            "extension-remove",
            if r.is_ok() { "completed" } else { "failed" },
            &format!("{} · {}", ext.browser_name, ext.name),
            1,
            &serde_json::json!({ "id": ext.id, "browser": ext.browser, "path": ext.path, "error": r.as_ref().err().map(|e| e.to_payload()) }),
        );
        r.ui()
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}
