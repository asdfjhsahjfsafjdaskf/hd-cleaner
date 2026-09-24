// Typed wrappers around Tauri commands. The UI never touches the file system
// directly: every operation goes through these validated backend commands.
import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AppInfo, DeleteMode, DeletePlan, DeleteSummary, DiffReport, DriveInfo, DupEvent, DupGroup, DupMode,
  ErrorPayload, NodeDetails, NodeRow, OperationRecord, Page, ScanEvent, ScanMeta, ScanMethod, ScanRecord,
  ScanStats, SearchSummary, SortKey, ToolInfo, Assessment, ItemResult, Program, AppSize, Identification, SizesEvent,
  PreparedUninstall, RunEvent, Leftover, LeftoverLevel, RemovalSummary, ForcedTarget, ForcedChoice, ProcInfo,
  ProcessRow, ProcessDetails, TreeKill, StartupItem, TargetPicked, StartupImpact, CategorySummary, CleanItem, CleanEvent, CleanReport, InstallTrace, MonitorStatus, BackupSet, BackupManifest, RestoreResult, Timeline, AppAnalysis, CleanOutcome, UninstallImpact, Game, FileInfo, WindowsApp, BrowserExtension,
} from "../types";

export function isErrorPayload(e: unknown): e is ErrorPayload {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** Normalise anything thrown by invoke into an ErrorPayload. */
export function toError(e: unknown): ErrorPayload {
  if (isErrorPayload(e)) return e;
  return { kind: "unknown", message: e instanceof Error ? e.message : String(e) };
}

export type Target = { node?: number; path?: string };

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listDrives: () => invoke<DriveInfo[]>("list_drives"),
  getSettings: () => invoke<Record<string, unknown>>("get_settings"),
  setSetting: (key: string, value: unknown) => invoke<void>("set_setting", { key, value }),
  listHistory: (limit?: number) => invoke<OperationRecord[]>("list_history", { limit }),
  clearHistory: () => invoke<number>("clear_history"),
  exportHistory: (path: string) => invoke<number>("export_history", { path }),
  resolveInterrupted: (id: number, action: "resume" | "discard") => invoke<string[]>("resolve_interrupted", { id, action }),
  listTools: () => invoke<ToolInfo[]>("list_tools"),
  launchTool: (id: string) => invoke<void>("launch_tool", { id }),
  restartElevated: () => invoke<void>("restart_elevated"),
  assess: (path: string) => invoke<Assessment>("protection_assess", { path }),

  startScan: (root: string, method: ScanMethod, elevate: boolean, followJunctions: boolean, onEvent: (e: ScanEvent) => void) => {
    const ch = new Channel<ScanEvent>();
    ch.onmessage = onEvent;
    return invoke<number>("start_scan", { root, method, elevate, followJunctions, onEvent: ch });
  },
  cancelJob: (jobId: number) => invoke<boolean>("cancel_job", { jobId }),
  closeScan: (scanId: number) => invoke<void>("close_scan", { scanId }),
  loadedScans: () => invoke<[number, ScanMeta][]>("loaded_scans"),
  nodeDetails: (scanId: number, node: number) => invoke<NodeDetails>("node_details", { scanId, node }),
  findPath: (scanId: number, path: string) => invoke<number | null>("find_path", { scanId, path }),
  scanStats: (scanId: number, scope?: number) => invoke<ScanStats>("scan_stats", { scanId, scope }),
  treemap: (scanId: number, root: number, options: { width: number; height: number; metric: "allocated" | "logical"; maxRects: number; highlight: number[] }) =>
    invoke<ArrayBuffer>("treemap", { scanId, root, options }),
  listSnapshots: (root?: string) => invoke<ScanRecord[]>("list_snapshots", { root }),
  openSnapshot: (path: string) => invoke<[number, ScanMeta]>("open_snapshot", { path }),
  exportSnapshot: (scanId: number, path: string) => invoke<void>("export_snapshot", { scanId, path }),
  compareSnapshot: (oldPath: string, scanId: number, limit?: number) => invoke<DiffReport>("compare_snapshot", { oldPath, scanId, limit }),
  exportScan: (scanId: number, format: "csv" | "json", path: string, scope?: number) =>
    invoke<number>("export_scan", { scanId, scope, format, path }),

  viewOpen: (scanId: number, root?: number) => invoke<number>("view_open", { scanId, root }),
  viewClose: (viewId: number) => invoke<void>("view_close", { viewId }),
  viewRows: (viewId: number, offset: number, limit: number) => invoke<Page<NodeRow>>("view_rows", { viewId, offset, limit }),
  viewSetExpanded: (viewId: number, node: number, expanded: boolean) => invoke<number>("view_set_expanded", { viewId, node, expanded }),
  viewExpandAll: (viewId: number, node?: number) => invoke<number>("view_expand_all", { viewId, node }),
  viewCollapseAll: (viewId: number) => invoke<number>("view_collapse_all", { viewId }),
  viewSort: (viewId: number, sort: SortKey, desc: boolean) => invoke<number>("view_sort", { viewId, sort, desc }),
  viewReveal: (viewId: number, node: number) => invoke<number | null>("view_reveal", { viewId, node }),
  viewExpandedPaths: (viewId: number) => invoke<string[]>("view_expanded_paths", { viewId }),
  viewRestore: (viewId: number, paths: string[]) => invoke<number>("view_restore", { viewId, paths }),

  search: (scanId: number, query: string, opts: { scope?: number; sort?: SortKey; desc?: boolean; limit?: number } = {}) =>
    invoke<SearchSummary>("search", { scanId, query, ...opts }),
  resultRows: (resultId: number, offset: number, limit: number) => invoke<Page<NodeRow>>("result_rows", { resultId, offset, limit }),
  resultSort: (resultId: number, sort: SortKey, desc: boolean) => invoke<void>("result_sort", { resultId, sort, desc }),
  exportResults: (resultId: number, format: "csv" | "json", path: string) => invoke<number>("export_results", { resultId, format, path }),
  exportReport: (scanId: number, path: string, scope?: number) => invoke<number>("export_report", { scanId, scope, path }),
  timelineBuild: (path: string, limit?: number) => invoke<Timeline>("timeline_build", { path, limit }),

  dupStart: (scanId: number, options: { mode: DupMode; minSize: number; scope?: number }, onEvent: (e: DupEvent) => void) => {
    const ch = new Channel<DupEvent>();
    ch.onmessage = onEvent;
    return invoke<number>("dup_start", { scanId, options, onEvent: ch });
  },
  dupGroups: (dupId: number, offset: number, limit: number) => invoke<Page<DupGroup>>("dup_groups", { dupId, offset, limit }),
  dupAutoselect: (dupId: number, rule: "keepOldest" | "keepNewest" | "keepShortestPath" | "keepInFolder", folder?: string) =>
    invoke<number[]>("dup_autoselect", { dupId, rule, folder }),

  planDelete: (scanId: number | undefined, targets: Target[], mode: DeleteMode) => invoke<DeletePlan>("plan_delete", { scanId, targets, mode }),
  executeDelete: (planId: string, dryRun: boolean, allowDangerous: boolean, onItem: (index: number, total: number, r: ItemResult) => void) => {
    const ch = new Channel<{ event: "item"; data: { index: number; total: number; result: ItemResult } }>();
    ch.onmessage = (m) => onItem(m.data.index, m.data.total, m.data.result);
    return invoke<DeleteSummary>("execute_delete", { planId, dryRun, allowDangerous, onEvent: ch });
  },
  openPath: (path: string) => invoke<void>("open_path", { path }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  showProperties: (path: string) => invoke<void>("show_properties", { path }),
  openTerminal: (path: string, kind: "cmd" | "powerShell") => invoke<void>("open_terminal", { path, kind }),
  renamePath: (path: string, newName: string) => invoke<string>("rename_path", { path, newName }),
  transfer: (paths: string[], dest: string, kind: "copy" | "move") => invoke<ItemResult[]>("transfer", { paths, dest, kind }),

  listPrograms: (refresh = false) => invoke<{ programs: Program[]; appxError?: ErrorPayload | null }>("list_programs", { refresh }),
  programIcon: (id: string) => invoke<ArrayBuffer>("program_icon", { id }),
  programSize: (id: string, refresh = false) => invoke<AppSize>("program_size", { id, refresh }),
  programSizesAll: (ids: string[], onEvent: (e: SizesEvent) => void) => {
    const ch = new Channel<SizesEvent>();
    ch.onmessage = onEvent;
    return invoke<number>("program_sizes_all", { ids, onEvent: ch });
  },
  programSizesCached: () => invoke<[string, number, number][]>("program_sizes_cached"),
  identifyProgram: (path: string) => invoke<Identification[]>("identify_program", { path }),
  programMapNodes: (scanId: number, id: string) => invoke<{ nodes: number[]; missing: string[] }>("program_map_nodes", { scanId, id }),

  uninstallPrepare: (id: string) => invoke<PreparedUninstall>("uninstall_prepare", { id }),
  uninstallRestorePoint: (sessionId: number) => invoke<void>("uninstall_restore_point", { sessionId }),
  uninstallRun: (sessionId: number, quiet: boolean, onEvent: (e: RunEvent) => void) => {
    const ch = new Channel<RunEvent>();
    ch.onmessage = onEvent;
    return invoke<void>("uninstall_run", { sessionId, quiet, onEvent: ch });
  },
  uninstallStopWaiting: (sessionId: number) => invoke<void>("uninstall_stop_waiting", { sessionId }),
  leftoversScan: (sessionId: number, level: LeftoverLevel) => invoke<Leftover[]>("leftovers_scan", { sessionId, level }),
  leftoversRemove: (sessionId: number, ids: number[], recycle: boolean, allowDangerous: boolean, dryRun: boolean) =>
    invoke<RemovalSummary>("leftovers_remove", { sessionId, ids, recycle, allowDangerous, dryRun }),
  uninstallFinish: (sessionId: number) => invoke<void>("uninstall_finish", { sessionId }),
  uninstallImpact: (sessionId: number) => invoke<UninstallImpact>("uninstall_impact", { sessionId }),
  leftoversRemoveBatch: (selections: { sessionId: number; ids: number[] }[], recycle: boolean, allowDangerous: boolean, dryRun: boolean) =>
    invoke<[number, RemovalSummary][]>("leftovers_remove_batch", { selections, recycle, allowDangerous, dryRun }),
  forcedResolve: (input: { name?: string; exe?: string; folder?: string }) =>
    invoke<{ sessionId: number; target: ForcedTarget }>("forced_resolve", { input }),
  forcedChoose: (sessionId: number, programId: string | null) => invoke<ForcedChoice>("forced_choose", { sessionId, programId }),
  sessionProcesses: (sessionId: number) => invoke<ProcInfo[]>("session_processes", { sessionId }),
  processTerminate: (pid: number, path: string) => invoke<void>("process_terminate", { pid, path }),

  processesSample: () => invoke<ProcessRow[]>("processes_sample"),
  processDetails: (pid: number) => invoke<ProcessDetails>("process_details", { pid }),
  processEnd: (pid: number, path: string | null | undefined, tree: boolean, elevated: boolean) => invoke<TreeKill[]>("process_end", { pid, path: path ?? null, tree, elevated }),
  fileIcon: (path: string) => invoke<ArrayBuffer>("file_icon", { path }),
  startWithWindows: () => invoke<boolean>("start_with_windows"),
  setStartWithWindows: (enabled: boolean) => invoke<void>("set_start_with_windows", { enabled }),
  startupList: () => invoke<StartupItem[]>("startup_list"),
  startupSetEnabled: (id: string, command: string, enabled: boolean) => invoke<void>("startup_set_enabled", { id, command, enabled }),
  startupRemove: (id: string, command: string) => invoke<string>("startup_remove", { id, command }),
  startupImpact: (cached: boolean) => invoke<StartupImpact | null>("startup_impact", { cached }),

  monitorStart: () => invoke<MonitorStatus>("monitor_start"),
  monitorStatus: () => invoke<MonitorStatus>("monitor_status"),
  monitorCancel: () => invoke<MonitorStatus>("monitor_cancel"),
  monitorFinish: (name: string) => invoke<InstallTrace>("monitor_finish", { name }),
  tracesList: () => invoke<InstallTrace[]>("traces_list"),
  traceGet: (id: number) => invoke<InstallTrace>("trace_get", { id }),
  traceDelete: (id: number) => invoke<boolean>("trace_delete", { id }),
  traceExport: (id: number, path: string) => invoke<number>("trace_export", { id, path }),
  traceImport: (path: string, suffix: string) => invoke<InstallTrace>("trace_import", { path, suffix }),

  fileInfo: (path: string) => invoke<FileInfo>("file_info", { path }),
  clipboardFiles: (paths: string[], cut: boolean) => invoke<void>("clipboard_files", { paths, cut }),
  extensionsList: () => invoke<BrowserExtension[]>("extensions_list"),
  extensionRemove: (id: string, browser: string, recycle: boolean) => invoke<void>("extension_remove", { id, browser, recycle }),
  windowsAppsList: (measure: boolean) => invoke<WindowsApp[]>("windows_apps_list", { measure }),
  windowsAppRepair: (fullName: string) => invoke<void>("windows_app_repair", { fullName }),
  windowsAppRemove: (fullName: string, allUsers: boolean) => invoke<void>("windows_app_remove", { fullName, allUsers }),
  gamesList: (measure: boolean) => invoke<Game[]>("games_list", { measure }),
  appAnalysis: (id: string) => invoke<AppAnalysis>("app_analysis", { id }),
  appCleanCache: (id: string, category: string, dryRun: boolean) => invoke<CleanOutcome>("app_clean_cache", { id, category, dryRun }),

  backupsList: () => invoke<BackupSet[]>("backups_list"),
  backupRead: (id: string) => invoke<BackupManifest>("backup_read", { id }),
  backupRestore: (id: string, indexes: number[]) => invoke<RestoreResult[]>("backup_restore", { id, indexes }),
  backupDelete: (id: string) => invoke<void>("backup_delete", { id }),
  createRestorePoint: (description: string) => invoke<void>("create_restore_point", { description }),

  cleanerAnalyze: () => invoke<CategorySummary[]>("cleaner_analyze"),
  cleanerAnalyzeAdmin: () => invoke<CategorySummary[]>("cleaner_analyze_admin"),
  cleanerCloseProgram: (id: string) => invoke<ProcInfo[]>("cleaner_close_program", { id }),
  cleanerItems: (id: string, offset: number, limit: number) => invoke<Page<CleanItem>>("cleaner_items", { id, offset, limit }),
  cleanerRun: (ids: string[], dryRun: boolean, onEvent: (e: CleanEvent) => void) => {
    const ch = new Channel<CleanEvent>();
    ch.onmessage = onEvent;
    return invoke<CleanReport>("cleaner_run", { ids, dryRun, onEvent: ch });
  },
  targetPick: (hint: string) => invoke<TargetPicked | null>("target_pick", { hint }),
};
