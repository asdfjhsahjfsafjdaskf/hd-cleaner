// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod dto;
mod state;

use state::AppState;

fn init_logging(dir: &std::path::Path, debug: bool) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs).ok()?;
    let appender = tracing_appender::rolling::daily(&logs, "hdcleaner.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let level = if debug { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("HDCLEANER_LOG").unwrap_or_else(|_| format!("{level},tao=warn,wry=warn").into()),
        )
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Some(guard)
}

fn main() {
    // Elevated helper mode: run the single requested privileged operation and
    // exit without ever creating a window.
    if let Some(code) = hdcleaner_core::elevation::maybe_run_helper() {
        std::process::exit(code);
    }

    let data_dir = hdcleaner_core::util::app_data_dir();
    let db = match hdcleaner_core::db::Database::open(&data_dir.join("hdcleaner.db")) {
        Ok(db) => db,
        Err(e) => {
            // Without a database the app still works, but history is not kept.
            eprintln!("database unavailable ({e}); using in-memory database");
            hdcleaner_core::db::Database::open_in_memory().expect("in-memory database")
        }
    };
    let debug = db.get_setting("debugLogs").ok().flatten().and_then(|v| v.as_bool()).unwrap_or(false);
    let _log_guard = init_logging(&data_dir, debug);
    let interrupted = db.interrupted_operations().unwrap_or_default();
    if !interrupted.is_empty() {
        tracing::warn!(count = interrupted.len(), "found operations interrupted by a previous session");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), elevated = hdcleaner_core::system::is_elevated(), "starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new(db, data_dir, interrupted))
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::app::list_drives,
            commands::app::get_settings,
            commands::app::set_setting,
            commands::app::list_history,
            commands::app::clear_history,
            commands::app::export_history,
            commands::app::resolve_interrupted,
            commands::app::list_tools,
            commands::app::launch_tool,
            commands::app::restart_elevated,
            commands::app::protection_assess,
            commands::scan::start_scan,
            commands::scan::cancel_job,
            commands::scan::close_scan,
            commands::scan::loaded_scans,
            commands::scan::node_details,
            commands::scan::find_path,
            commands::scan::scan_stats,
            commands::scan::treemap,
            commands::scan::list_snapshots,
            commands::scan::open_snapshot,
            commands::scan::export_snapshot,
            commands::scan::compare_snapshot,
            commands::scan::export_scan,
            commands::tree::view_open,
            commands::tree::view_close,
            commands::tree::view_rows,
            commands::tree::view_set_expanded,
            commands::tree::view_expand_all,
            commands::tree::view_collapse_all,
            commands::tree::view_sort,
            commands::tree::view_reveal,
            commands::tree::view_expanded_paths,
            commands::tree::view_restore,
            commands::search::search,
            commands::search::result_rows,
            commands::search::result_sort,
            commands::search::export_results,
            commands::search::dup_start,
            commands::search::dup_groups,
            commands::search::dup_autoselect,
            commands::files::plan_delete,
            commands::files::execute_delete,
            commands::files::open_path,
            commands::files::reveal_path,
            commands::files::show_properties,
            commands::files::open_terminal,
            commands::files::rename_path,
            commands::files::transfer,
            commands::programs::list_programs,
            commands::programs::program_icon,
            commands::programs::program_size,
            commands::programs::program_sizes_all,
            commands::programs::program_sizes_cached,
            commands::programs::identify_program,
            commands::programs::program_map_nodes,
            commands::uninstall::uninstall_prepare,
            commands::uninstall::uninstall_restore_point,
            commands::uninstall::uninstall_run,
            commands::uninstall::uninstall_stop_waiting,
            commands::uninstall::leftovers_scan,
            commands::uninstall::leftovers_remove,
            commands::uninstall::uninstall_finish,
            commands::uninstall::leftovers_remove_batch,
            commands::forced::forced_resolve,
            commands::forced::forced_choose,
            commands::forced::session_processes,
            commands::forced::process_terminate,
            commands::system::processes_sample,
            commands::system::process_details,
            commands::system::process_end,
            commands::system::file_icon,
            commands::system::startup_list,
            commands::system::startup_set_enabled,
            commands::system::startup_remove,
            commands::system::startup_impact,
            commands::monitor::monitor_start,
            commands::monitor::monitor_status,
            commands::monitor::monitor_cancel,
            commands::monitor::monitor_finish,
            commands::monitor::traces_list,
            commands::monitor::trace_get,
            commands::monitor::trace_delete,
            commands::monitor::trace_export,
            commands::monitor::trace_import,
            commands::uninstall::uninstall_impact,
            commands::windowsapps::extensions_list,
            commands::windowsapps::extension_remove,
            commands::windowsapps::windows_apps_list,
            commands::windowsapps::windows_app_repair,
            commands::windowsapps::windows_app_remove,
            commands::files::file_info,
            commands::programs::games_list,
            commands::programs::app_analysis,
            commands::programs::app_clean_cache,
            commands::scan::export_report,
            commands::scan::timeline_build,
            commands::backups::backups_list,
            commands::backups::backup_read,
            commands::backups::backup_restore,
            commands::backups::backup_delete,
            commands::backups::create_restore_point,
            commands::cleaner::cleaner_analyze,
            commands::cleaner::cleaner_analyze_admin,
            commands::cleaner::cleaner_close_program,
            commands::cleaner::cleaner_items,
            commands::cleaner::cleaner_run,
            commands::system::target_pick,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the application");
}
