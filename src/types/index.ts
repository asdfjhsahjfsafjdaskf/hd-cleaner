// Shapes returned by the Rust commands (camelCase via serde).

export interface ErrorPayload {
  kind: string;
  message: string;
  path?: string | null;
  suggestion?: string | null;
  code?: number | null;
}

export type DriveKind = "fixed" | "removable" | "network" | "cdRom" | "ramDisk" | "unknown";
export type MediaKind = "ssd" | "hdd" | "unknown";

export interface DriveInfo {
  root: string;
  letter: string;
  label: string;
  fileSystem: string;
  kind: DriveKind;
  media: MediaKind;
  totalBytes: number;
  freeBytes: number;
  usedBytes: number;
  ready: boolean;
  fastScanCapable: boolean;
}

export interface OsInfo {
  elevated: boolean;
  arch: string;
  windowsDir: string;
  build: number;
}

export interface OperationRecord {
  id: number;
  kind: string;
  status: string;
  startedMs: number;
  finishedMs?: number | null;
  summary: string;
  itemCount: number;
  okCount: number;
  errorCount: number;
  dryRun: boolean;
  details: any;
}

export interface AppInfo {
  name: string;
  version: string;
  os: OsInfo;
  dataDir: string;
  interrupted: OperationRecord[];
}

export type ScanMethod = "auto" | "standard" | "ntfsMft";
export type ScanPhase = "starting" | "readingMft" | "enumerating" | "buildingTree" | "done";

export interface ScanProgress {
  phase: ScanPhase;
  files: number;
  dirs: number;
  bytes: number;
  errors: number;
  recordsDone: number;
  recordsTotal: number;
  current: string;
}

export interface ScanErrorSample {
  path: string;
  message: string;
}

export interface ScanMeta {
  rootPath: string;
  method: string;
  fileSystem: string;
  startedMs: number;
  durationMs: number;
  files: number;
  dirs: number;
  totalSize: number;
  totalAlloc: number;
  volumeTotal: number;
  volumeFree: number;
  errorCount: number;
  errorSamples: ScanErrorSample[];
  notes: string[];
}

export type ScanEvent =
  | { event: "progress"; data: ScanProgress }
  | { event: "done"; data: { scanId: number; meta: ScanMeta } }
  | { event: "cancelled" }
  | { event: "error"; data: ErrorPayload };

export type Category =
  | "other" | "video" | "image" | "audio" | "executable" | "archive" | "game"
  | "document" | "code" | "cache" | "system" | "installer" | "diskImage";

export interface NodeRow {
  id: number;
  name: string;
  isDir: boolean;
  size: number;
  alloc: number;
  files: number;
  dirs: number;
  modified: number;
  created: number;
  accessed: number;
  ext: string;
  category: Category;
  attributes: number;
  flags: number;
  links: number;
  childCount: number;
  parentShare: number;
  depth: number;
  expanded: boolean;
  path?: string;
}

export interface NodeDetails extends NodeRow {
  path: string;
  ancestors: number[];
  hardlinkDuplicate: boolean;
  cloud: boolean;
  reparse: boolean;
  unreadable: boolean;
}

export interface Page<T> {
  total: number;
  offset: number;
  rows: T[];
}

export type SortKey = "name" | "size" | "alloc" | "modified" | "created" | "accessed" | "ext" | "path" | "files";

export interface SearchSummary {
  resultId: number;
  total: number;
  totalSize: number;
  totalAlloc: number;
  elapsedMs: number;
}

export interface Bucket {
  key: string;
  files: number;
  size: number;
  alloc: number;
}

export interface ScanStats {
  categories: Bucket[];
  extensions: Bucket[];
  histogram: Bucket[];
}

export type Risk = "safe" | "review" | "dangerous" | "blocked";

export interface Assessment {
  risk: Risk;
  reason: string;
}

export interface PlanItem {
  path: string;
  finalPath: string;
  isDir: boolean;
  isLink: boolean;
  size: number;
  assessment: Assessment;
  error?: ErrorPayload | null;
}

export type DeleteMode = "recycleBin" | "permanent";

export interface DeletePlan {
  id: string;
  mode: DeleteMode;
  items: PlanItem[];
  totalSize: number;
  blocked: number;
  dangerous: number;
  createdMs: number;
}

export type ItemOutcome =
  | { status: "deleted" }
  | { status: "wouldDelete" }
  | { status: "skipped"; reason: string }
  | { status: "failed"; error: ErrorPayload };

export type ItemResult = { path: string } & ItemOutcome;

export interface DeleteSummary {
  operationId: number;
  deleted: number;
  wouldDelete: number;
  skipped: number;
  failed: number;
  freed: number;
  results: ItemResult[];
}

export type DupMode = "quick" | "quickWithDate" | "precise";

export interface DupGroup {
  id: number;
  size: number;
  hash: string;
  wasted: number;
  files: NodeRow[];
}

export type DupEvent =
  | { event: "progress"; data: ScanProgress }
  | { event: "done"; data: { dupId: number; groups: number; totalWasted: number; bytesHashed: number; unreadable: number } }
  | { event: "cancelled" }
  | { event: "error"; data: ErrorPayload };

export interface ScanRecord {
  id: number;
  root: string;
  method: string;
  startedMs: number;
  durationMs: number;
  files: number;
  dirs: number;
  totalSize: number;
  totalAlloc: number;
  snapshotPath?: string | null;
}

export type ChangeKind = "added" | "removed" | "grew" | "shrank";

export interface Change {
  path: string;
  kind: ChangeKind;
  isDir: boolean;
  oldSize: number;
  newSize: number;
  delta: number;
}

export interface DiffReport {
  oldRoot: string;
  newRoot: string;
  oldStartedMs: number;
  newStartedMs: number;
  oldTotal: number;
  newTotal: number;
  totalDelta: number;
  addedFiles: number;
  removedFiles: number;
  grownFiles: number;
  shrunkFiles: number;
  files: Change[];
  folders: Change[];
}

export interface ToolInfo {
  id: string;
  group: string;
}

// Node flag bits (crates/hdcleaner-core/src/scan/model.rs)
export const NodeFlags = {
  DIR: 1 << 0,
  HARDLINK_DUP: 1 << 1,
  REPARSE: 1 << 2,
  CLOUD: 1 << 3,
  UNREADABLE: 1 << 4,
  METAFILE: 1 << 5,
  SPARSE: 1 << 6,
  COMPRESSED: 1 << 7,
  SYMLINK: 1 << 8,
  MOUNT_POINT: 1 << 9,
  NOT_FOLLOWED: 1 << 10,
  MULTI_LINK: 1 << 11,
} as const;

// ---- Installed programs (Phase 5) ----
export type ProgramSource = "win32" | "msi" | "store" | "appx";
export type Arch = "x64" | "x86" | "arm64" | "neutral" | "unknown";

export interface Program {
  id: string;
  name: string;
  version?: string | null;
  publisher?: string | null;
  installDate?: string | null;
  installLocation?: string | null;
  inferredLocation?: string | null;
  uninstallString?: string | null;
  quietUninstallString?: string | null;
  modifyPath?: string | null;
  reportedSize?: number | null;
  icon?: string | null;
  url?: string | null;
  source: ProgramSource;
  arch: Arch;
  scope: "machine" | "user";
  registryKey?: string | null;
  msiProductCode?: string | null;
  systemComponent: boolean;
  isUpdate: boolean;
  noRemove: boolean;
  packageFullName?: string | null;
  packageFamilyName?: string | null;
  isFramework: boolean;
  signatureKind?: string | null;
  dependencies: string[];
}

export type Confidence = "confirmed" | "probable" | "possible";
export type LocationKind = "install" | "localAppData" | "roamingAppData" | "programData" | "packageData";

export interface AppLocation {
  path: string;
  kind: LocationKind;
  confidence: Confidence;
  reason: string;
  measured: { total: number; cache: number; logs: number; files: number };
  source: "scan" | "live";
}

export interface AppSize {
  programId: string;
  locations: AppLocation[];
  install: number;
  userData: number;
  cache: number;
  logs: number;
  total: number;
  possibleTotal: number;
  reported?: number | null;
}

export interface Identification {
  programId: string;
  name: string;
  confidence: Confidence;
  reason: string;
  location: string;
}

export type SizesEvent =
  | { event: "progress"; data: { done: number; total: number; programId: string; totalSize: number } }
  | { event: "done"; data: { measured: number } }
  | { event: "cancelled" };
// ---- Uninstall (Phase 6) ----
export interface UninstallCommand {
  file: string;
  params: string;
  quiet: boolean;
  kind: "msi" | "registry" | "appx";
}

export interface PreparedUninstall {
  sessionId: number;
  program: Program;
  command?: UninstallCommand | null;
  quietCommand?: UninstallCommand | null;
  commandError?: ErrorPayload | null;
  hasQuiet: boolean;
  stillInstalled: boolean;
  elevated: boolean;
}

export interface RunOutcome {
  exitCode?: number | null;
  processes: string[];
  stillInstalled: boolean;
  stoppedWaiting: boolean;
  durationMs: number;
}

export type RunEvent =
  | { event: "waiting"; data: { processes: string[] } }
  | { event: "done"; data: { outcome: RunOutcome } }
  | { event: "error"; data: ErrorPayload };

export type LeftoverLevel = "safe" | "moderate" | "advanced";
export type LeftoverKind = "folder" | "file" | "shortcut" | "registryKey" | "registryValue" | "service" | "task";

export interface Leftover {
  id: number;
  kind: LeftoverKind;
  category: "files" | "registry" | "shortcuts" | "startup" | "services" | "tasks";
  level: LeftoverLevel;
  confidence: number;
  reason: string;
  detail?: string | null;
  path: string;
  size: number;
  shared: boolean;
  preselected: boolean;
  risk?: Risk | null;
}

export interface RemovalResult {
  id: number;
  path: string;
  status: "removed" | "failed" | "skipped" | "needsElevation" | "wouldRemove";
  error?: ErrorPayload | null;
}

export interface RemovalSummary {
  results: RemovalResult[];
  backupDir: string;
  elevatedCount: number;
}
// ---- Forced / batch uninstall (Phase 7) ----
export interface ProcInfo {
  pid: number;
  parent: number;
  name: string;
  path?: string | null;
}

export interface ForcedTarget {
  program: Program;
  matches: { program: Program; reason: "sameFolder" | "uninstallerInFolder" | "sameName" }[];
  processes: ProcInfo[];
  version?: { company?: string | null; product?: string | null; description?: string | null; version?: string | null } | null;
}

export interface ForcedChoice {
  program: Program;
  command?: UninstallCommand | null;
  commandError?: ErrorPayload | null;
  stillInstalled: boolean;
}
// ---- Phase 8: processes, startup, Target Mode ----

export interface ProcessRow {
  pid: number;
  parent: number;
  name: string;
  path?: string | null;
  threads: number;
  privateBytes?: number | null;
  workingSet?: number | null;
  cpu?: number | null;
  cpuTimeMs?: number | null;
  startedMs?: number | null;
  user?: string | null;
  company?: string | null;
  description?: string | null;
  isWindows: boolean;
  isSelf: boolean;
}

export interface VersionInfo {
  company?: string | null;
  product?: string | null;
  description?: string | null;
  version?: string | null;
}

export interface TreeKill {
  pid: number;
  name: string;
  path?: string | null;
  ok: boolean;
  skipped: boolean;
  error?: ErrorPayload | null;
}

export type StartupSource =
  | { kind: "runKey"; hive: "localMachine" | "currentUser"; view: "default" | "reg64" | "reg32"; once: boolean }
  | { kind: "startupFolder"; common: boolean }
  | { kind: "task" }
  | { kind: "service" };

export interface StartupItem {
  id: string;
  name: string;
  source: StartupSource;
  location: string;
  key: string;
  command: string;
  exe?: string | null;
  exeExists: boolean;
  enabled: boolean;
  disabledAtMs?: number | null;
  canDisable: boolean;
  canRemove: boolean;
  needsAdmin: boolean;
  isWindows: boolean;
  company?: string | null;
  description?: string | null;
  detail?: string | null;
  programId?: string | null;
  programName?: string | null;
}

export interface ProcessDetails {
  row: ProcessRow;
  version?: VersionInfo | null;
  programs: Identification[];
  startup: StartupItem[];
  children: number;
}

export interface WindowInfo {
  hwnd: number;
  title: string;
  className: string;
  pid: number;
  hostPid?: number | null;
  rect: [number, number, number, number];
  shell?: "taskbar" | "desktop" | "trayOverflow" | null;
  trayIcon?: { tooltip: string; pid: number; ownerHwnd: number } | null;
  trayReadable: boolean;
}

export interface TargetPicked {
  window: WindowInfo;
  process?: ProcessRow | null;
}

export interface Boot {
  timeMs: number;
  bootMs: number;
  mainPathMs: number;
  postBootMs: number;
  startupApps: number;
}

export interface Slowdown {
  kind: "app" | "driver" | "service";
  name: string;
  path: string;
  count: number;
  avgDelayMs: number;
  maxDelayMs: number;
  lastMs: number;
}

export interface StartupImpact {
  boots: Boot[];
  items: Record<string, Slowdown>;
  others: Slowdown[];
}

// ---- Phase 9: cleaner ----

export type CleanRisk = "safe" | "review" | "dangerous";
export type CleanGroup = "system" | "apps" | "browsers" | "privacy";

export interface CleanCategory {
  id: string;
  group: CleanGroup;
  label: string;
  owner?: string | null;
  risk: CleanRisk;
  defaultOn: boolean;
  admin: boolean;
  special?: { kind: "recycleBin" | "chromiumDownloads" | "registryMru" } | null;
  warning?: string | null;
}

export interface CategorySummary {
  category: CleanCategory;
  count: number;
  bytes: number;
  running: boolean;
  recentSkipped: number;
  adminPending: boolean;
  truncated: boolean;
  error?: ErrorPayload | null;
}

export interface CleanItem {
  path: string;
  size: number;
  mtime: number;
  display?: string | null;
}

export interface CleanOutcome {
  removed: number;
  freed: number;
  inUse: number;
  changed: number;
  denied: number;
  failed: number;
  errors: string[];
}

export interface CleanReport {
  results: { id: string; outcome?: CleanOutcome | null; error?: ErrorPayload | null }[];
  removed: number;
  freed: number;
  dryRun: boolean;
  backupDir?: string | null;
}

export type CleanEvent =
  | { event: "category"; data: { id: string; index: number; total: number } }
  | { event: "progress"; data: { id: string; done: number; total: number } };

// ---- Phase 10: installation monitor ----

export interface TraceFile {
  path: string;
  size: number;
  transient: boolean;
  related: boolean;
}

export interface TraceRegistry {
  path: string;
  kind: "key" | "value";
  related: boolean;
}

export interface InstallTrace {
  id: number;
  name: string;
  programId?: string | null;
  programName?: string | null;
  startedMs: number;
  finishedMs: number;
  files: TraceFile[];
  registry: TraceRegistry[];
  services: string[];
  tasks: string[];
  bytes: number;
}

export interface MonitorStatus {
  running: boolean;
  startedMs: number;
  roots: string[];
  baseline: number;
  seen: number;
}

// ---- backups ----------------------------------------------------------------

export type BackupEntryKind = "file" | "folder" | "registry" | "task";

export interface BackupEntry {
  kind: BackupEntryKind;
  /** Where it came from (a path, or the registry target). */
  path: string;
  /** File inside the backup folder holding the saved copy. */
  stored?: string | null;
  size: number;
}

export interface BackupSet {
  id: string;
  path: string;
  createdMs: number;
  kind: string;
  label: string;
  entries: number;
  bytes: number;
  restorable: number;
  /** Written before backups had a manifest: less is known about it. */
  legacy: boolean;
}

export interface BackupManifest {
  version: number;
  kind: string;
  label: string;
  createdMs: number;
  entries: BackupEntry[];
}

export interface RestoreResult {
  index: number;
  path: string;
  status: "restored" | "partial" | "exists" | "unknownOrigin" | "missing" | "failed";
  values?: number | null;
  error?: ErrorPayload | null;
}

// ---- timeline ---------------------------------------------------------------

export interface TimelinePoint {
  takenMs: number;
  size: number;
  alloc: number;
  files: number;
  dirs: number;
  /** The folder was not in this snapshot. */
  missing: boolean;
}

export interface Timeline {
  path: string;
  points: TimelinePoint[];
  /** Newest minus oldest measured point, in allocated bytes. */
  delta: number;
  days: number;
  unreadable: number;
}
