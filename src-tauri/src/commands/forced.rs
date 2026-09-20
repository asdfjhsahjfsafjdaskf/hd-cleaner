//! Forced uninstall: build a target from name / exe / folder, pick a
//! registered match (optional), handle running processes, then reuse the
//! uninstall session (official uninstaller if any, leftovers, removal).

use crate::dto::*;
use crate::state::{AppState, UninstallSession};
use hdcleaner_core::forced::{self, ForcedInput, ForcedTarget};
use hdcleaner_core::processes::ProcInfo;
use hdcleaner_core::programs::Program;
use hdcleaner_core::uninstall::{self, UninstallCommand};
use hdcleaner_core::{ErrorPayload, AppError};
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForcedPrepared {
    pub session_id: u32,
    pub target: ForcedTarget,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForcedChoice {
    pub program: Program,
    pub command: Option<UninstallCommand>,
    pub command_error: Option<ErrorPayload>,
    pub still_installed: bool,
}

pub(crate) fn inventory(state: &AppState) -> Arc<Vec<Program>> {
    if let Some(p) = state.programs.read().clone() {
        return p;
    }
    fresh_inventory(state)
}

/// Re-read the installed programs (and update the cache). Used where the
/// list protects other programs' files, so a stale cache is not acceptable.
pub(crate) fn fresh_inventory(state: &AppState) -> Arc<Vec<Program>> {
    let (list, _) = hdcleaner_core::programs::list_all();
    let list = Arc::new(list);
    *state.programs.write() = Some(list.clone());
    list
}

#[tauri::command]
pub async fn forced_resolve(state: State<'_, AppState>, input: ForcedInput) -> CmdResult<ForcedPrepared> {
    // Fresh: the program may have been installed after the list was loaded.
    let all = fresh_inventory(&state);
    let target = forced::resolve(&input, &all).ui()?;
    let session_id = state.next_id();
    let name: String = target.program.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).take(40).collect();
    state.uninstalls.lock().insert(
        session_id,
        UninstallSession {
            program: target.program.clone(),
            command: None,
            quiet_command: None,
            stop: Arc::new(AtomicBool::new(false)),
            outcome: None,
            leftovers: Vec::new(),
            removed: Vec::new(),
            backup_dir: state.data_dir.join("backups").join(format!("{}-forced-{name}", hdcleaner_core::util::now_unix_ms())),
            operation_id: None,
            restore_point: None,
            forced: true,
            synthetic: Some(target.program.clone()),
        },
    );
    Ok(ForcedPrepared { session_id, target })
}

/// Choose which registered program (if any) the forced session targets.
/// `None` = only the synthetic description (unregistered program).
#[tauri::command]
pub async fn forced_choose(state: State<'_, AppState>, session_id: u32, program_id: Option<String>) -> CmdResult<ForcedChoice> {
    let all = inventory(&state);
    let mut guard = state.uninstalls.lock();
    let s = guard
        .get_mut(&session_id)
        .ok_or_else(|| AppError::InvalidInput("this uninstall session has ended".into()).to_payload())?;
    let synthetic = s.synthetic.clone().ok_or_else(|| AppError::InvalidInput("not a forced session".into()).to_payload())?;
    let program = match &program_id {
        Some(id) => {
            let m = all.iter().find(|p| &p.id == id).ok_or_else(|| AppError::NotFound { path: id.clone() }.to_payload())?;
            forced::target_from_match(m, &synthetic)
        }
        None => synthetic,
    };
    let (command, command_error) = if program_id.is_some() {
        match uninstall::command_for(&program, false) {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e.to_payload())),
        }
    } else {
        (None, None)
    };
    s.program = program.clone();
    s.command = command.clone();
    s.quiet_command = program_id.as_ref().and_then(|_| uninstall::command_for(&program, true).ok()).filter(|q| q.quiet);
    s.leftovers.clear();
    let still_installed = program_id.is_some() && uninstall::still_installed(&program);
    Ok(ForcedChoice { program, command, command_error, still_installed })
}

#[tauri::command]
pub async fn session_processes(state: State<'_, AppState>, session_id: u32) -> CmdResult<Vec<ProcInfo>> {
    let folder = {
        let guard = state.uninstalls.lock();
        let s = guard
            .get(&session_id)
            .ok_or_else(|| AppError::InvalidInput("this uninstall session has ended".into()).to_payload())?;
        s.program.install_location.clone().or(s.program.inferred_location.clone())
    };
    Ok(folder.as_deref().map(hdcleaner_core::processes::running_from).unwrap_or_default())
}

/// End a process (re-verifies PID ↔ path; Windows binaries are refused).
#[tauri::command]
pub async fn process_terminate(state: State<'_, AppState>, pid: u32, path: String) -> CmdResult<()> {
    let r = hdcleaner_core::processes::terminate(pid, &path);
    let _ = state.db.lock().record_operation(
        "end-process",
        if r.is_ok() { "completed" } else { "failed" },
        &path,
        1,
        &serde_json::json!({ "pid": pid, "path": path, "error": r.as_ref().err().map(|e| e.to_payload()) }),
    );
    r.ui()
}
