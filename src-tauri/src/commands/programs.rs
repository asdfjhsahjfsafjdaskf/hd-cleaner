//! Installed programs: inventory, icons, true size, identification and the
//! disk-map integration.

use crate::dto::*;
use crate::state::AppState;
use hdcleaner_core::appsize::{self, AppSize, Measured, Roots};
use hdcleaner_core::programs::{Identification, Program};
use hdcleaner_core::scan::{NodeId, ScanControl};
use hdcleaner_core::{ErrorPayload, AppError};
use serde::Serialize;
use std::sync::Arc;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Manager, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramList {
    pub programs: Arc<Vec<Program>>,
    /// AppX enumeration failure (registry programs are still listed).
    pub appx_error: Option<ErrorPayload>,
}

/// The cached program list, scanning once when nothing is cached yet (other
/// screens — the install monitor, Target Mode — can start an uninstall before
/// the Programs page was ever opened).
pub(crate) fn load(state: &AppState, refresh: bool) -> (Arc<Vec<Program>>, Option<ErrorPayload>) {
    if !refresh {
        if let Some(p) = state.programs.read().clone() {
            return (p, None);
        }
    }
    let (list, err) = hdcleaner_core::programs::list_all();
    let list = Arc::new(list);
    *state.programs.write() = Some(list.clone());
    if refresh {
        state.program_sizes.lock().clear();
        state.icon_cache.lock().clear();
    }
    (list, err.map(|e| e.to_payload()))
}

fn find(state: &AppState, id: &str) -> Result<Program, ErrorPayload> {
    let (list, _) = load(state, false);
    list.iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| AppError::NotFound { path: id.to_string() }.to_payload())
}

#[tauri::command]
pub async fn list_programs(state: State<'_, AppState>, refresh: bool) -> CmdResult<ProgramList> {
    let (programs, appx_error) = load(&state, refresh);
    Ok(ProgramList { programs, appx_error })
}

#[tauri::command]
pub async fn program_icon(state: State<'_, AppState>, id: String) -> CmdResult<Response> {
    if let Some(hit) = state.icon_cache.lock().get(&id) {
        return Ok(Response::new(hit.as_ref().map(|b| b.as_ref().clone()).unwrap_or_default()));
    }
    let prog = find(&state, &id)?;
    let png = hdcleaner_core::icons::program_icon_png(&prog, 32).map(Arc::new);
    state.icon_cache.lock().insert(id, png.clone());
    // Empty body = no icon (the UI shows a generic one).
    Ok(Response::new(png.map(|b| b.as_ref().clone()).unwrap_or_default()))
}

/// Measure a folder from a loaded scan when one covers it (instant),
/// otherwise scan it now.
fn measure(state: &AppState, path: &str) -> Option<(Measured, &'static str)> {
    for tree in state.scans.read().values() {
        let t = tree.read();
        if let Some(id) = t.find_path(path) {
            if t.node(id).is_dir() && !t.is_detached(id) {
                return Some((appsize::measure_tree(&t, id), "scan"));
            }
        }
    }
    if !std::path::Path::new(path).is_dir() {
        return None;
    }
    appsize::measure_live(path, &ScanControl::new()).map(|m| (m, "live"))
}

#[tauri::command]
pub async fn program_size(state: State<'_, AppState>, id: String, refresh: Option<bool>) -> CmdResult<AppSize> {
    if !refresh.unwrap_or(false) {
        if let Some(s) = state.program_sizes.lock().get(&id) {
            return Ok(s.clone());
        }
    }
    let prog = find(&state, &id)?;
    let roots = Roots::load();
    let size = appsize::compute(&prog, &roots, |p| measure(&state, p));
    state.program_sizes.lock().insert(id, size.clone());
    Ok(size)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "event", content = "data")]
pub enum SizesEvent {
    Progress { done: usize, total: usize, program_id: String, total_size: u64 },
    Done { measured: usize },
    Cancelled,
}

/// Compute true sizes for many programs in the background (cancellable).
#[tauri::command]
pub fn program_sizes_all(app: AppHandle, state: State<'_, AppState>, ids: Vec<String>, on_event: Channel<SizesEvent>) -> CmdResult<u32> {
    let job_id = state.next_id();
    let ctl = Arc::new(ScanControl::new());
    state.jobs.lock().insert(job_id, ctl.clone());
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let roots = Roots::load();
        let (list, _) = load(&state, false);
        let total = ids.len();
        let mut measured = 0;
        let mut cancelled = false;
        for (i, id) in ids.iter().enumerate() {
            if ctl.is_cancelled() {
                cancelled = true;
                break;
            }
            let Some(prog) = list.iter().find(|p| &p.id == id) else { continue };
            // Bind first: a guard in the match expression would still be held
            // in the `None` arm, which locks the same mutex (deadlock).
            let cached = state.program_sizes.lock().get(id).cloned();
            let size = match cached {
                Some(s) => s,
                None => {
                    let s = appsize::compute(prog, &roots, |p| measure(&state, p));
                    state.program_sizes.lock().insert(id.clone(), s.clone());
                    s
                }
            };
            measured += 1;
            let _ = on_event.send(SizesEvent::Progress { done: i + 1, total, program_id: id.clone(), total_size: size.total });
        }
        state.jobs.lock().remove(&job_id);
        let _ = on_event.send(if cancelled { SizesEvent::Cancelled } else { SizesEvent::Done { measured } });
    });
    Ok(job_id)
}

/// Sizes already computed (so the list can show them without recomputing).
#[tauri::command]
pub fn program_sizes_cached(state: State<'_, AppState>) -> Vec<(String, u64, u64)> {
    state.program_sizes.lock().iter().map(|(k, v)| (k.clone(), v.total, v.possible_total)).collect()
}

/// "Identify installed program" for a file or folder.
#[tauri::command]
pub async fn identify_program(state: State<'_, AppState>, path: String) -> CmdResult<Vec<Identification>> {
    let (list, _) = load(&state, false);
    Ok(hdcleaner_core::programs::identify(&list, &path))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapTargets {
    pub nodes: Vec<NodeId>,
    pub missing: Vec<String>,
}

/// Nodes of a loaded scan that belong to the program (App Storage Map).
/// Games the installed launchers report, from their own files. `measure`
/// walks each folder (slow), otherwise the launcher's own number is used.
#[tauri::command]
pub async fn games_list(measure: bool) -> CmdResult<Vec<hdcleaner_core::smartstorage::Game>> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut games = hdcleaner_core::smartstorage::games();
        if measure {
            hdcleaner_core::smartstorage::measure(&mut games, &|| false);
        }
        games
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())
}

/// Everything known about one program: what is running, what it starts at
/// logon, its registry keys and the caches it leaves behind. Read-only.
#[tauri::command]
pub async fn app_analysis(app: AppHandle, id: String) -> CmdResult<hdcleaner_core::appanalysis::AppAnalysis> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (list, _) = load(&state, false);
        let program = list.iter().find(|p| p.id == id).cloned().ok_or_else(|| AppError::NotFound { path: id.clone() }.to_payload())?;
        let others: Vec<Program> = list.iter().filter(|p| p.id != id).cloned().collect();
        let procs = hdcleaner_core::processes::list();
        let startup = hdcleaner_core::startup::list(&crate::commands::system::disabled_services(&state));
        let caches = hdcleaner_core::appanalysis::cache_analysis(&program);
        Ok(hdcleaner_core::appanalysis::analyze(&program, &others, &procs, &startup, &caches))
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

/// Clean one of the caches listed by [`app_analysis`]. The items are analyzed
/// again right before removal, so only what is still there is touched.
#[tauri::command]
pub async fn app_clean_cache(app: AppHandle, id: String, category: String, dry_run: bool) -> CmdResult<hdcleaner_core::cleaner::CleanOutcome> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (list, _) = load(&state, false);
        let program = list.iter().find(|p| p.id == id).cloned().ok_or_else(|| AppError::NotFound { path: id.clone() }.to_payload())?;
        let analysis = hdcleaner_core::appanalysis::cache_analysis(&program);
        let r = analysis
            .iter()
            .find(|r| r.category.id == category)
            .ok_or_else(|| AppError::InvalidInput(format!("{category} is not a cache of this program")).to_payload())?;
        if r.running {
            return Err(AppError::InvalidInput(format!("{} is open", r.category.owner.clone().unwrap_or_else(|| program.name.clone()))).to_payload());
        }
        let backup = state
            .data_dir
            .join("backups")
            .join(format!("{}-cleanup", hdcleaner_core::util::now_unix_ms()));
        let outcome = hdcleaner_core::cleaner::clean_category(&r.category.id, &r.items, &backup, dry_run, &mut |_| {}).ui()?;
        if !dry_run {
            let _ = state.db.lock().record_operation(
                "cleanup",
                if outcome.failed == 0 { "completed" } else { "partial" },
                &format!("{} · {}", program.name, r.category.id),
                outcome.removed as usize,
                &serde_json::json!({ "program": program.id, "category": r.category.id, "freed": outcome.freed }),
            );
        }
        Ok(outcome)
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
}

#[tauri::command]
pub async fn program_map_nodes(state: State<'_, AppState>, scan_id: u32, id: String) -> CmdResult<MapTargets> {
    let cached = state.program_sizes.lock().get(&id).cloned();
    let size = match cached {
        Some(s) => s,
        None => {
            let prog = find(&state, &id)?;
            appsize::compute(&prog, &Roots::load(), |p| measure(&state, p))
        }
    };
    let tree = state.tree(scan_id)?;
    let t = tree.read();
    let mut out = MapTargets { nodes: Vec::new(), missing: Vec::new() };
    for loc in &size.locations {
        match t.find_path(&loc.candidate.path) {
            Some(n) if !t.is_detached(n) => out.nodes.push(n),
            _ => out.missing.push(loc.candidate.path.clone()),
        }
    }
    Ok(out)
}
