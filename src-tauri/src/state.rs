//! Application state shared by all commands.

use hdcleaner_core::db::Database;
use hdcleaner_core::duplicates::DupReport;
use hdcleaner_core::fsops::DeletePlan;
use hdcleaner_core::scan::{NodeId, ScanControl, ScanTree};
use hdcleaner_core::search::SortKey;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub type SharedTree = Arc<RwLock<ScanTree>>;

pub struct ResultSet {
    pub scan_id: u32,
    pub ids: Vec<NodeId>,
}

pub struct DupEntry {
    pub scan_id: u32,
    pub report: Arc<DupReport>,
}

/// Backend-side state of a tree view: only the expansion set lives here; the
/// flattened row list is rebuilt lazily and paged to the UI.
pub struct TreeView {
    pub scan_id: u32,
    pub root: NodeId,
    pub expanded: HashSet<NodeId>,
    pub sort: SortKey,
    pub desc: bool,
    pub rows: Option<Vec<(NodeId, u16)>>,
}

/// One run of the uninstall wizard.
pub struct UninstallSession {
    pub program: hdcleaner_core::programs::Program,
    /// Inventory snapshot taken before uninstalling (other programs' folders).
    pub command: Option<hdcleaner_core::uninstall::UninstallCommand>,
    pub quiet_command: Option<hdcleaner_core::uninstall::UninstallCommand>,
    pub stop: Arc<std::sync::atomic::AtomicBool>,
    pub outcome: Option<hdcleaner_core::uninstall::RunOutcome>,
    pub leftovers: Vec<hdcleaner_core::leftovers::Leftover>,
    pub removed: Vec<hdcleaner_core::leftovers::RemovalResult>,
    pub backup_dir: PathBuf,
    pub operation_id: Option<i64>,
    pub restore_point: Option<bool>,
    /// Forced mode: leftovers may be scanned even if still registered.
    pub forced: bool,
    /// Forced mode: the synthetic target built from the user's input.
    pub synthetic: Option<hdcleaner_core::programs::Program>,
}

pub struct AppState {
    pub scans: RwLock<HashMap<u32, SharedTree>>,
    pub jobs: Mutex<HashMap<u32, Arc<ScanControl>>>,
    pub results: Mutex<HashMap<u32, ResultSet>>,
    pub views: Mutex<HashMap<u32, TreeView>>,
    pub dups: Mutex<HashMap<u32, DupEntry>>,
    pub plans: Mutex<HashMap<String, (DeletePlan, Option<u32>)>>,
    pub db: Mutex<Database>,
    pub data_dir: PathBuf,
    /// Operations found `running` at startup (crash / forced exit).
    pub interrupted: Mutex<Vec<hdcleaner_core::db::OperationRecord>>,
    /// Installed programs inventory (loaded on first use, refreshed on demand).
    pub programs: RwLock<Option<Arc<Vec<hdcleaner_core::programs::Program>>>>,
    /// Program id → PNG icon (None = no icon available).
    pub icon_cache: Mutex<HashMap<String, Option<Arc<Vec<u8>>>>>,
    pub program_sizes: Mutex<HashMap<String, hdcleaner_core::appsize::AppSize>>,
    pub uninstalls: Mutex<HashMap<u32, UninstallSession>>,
    /// Previous process sample (CPU % is computed between two samples).
    pub sampler: Mutex<hdcleaner_core::processes::ProcessSampler>,
    /// Executable path → PNG icon (None = no icon).
    pub file_icons: Mutex<HashMap<String, Option<Arc<Vec<u8>>>>>,
    /// Boot performance log, read on request (needs admin rights).
    pub boot_report: Mutex<Option<Arc<hdcleaner_core::bootperf::BootReport>>>,
    /// Last cleaner analysis (cleaning only removes items listed here).
    pub clean_analysis: Mutex<Option<Arc<Vec<hdcleaner_core::cleaner::CategoryResult>>>>,
    next_id: AtomicU32,
}

impl AppState {
    pub fn new(db: Database, data_dir: PathBuf, interrupted: Vec<hdcleaner_core::db::OperationRecord>) -> Self {
        AppState {
            scans: RwLock::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            views: Mutex::new(HashMap::new()),
            dups: Mutex::new(HashMap::new()),
            plans: Mutex::new(HashMap::new()),
            db: Mutex::new(db),
            data_dir,
            interrupted: Mutex::new(interrupted),
            programs: RwLock::new(None),
            icon_cache: Mutex::new(HashMap::new()),
            program_sizes: Mutex::new(HashMap::new()),
            uninstalls: Mutex::new(HashMap::new()),
            sampler: Mutex::new(Default::default()),
            file_icons: Mutex::new(HashMap::new()),
            boot_report: Mutex::new(None),
            clean_analysis: Mutex::new(None),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn next_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn tree(&self, scan_id: u32) -> Result<SharedTree, hdcleaner_core::ErrorPayload> {
        self.scans.read().get(&scan_id).cloned().ok_or_else(|| {
            hdcleaner_core::AppError::InvalidInput(format!("scan {scan_id} is not loaded (it may have been closed)")).to_payload()
        })
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        self.db.lock().get_setting(key).ok().flatten().and_then(|v| v.as_bool()).unwrap_or(default)
    }

    /// Keep memory bounded: drop older scans of the same root and anything
    /// beyond `keep` loaded scans (views/results pointing to them go too).
    pub fn insert_scan(&self, tree: ScanTree, keep: usize) -> u32 {
        let id = self.next_id();
        let root = tree.meta.root_path.to_lowercase();
        let mut scans = self.scans.write();
        let stale: Vec<u32> = scans
            .iter()
            .filter(|(_, t)| t.read().meta.root_path.to_lowercase() == root)
            .map(|(k, _)| *k)
            .collect();
        for k in &stale {
            scans.remove(k);
        }
        while scans.len() >= keep {
            let Some(&oldest) = scans.keys().min() else { break };
            scans.remove(&oldest);
        }
        scans.insert(id, Arc::new(RwLock::new(tree)));
        drop(scans);
        let alive: HashSet<u32> = self.scans.read().keys().copied().collect();
        self.views.lock().retain(|_, v| alive.contains(&v.scan_id));
        self.results.lock().retain(|_, r| alive.contains(&r.scan_id));
        self.dups.lock().retain(|_, d| alive.contains(&d.scan_id));
        id
    }
}
