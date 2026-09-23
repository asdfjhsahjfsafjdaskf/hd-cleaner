//! Uninstall wizard backend: prepare → (restore point) → run official
//! uninstaller → scan leftovers → remove selected (with backup) → finish.
//! The whole flow is journaled as one "uninstall" operation.

use crate::dto::*;
use crate::state::{AppState, UninstallSession};
use hdcleaner_core::elevation::{run_elevated_ops, ElevatedOp};
use hdcleaner_core::leftovers::{self, Leftover, Level, RemovalOptions, RemovalResult};
use hdcleaner_core::programs::Program;
use hdcleaner_core::uninstall::{self, RunOutcome, UninstallCommand};
use hdcleaner_core::{ErrorPayload, AppError};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    pub session_id: u32,
    pub program: Program,
    pub command: Option<UninstallCommand>,
    /// Quiet variant (QuietUninstallString / msiexec /passive), when different.
    pub quiet_command: Option<UninstallCommand>,
    /// Why the official uninstaller cannot run (missing file, NoRemove...).
    pub command_error: Option<ErrorPayload>,
    pub has_quiet: bool,
    pub still_installed: bool,
    pub elevated: bool,
}

fn session<'a>(state: &'a AppState, id: u32) -> Result<parking_lot::MappedMutexGuard<'a, UninstallSession>, ErrorPayload> {
    let guard = state.uninstalls.lock();
    parking_lot::MutexGuard::try_map(guard, |m| m.get_mut(&id))
        .map_err(|_| AppError::InvalidInput("this uninstall session has ended".into()).to_payload())
}

fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).take(40).collect();
    n.trim_matches('_').to_string()
}

#[tauri::command]
pub async fn uninstall_prepare(state: State<'_, AppState>, id: String) -> CmdResult<Prepared> {
    let (list, _) = crate::commands::programs::load(&state, false);
    let program = match list.iter().find(|p| p.id == id).cloned() {
        Some(p) => p,
        // The cached list may predate an install (or another uninstall): look again.
        None => {
            let (fresh, _) = crate::commands::programs::load(&state, true);
            fresh.iter().find(|p| p.id == id).cloned().ok_or_else(|| AppError::NotFound { path: id.clone() }.to_payload())?
        }
    };
    let (command, command_error) = match uninstall::command_for(&program, false) {
        Ok(c) => (Some(c), None),
        Err(e) => (None, Some(e.to_payload())),
    };
    let quiet_command = uninstall::command_for(&program, true).ok().filter(|q| q.quiet && Some(q) != command.as_ref());
    let still_installed = uninstall::still_installed(&program);
    let session_id = state.next_id();
    let backup_dir = state
        .data_dir
        .join("backups")
        .join(format!("{}-{}", hdcleaner_core::util::now_unix_ms(), safe_name(&program.name)));
    state.uninstalls.lock().insert(
        session_id,
        UninstallSession {
            program: program.clone(),
            command: command.clone(),
            quiet_command: quiet_command.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            outcome: None,
            leftovers: Vec::new(),
            removed: Vec::new(),
            backup_dir,
            operation_id: None,
            restore_point: None,
            forced: false,
            synthetic: None,
        },
    );
    Ok(Prepared {
        session_id,
        has_quiet: quiet_command.is_some(),
        program,
        command,
        quiet_command,
        command_error,
        still_installed,
        elevated: hdcleaner_core::system::is_elevated(),
    })
}

/// Create a System Restore point (one UAC prompt unless already elevated).
#[tauri::command]
pub async fn uninstall_restore_point(state: State<'_, AppState>, session_id: u32) -> CmdResult<()> {
    let name = session(&state, session_id)?.program.name.clone();
    let description = format!("Before uninstalling {name}");
    let r = if hdcleaner_core::system::is_elevated() {
        uninstall::create_restore_point(&description)
    } else {
        run_elevated_ops(&[ElevatedOp::CreateRestorePoint { description: description.clone() }]).and_then(|v| match v.first() {
            Some(r) if r.ok => Ok(()),
            Some(r) => Err(AppError::Helper(r.message.clone().unwrap_or_default())),
            None => Err(AppError::Helper("no result from helper".into())),
        })
    };
    session(&state, session_id)?.restore_point = Some(r.is_ok());
    r.ui()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "event", content = "data")]
pub enum RunEvent {
    Waiting { processes: Vec<String> },
    Done { outcome: RunOutcome },
    Error(ErrorPayload),
}

#[tauri::command]
pub fn uninstall_run(app: AppHandle, state: State<'_, AppState>, session_id: u32, quiet: bool, on_event: Channel<RunEvent>) -> CmdResult<()> {
    let (program, command, stop) = {
        let s = session(&state, session_id)?;
        let chosen = if quiet { s.quiet_command.clone().or_else(|| s.command.clone()) } else { s.command.clone() };
        let cmd = chosen.ok_or_else(|| AppError::NotSupported("no uninstaller can be run for this program".into()).to_payload())?;
        (s.program.clone(), cmd, s.stop.clone())
    };
    let op_id = state
        .db
        .lock()
        .begin_operation(
            "uninstall",
            &program.name,
            1,
            false,
            &serde_json::json!({ "program": program.name, "programId": program.id, "command": command }),
        )
        .ui()?;
    session(&state, session_id)?.operation_id = Some(op_id);
    std::thread::spawn(move || {
        let r = uninstall::run(&program, &command, &stop, |procs| {
            let _ = on_event.send(RunEvent::Waiting { processes: procs.to_vec() });
        });
        let state = app.state::<AppState>();
        match r {
            Ok(outcome) => {
                if let Ok(mut s) = session(&state, session_id) {
                    s.outcome = Some(outcome.clone());
                }
                // The inventory changed: reload it on next use.
                *state.programs.write() = None;
                let _ = on_event.send(RunEvent::Done { outcome });
            }
            Err(e) => {
                let _ = state.db.lock().finish_operation(op_id, "failed", 0, 1, &serde_json::json!({ "program": program.name, "error": e.to_payload() }));
                let _ = on_event.send(RunEvent::Error(e.to_payload()));
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub fn uninstall_stop_waiting(state: State<'_, AppState>, session_id: u32) -> CmdResult<()> {
    session(&state, session_id)?.stop.store(true, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn leftovers_scan(state: State<'_, AppState>, session_id: u32, level: Level) -> CmdResult<Vec<Leftover>> {
    let (program, forced) = {
        let s = session(&state, session_id)?;
        (s.program.clone(), s.forced)
    };
    // The other installed programs protect their own folders from being
    // proposed: read them now, not from a list loaded before other installs.
    let others: Vec<_> = super::forced::fresh_inventory(&state).iter().filter(|p| p.id != program.id).cloned().collect();
    // Never scan while the program is still registered: its live files and
    // entries would be proposed as "leftovers". Forced mode is the explicit
    // exception (broken programs stay registered), reviewed item by item.
    if !forced && uninstall::still_installed(&program) {
        return Err(AppError::InvalidInput(
            "the program is still installed (the uninstaller may have been cancelled); leftovers are only scanned after it is removed".into(),
        )
        .to_payload());
    }
    // An installation trace recorded for this program tells exactly what the
    // installer created: those items are added to what the rules found.
    // One lock at a time: holding the guard while locking again deadlocks.
    let traces: Vec<hdcleaner_core::monitor::InstallTrace> = {
        let rows = state.db.lock().traces_for(&program.id, &program.name).unwrap_or_default();
        rows.into_iter().filter_map(|t| state.db.lock().trace(t.id).ok()).collect()
    };
    let found = tauri::async_runtime::spawn_blocking(move || {
        let mut found = leftovers::scan(&program, &others, level, true);
        for t in &traces {
            let extra = leftovers::from_trace(&program, &others, t, &found);
            found.extend(extra);
        }
        found
    })
    .await
    .map_err(|e| AppError::Helper(e.to_string()).to_payload())?;
    session(&state, session_id)?.leftovers = found.clone();
    Ok(found)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalSummary {
    pub results: Vec<RemovalResult>,
    pub backup_dir: String,
    pub elevated_count: usize,
}

#[tauri::command]
pub async fn leftovers_remove(
    state: State<'_, AppState>,
    session_id: u32,
    ids: Vec<u32>,
    recycle: bool,
    allow_dangerous: bool,
    dry_run: bool,
) -> CmdResult<RemovalSummary> {
    let mut v = remove_many(&state, vec![BatchSelection { session_id, ids }], recycle, allow_dangerous, dry_run).await?;
    Ok(v.remove(0).1)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSelection {
    pub session_id: u32,
    pub ids: Vec<u32>,
}

/// Remove leftovers of several sessions (batch uninstall) with a single UAC
/// prompt for everything that needs administrator rights.
#[tauri::command]
pub async fn leftovers_remove_batch(
    state: State<'_, AppState>,
    selections: Vec<BatchSelection>,
    recycle: bool,
    allow_dangerous: bool,
    dry_run: bool,
) -> CmdResult<Vec<(u32, RemovalSummary)>> {
    remove_many(&state, selections, recycle, allow_dangerous, dry_run).await
}

async fn remove_many(
    state: &AppState,
    selections: Vec<BatchSelection>,
    recycle: bool,
    allow_dangerous: bool,
    dry_run: bool,
) -> CmdResult<Vec<(u32, RemovalSummary)>> {
    // 1. Per session, everything that can be done without elevation.
    type Session = (u32, Vec<Leftover>, std::path::PathBuf, Vec<RemovalResult>, Vec<(u32, leftovers::PendingOp)>, Vec<hdcleaner_core::backups::Entry>);
    let mut per: Vec<Session> = Vec::new();
    for sel in selections {
        let (items, backup_dir) = {
            let s = session(state, sel.session_id)?;
            let items: Vec<Leftover> = s.leftovers.iter().filter(|l| sel.ids.contains(&l.id)).cloned().collect();
            (items, s.backup_dir.clone())
        };
        if items.is_empty() {
            continue;
        }
        let (bd, its) = (backup_dir.clone(), items.clone());
        let removal = tauri::async_runtime::spawn_blocking(move || {
            leftovers::remove(&its, &RemovalOptions { backup_dir: &bd, recycle, allow_dangerous, dry_run, quarantine: true })
        })
        .await
        .map_err(|e| AppError::Helper(e.to_string()).to_payload())?;
        per.push((sel.session_id, items, backup_dir, removal.results, removal.pending, removal.saved));
    }
    if per.is_empty() {
        return Err(AppError::InvalidInput("nothing selected".into()).to_payload());
    }

    // 2. One elevated helper run for all pending operations of all sessions.
    let ops: Vec<ElevatedOp> = per
        .iter()
        .flat_map(|(_, _, _, _, pending, _)| pending.iter().map(|(_, op)| ElevatedOp::Removal { item: op.clone() }))
        .collect();
    let outcome = if ops.is_empty() {
        Ok(Vec::new())
    } else {
        tauri::async_runtime::spawn_blocking(move || run_elevated_ops(&ops))
            .await
            .map_err(|e| AppError::Helper(e.to_string()).to_payload())?
    };

    // 3. Merge results back per session; manifests and session bookkeeping.
    let mut index = 0usize;
    let mut out = Vec::new();
    for (session_id, items, backup_dir, mut results, pending, saved) in per {
        let elevated_count = pending.len();
        for (id, _) in &pending {
            let path = items.iter().find(|l| l.id == *id).map(|l| l.path.clone()).unwrap_or_default();
            let r = match &outcome {
                Ok(v) => match v.iter().find(|r| r.index == index) {
                    Some(r) if r.ok => RemovalResult { id: *id, path, status: "removed", error: None },
                    Some(r) => RemovalResult {
                        id: *id,
                        path,
                        status: "failed",
                        error: Some(AppError::Helper(r.message.clone().unwrap_or_default()).to_payload()),
                    },
                    None => RemovalResult { id: *id, path, status: "failed", error: Some(AppError::Helper("no result".into()).to_payload()) },
                },
                Err(e) => RemovalResult { id: *id, path, status: "needsElevation", error: Some(e.to_payload()) },
            };
            results.push(r);
            index += 1;
        }
        if !dry_run && !saved.is_empty() {
            // The manifest is what the Backups page reads to restore this.
            let program = items.first().map(|_| ()).and(session(state, session_id).ok().map(|s| s.program.name.clone()));
            let mut manifest = hdcleaner_core::backups::Manifest::new("uninstall", program.as_deref().unwrap_or("uninstall"));
            manifest.entries = saved;
            if let Err(e) = manifest.save(&backup_dir) {
                tracing::warn!("backup manifest not written: {e}");
            }
        }
        if !dry_run {
            if let Ok(mut s) = session(state, session_id) {
                s.removed.extend(results.iter().cloned());
            }
        }
        out.push((session_id, RemovalSummary { results, backup_dir: backup_dir.to_string_lossy().into_owned(), elevated_count }));
    }
    Ok(out)
}

/// Close the session and record the operation in the history.
#[tauri::command]
pub fn uninstall_finish(state: State<'_, AppState>, session_id: u32) -> CmdResult<()> {
    let Some(s) = state.uninstalls.lock().remove(&session_id) else { return Ok(()) };
    // Nothing happened in this session: nothing to record.
    if s.operation_id.is_none() && s.removed.is_empty() {
        return Ok(());
    }
    let removed = s.removed.iter().filter(|r| r.status == "removed").count();
    let failed = s.removed.iter().filter(|r| r.status != "removed").count();
    let details = serde_json::json!({
        "program": s.program.name,
        "programId": s.program.id,
        "command": s.command,
        "outcome": s.outcome,
        "restorePoint": s.restore_point,
        "leftoversFound": s.leftovers.len(),
        "leftoversRemoved": s.removed,
        "backupDir": s.backup_dir,
    });
    let db = state.db.lock();
    let status = match (&s.outcome, failed) {
        (Some(o), 0) if !o.still_installed => "completed",
        (Some(o), _) if !o.still_installed => "partial",
        (Some(_), _) => "cancelled",
        (None, _) => "completed",
    };
    match s.operation_id {
        Some(id) => db.finish_operation(id, status, removed + 1, failed, &details).ui()?,
        None => {
            let kind = if s.forced { "forced-uninstall" } else { "uninstall-leftovers" };
            db.record_operation(kind, status, &s.program.name, removed, &details).ui()?;
        }
    }
    Ok(())
}
