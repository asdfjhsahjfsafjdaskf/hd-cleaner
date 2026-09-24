//! Process manager, startup manager and Target Mode.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::elevation::{run_elevated_ops, ElevatedOp};
use hdcleaner_core::processes::{ProcessRow, TreeKill, VersionInfo};
use hdcleaner_core::programs::Identification;
use hdcleaner_core::startup::{self, StartupItem};
use hdcleaner_core::target::WindowInfo;
use hdcleaner_core::AppError;
use serde::Serialize;
use std::sync::Arc;
use tauri::ipc::Response;
use tauri::{AppHandle, Manager};

fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = CmdResult<T>> {
    async move { tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::Helper(e.to_string()).to_payload()) }
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn processes_sample(app: AppHandle) -> CmdResult<Vec<ProcessRow>> {
    blocking(move || app.state::<AppState>().sampler.lock().sample()).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessDetails {
    pub row: ProcessRow,
    pub version: Option<VersionInfo>,
    /// Installed programs whose folder contains the executable.
    pub programs: Vec<Identification>,
    /// Startup entries that launch this executable.
    pub startup: Vec<StartupItem>,
    pub children: usize,
}

pub(crate) fn disabled_services(state: &AppState) -> Vec<String> {
    startup::disabled_services(&state.db.lock())
}

fn remember_service(state: &AppState, name: &str, disabled: bool) {
    startup::remember_service(&state.db.lock(), name, disabled)
}

#[tauri::command]
pub async fn process_details(app: AppHandle, pid: u32) -> CmdResult<ProcessDetails> {
    blocking(move || {
        let state = app.state::<AppState>();
        let row = hdcleaner_core::processes::process_row(pid).ok_or_else(|| AppError::NotFound { path: format!("pid {pid}") }.to_payload())?;
        let exe = row.path.clone();
        let version = exe.as_deref().map(hdcleaner_core::processes::version_info);
        let programs = match &exe {
            Some(p) => hdcleaner_core::programs::identify(&super::forced::inventory(&state), p),
            None => Vec::new(),
        };
        let startup = match &exe {
            Some(p) => startup::list(&disabled_services(&state)).into_iter().filter(|i| i.exe.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(p))).collect(),
            None => Vec::new(),
        };
        let children = hdcleaner_core::processes::descendants(pid).len();
        Ok(ProcessDetails { row, version, programs, startup, children })
    })
    .await?
}

/// End a process (or its whole tree). `elevated` runs it through the helper
/// (one UAC prompt) for processes of other accounts or elevated ones.
#[tauri::command]
pub async fn process_end(app: AppHandle, pid: u32, path: Option<String>, tree: bool, elevated: bool) -> CmdResult<Vec<TreeKill>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let result: hdcleaner_core::Result<Vec<TreeKill>> = match (elevated, &path) {
            // The helper identifies the process itself when its path is not readable here.
            (true, p) => hdcleaner_core::processes::terminate_elevated(pid, p.as_deref(), tree),
            (false, Some(p)) if tree => hdcleaner_core::processes::terminate_tree(pid, p),
            (false, Some(p)) => hdcleaner_core::processes::terminate(pid, p).map(|_| {
                let name = p.rsplit('\\').next().unwrap_or(p).to_string();
                vec![TreeKill { pid, name, path: Some(p.clone()), ok: true, skipped: false, error: None }]
            }),
            (false, None) => Err(AppError::AccessDenied { path: format!("pid {pid}") }),
        };        let kind = if tree { "end-process-tree" } else { "end-process" };
        let (status, n) = match &result {
            Ok(v) if v.iter().all(|k| k.ok || k.skipped) => ("completed", v.len()),
            Ok(v) => ("partial", v.len()),
            Err(_) => ("failed", 1),
        };
        let _ = state.db.lock().record_operation(
            kind,
            status,
            path.as_deref().unwrap_or(&format!("pid {pid}")),
            n,
            &serde_json::json!({ "pid": pid, "path": path, "elevated": elevated, "result": result.as_ref().ok(), "error": result.as_ref().err().map(|e| e.to_payload()) }),
        );
        result.ui()
    })
    .await?
}

/// Icon of an executable (resource extraction only; the file is not run).
#[tauri::command]
pub async fn file_icon(app: AppHandle, path: String) -> CmdResult<Response> {
    blocking(move || {
        let state = app.state::<AppState>();
        let key = path.to_lowercase();
        if let Some(hit) = state.file_icons.lock().get(&key) {
            return Response::new(hit.as_ref().map(|b| b.as_ref().clone()).unwrap_or_default());
        }
        let png = (key.ends_with(".exe") && std::path::Path::new(&path).is_file())
            .then(|| hdcleaner_core::icons::icon_png(&format!("{path},0"), 32).ok())
            .flatten()
            .map(Arc::new);
        state.file_icons.lock().insert(key, png.clone());
        Response::new(png.map(|b| b.as_ref().clone()).unwrap_or_default())
    })
    .await
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupRow {
    #[serde(flatten)]
    pub item: StartupItem,
    /// Installed program the executable belongs to.
    pub program_id: Option<String>,
    pub program_name: Option<String>,
}

#[tauri::command]
pub async fn startup_list(app: AppHandle) -> CmdResult<Vec<StartupRow>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let programs = super::forced::inventory(&state);
        startup::list(&disabled_services(&state))
            .into_iter()
            .map(|item| {
                let id = item.exe.as_deref().and_then(|e| hdcleaner_core::programs::identify(&programs, e).into_iter().next());
                StartupRow { program_id: id.as_ref().map(|i| i.program_id.clone()), program_name: id.map(|i| i.name), item }
            })
            .collect()
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupImpact {
    /// Most recent boots first.
    pub boots: Vec<hdcleaner_core::bootperf::Boot>,
    /// Startup entry id → delay recorded by Windows.
    pub items: std::collections::HashMap<String, hdcleaner_core::bootperf::Slowdown>,
    /// Services / apps delaying startup that are not startup entries here.
    pub others: Vec<hdcleaner_core::bootperf::Slowdown>,
}

/// Startup delays recorded by Windows (boot performance log). The log needs
/// administrator rights: read directly when elevated, otherwise through the
/// helper. `cached` returns the last read without asking again.
#[tauri::command]
pub async fn startup_impact(app: AppHandle, cached: bool) -> CmdResult<Option<StartupImpact>> {
    blocking(move || {
        let state = app.state::<AppState>();
        // Bind first: a guard inside the match expression would stay locked
        // for the whole match and the store below would deadlock.
        let previous = state.boot_report.lock().clone();
        let report = match previous {
            Some(r) if cached => r,
            _ if cached => return Ok(None),
            _ => {
                let r = Arc::new(hdcleaner_core::bootperf::read_any().ui()?);
                *state.boot_report.lock() = Some(r.clone());
                r
            }
        };
        let list = startup::list(&disabled_services(&state));
        let mut items = std::collections::HashMap::new();
        let mut matched = std::collections::HashSet::new();
        for i in &list {
            let Some(exe) = i.exe.as_deref() else { continue };
            // Every version counted in an entry is left out of "others".
            matched.extend(report.matching(exe, &i.command).into_iter().map(|s| s.path.to_lowercase()));
            if let Some(s) = report.for_startup(exe, &i.command) {
                items.insert(i.id.clone(), s);
            }
        }
        let win = format!("{}\\", hdcleaner_core::system::windows_dir().to_lowercase());
        let others = report
            .slowdowns
            .iter()
            .filter(|s| !s.path.to_lowercase().starts_with(&win) && !matched.contains(&s.path.to_lowercase()))
            .take(15)
            .cloned()
            .collect();
        Ok(Some(StartupImpact { boots: report.boots.iter().take(10).cloned().collect(), items, others }))
    })
    .await?
}

/// Enable or disable an entry. The entry is re-read and must still have the
/// command the user reviewed.
#[tauri::command]
pub async fn startup_set_enabled(app: AppHandle, id: String, command: String, enabled: bool) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        let item = startup::find(&id, &command, &disabled_services(&state)).ui()?;
        let r = match startup::set_enabled(&item, enabled) {
            Ok(None) => Ok(()),
            Ok(Some(op)) => run_elevated_ops(&[ElevatedOp::Startup { op }]).and_then(|v| match v.first() {
                Some(r) if r.ok => Ok(()),
                Some(r) => Err(AppError::Helper(r.message.clone().unwrap_or_default())),
                None => Err(AppError::Helper("no result from helper".into())),
            }),
            Err(e) => Err(e),
        };
        if r.is_ok() && matches!(item.source, startup::Source::Service) {
            remember_service(&state, &item.key, !enabled);
        }
        let _ = state.db.lock().record_operation(
            if enabled { "startup-enable" } else { "startup-disable" },
            if r.is_ok() { "completed" } else { "failed" },
            &item.name,
            1,
            &serde_json::json!({ "id": item.id, "source": item.source, "command": item.command, "error": r.as_ref().err().map(|e| e.to_payload()) }),
        );
        r.ui()
    })
    .await?
}

/// Is HD Cleaner itself set to start with Windows?
#[tauri::command]
pub fn start_with_windows() -> bool {
    startup::self_startup().is_some()
}

/// Add or remove this app's own Run entry (current user only).
#[tauri::command]
pub async fn set_start_with_windows(app: AppHandle, enabled: bool) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        let r = startup::set_self_startup(enabled);
        let _ = state.db.lock().record_operation(
            "startup-self",
            if r.is_ok() { "completed" } else { "failed" },
            if enabled { "on" } else { "off" },
            1,
            &serde_json::json!({ "enabled": enabled, "error": r.as_ref().err().map(|e| e.to_payload()) }),
        );
        r.ui()
    })
    .await?
}

/// Remove an entry (backup first). Returns the backup folder.
#[tauri::command]
pub async fn startup_remove(app: AppHandle, id: String, command: String) -> CmdResult<String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let item = startup::find(&id, &command, &disabled_services(&state)).ui()?;
        let name: String = item.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).take(40).collect();
        let backup = state.data_dir.join("backups").join(format!("{}-startup-{name}", hdcleaner_core::util::now_unix_ms()));
        let r = startup::remove(&item, &backup).and_then(|pending| match pending {
            None => Ok(()),
            Some(op) => {
                // Removal + approval cleanup in one helper run (one UAC prompt).
                let mut ops = vec![ElevatedOp::Removal { item: op }];
                ops.extend(startup::forget_approval_op(&item).map(|op| ElevatedOp::Startup { op }));
                run_elevated_ops(&ops)
            }
            .and_then(|v| match v.first() {
                Some(r) if r.ok => Ok(()),
                Some(r) => Err(AppError::Helper(r.message.clone().unwrap_or_default())),
                None => Err(AppError::Helper("no result from helper".into())),
            }),
        });
        let _ = state.db.lock().record_operation(
            "startup-remove",
            if r.is_ok() { "completed" } else { "failed" },
            &item.name,
            1,
            &serde_json::json!({ "id": item.id, "source": item.source, "command": item.command, "backupDir": backup, "error": r.as_ref().err().map(|e| e.to_payload()) }),
        );
        r.map(|_| backup.to_string_lossy().to_string()).ui()
    })
    .await?
}

// ---------------------------------------------------------------------------
// Target Mode
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetPicked {
    pub window: WindowInfo,
    pub process: Option<ProcessRow>,
}

/// Minimise the app, let the user click a window, restore the app.
#[tauri::command]
pub async fn target_pick(app: AppHandle, hint: String) -> CmdResult<Option<TargetPicked>> {
    let hint: String = hint.chars().take(200).collect();
    let win = app.get_webview_window("main");
    if let Some(w) = &win {
        let _ = w.minimize();
    }
    // Let the minimise animation finish so the app is not what gets picked.
    tokio_sleep(350).await;
    let r = blocking(move || hdcleaner_core::target::pick(&hint)).await;
    if let Some(w) = &win {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    let picked = r?.ui()?;
    Ok(picked.map(|window| TargetPicked { process: hdcleaner_core::processes::process_row(window.pid), window }))
}

async fn tokio_sleep(ms: u64) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(std::time::Duration::from_millis(ms))).await;
}
