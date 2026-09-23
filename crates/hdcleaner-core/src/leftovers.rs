//! Leftover scanner: what an uninstaller left behind.
//!
//! Levels (each includes the previous ones):
//! * `Safe`     — items tied to the program's identity: its registered folder,
//!   its uninstall entry, shortcuts / Run entries / services / tasks whose
//!   target is inside its folder, its package data folder.
//! * `Moderate` — items that match the program by name in well-known places
//!   (AppData, ProgramData, `Software\Vendor\Product`, Start Menu folders,
//!   broken shortcuts with the program's name).
//! * `Advanced` — broader matches (vendor keys holding only this product,
//!   folders whose name *contains* the program name, Temp folders).
//!
//! Every item carries a 0-100 confidence, a reason key and a `shared` flag.
//! Shared items and anything below 70 are never pre-selected, items inside
//! other installed programs' folders are excluded, and Blocked locations
//! (Protection Engine) are never proposed.

use crate::appsize::{name_keys, norm, Confidence, Roots};
use crate::programs::Program;
use crate::protection::{assess, Risk};
use crate::regops::{deletion_allowed, key_exists, RegTarget};
use crate::registry::{Hive, Key, View};
use crate::{ErrorPayload, AppError};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Safe,
    Moderate,
    Advanced,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LeftoverKind {
    Folder,
    File,
    Shortcut,
    RegistryKey,
    RegistryValue,
    Service,
    Task,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leftover {
    pub id: u32,
    pub kind: LeftoverKind,
    /// "files" | "registry" | "shortcuts" | "startup" | "services" | "tasks"
    pub category: String,
    pub level: Level,
    pub confidence: u8,
    pub reason: String,
    /// Extra evidence shown to the user (e.g. a shortcut's target).
    pub detail: Option<String>,
    /// Display path or registry key.
    pub path: String,
    pub size: u64,
    pub shared: bool,
    pub preselected: bool,
    /// Protection Engine verdict for file system items.
    pub risk: Option<Risk>,
    pub reg: Option<RegTarget>,
    pub service: Option<(String, String)>,
    pub task: Option<String>,
}

struct Ctx {
    keys: Vec<String>,
    /// Folders known to belong to the program (for "target inside" checks).
    own_dirs: Vec<String>,
    /// Folders of every *other* installed program.
    other_dirs: Vec<String>,
    level: Level,
    out: Vec<Leftover>,
}

fn lower(s: &str) -> String {
    s.trim_end_matches('\\').to_lowercase()
}

fn within(path: &str, dir: &str) -> bool {
    let (p, d) = (lower(path), lower(dir));
    !d.is_empty() && (p == d || (p.starts_with(&d) && p.as_bytes().get(d.len()) == Some(&b'\\')))
}

impl Ctx {
    fn in_own(&self, path: &str) -> bool {
        self.own_dirs.iter().any(|d| within(path, d))
    }

    fn name_matches(&self, name: &str) -> bool {
        let n = norm(name);
        n.len() >= 3 && self.keys.iter().any(|k| *k == n)
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: LeftoverKind,
        category: &str,
        level: Level,
        confidence: u8,
        reason: &str,
        path: String,
        detail: Option<String>,
    ) -> Option<&mut Leftover> {
        if level > self.level || self.out.iter().any(|l| l.path.eq_ignore_ascii_case(&path)) {
            return None;
        }
        let mut shared = false;
        let mut risk = None;
        if matches!(kind, LeftoverKind::Folder | LeftoverKind::File | LeftoverKind::Shortcut) {
            let r = assess(&path).risk;
            if r == Risk::Blocked {
                return None;
            }
            // Never propose another program's folder; flag containers of one.
            if self.other_dirs.iter().any(|d| within(&path, d)) {
                return None;
            }
            if self.other_dirs.iter().any(|d| within(d, &path)) {
                shared = true;
            }
            // Items nested inside something already listed are redundant.
            if self.out.iter().any(|l| l.kind == LeftoverKind::Folder && within(&path, &l.path)) {
                return None;
            }
            risk = Some(r);
        }
        let id = self.out.len() as u32 + 1;
        self.out.push(Leftover {
            id,
            kind,
            category: category.to_string(),
            level,
            confidence,
            reason: reason.to_string(),
            detail,
            path,
            size: 0,
            shared,
            preselected: false,
            risk,
            reg: None,
            service: None,
            task: None,
        });
        self.out.last_mut()
    }

    fn push_reg(&mut self, level: Level, confidence: u8, reason: &str, t: RegTarget, detail: Option<String>) {
        if deletion_allowed(&t).is_err() || !key_exists(t.hive, &t.path, t.view) {
            return;
        }
        let kind = if t.value.is_some() { LeftoverKind::RegistryValue } else { LeftoverKind::RegistryKey };
        let category = if t.value.is_some() { "startup" } else { "registry" };
        let display = t.display();
        if let Some(item) = self.push(kind, category, level, confidence, reason, display, detail) {
            item.reg = Some(t);
        }
    }
}

fn dirs_of(p: &Program) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(d) = &p.install_location {
        v.push(d.clone());
    } else if let Some(d) = &p.inferred_location {
        v.push(d.clone());
    }
    v
}

fn subkeys(hive: Hive, path: &str, view: View) -> Vec<String> {
    Key::open(hive, path, view).map(|k| k.subkeys()).unwrap_or_default()
}

/// Uninstall-entry registry location of a registry program.
pub fn uninstall_entry(p: &Program) -> Option<RegTarget> {
    let id = p.id.strip_prefix("reg:")?;
    let (loc, sub) = id.split_once(':')?;
    let (hive, view, base) = match loc {
        "HKLM64" => (Hive::LocalMachine, View::Reg64, "SOFTWARE"),
        "HKLM32" => (Hive::LocalMachine, View::Reg32, "SOFTWARE"),
        "HKLM" => (Hive::LocalMachine, View::Default, "SOFTWARE"),
        _ => (Hive::CurrentUser, View::Default, "Software"),
    };
    Some(RegTarget { hive, path: format!(r"{base}\Microsoft\Windows\CurrentVersion\Uninstall\{sub}"), view, value: None })
}

/// Leftovers taken from an installation trace: what the installer itself put
/// on the machine and is still there. These are facts, not guesses, so they
/// come with high confidence — but they still go through the same protection
/// checks, and anything already found by the normal scan is not repeated.
pub fn from_trace(p: &Program, others: &[Program], trace: &crate::monitor::InstallTrace, existing: &[Leftover]) -> Vec<Leftover> {
    let own_dirs = dirs_of(p);
    let other_dirs: Vec<String> = others
        .iter()
        .filter(|o| o.id != p.id)
        .flat_map(dirs_of)
        .filter(|d| !own_dirs.iter().any(|o| within(o, d)))
        .collect();
    let mut cx = Ctx { keys: name_keys(p), own_dirs, other_dirs, level: Level::Advanced, out: existing.to_vec() };
    let before = cx.out.len();

    // Folders first (shallowest first), so the files inside them are skipped.
    let mut dirs: Vec<&crate::monitor::TraceFile> = trace.files.iter().filter(|f| f.related && !f.transient && Path::new(&f.path).is_dir()).collect();
    dirs.sort_by_key(|f| f.path.matches('\\').count());
    for d in dirs {
        cx.push(LeftoverKind::Folder, "files", Level::Safe, 95, "installTrace", d.path.clone(), None);
    }
    for f in trace.files.iter().filter(|f| f.related && !f.transient && Path::new(&f.path).is_file()) {
        if let Some(l) = cx.push(LeftoverKind::File, "files", Level::Safe, 95, "installTrace", f.path.clone(), None) {
            l.size = f.size;
        }
    }

    for r in trace.registry.iter().filter(|r| r.related) {
        let Some(target) = parse_trace_registry(&r.path) else { continue };
        if crate::regops::deletion_allowed(&target).is_err() || !crate::regops::key_exists(target.hive, &target.path, target.view) {
            continue;
        }
        let kind = if target.value.is_some() { LeftoverKind::RegistryValue } else { LeftoverKind::RegistryKey };
        let category = if target.value.is_some() { "startup" } else { "registry" };
        if let Some(l) = cx.push(kind, category, Level::Safe, 95, "installTrace", target.display(), None) {
            l.reg = Some(target);
        }
    }

    let services = crate::sysitems::services();
    for name in &trace.services {
        let Some(s) = services.iter().find(|s| s.name.eq_ignore_ascii_case(name)) else { continue };
        let image = s.image_path.clone().unwrap_or_default();
        if let Some(l) = cx.push(LeftoverKind::Service, "services", Level::Safe, 95, "installTrace", format!("{} ({})", s.display_name.clone().unwrap_or_else(|| s.name.clone()), s.name), None) {
            l.service = Some((s.name.clone(), image));
        }
    }
    let tasks = crate::sysitems::tasks();
    for path in &trace.tasks {
        let Some(t) = tasks.iter().find(|t| t.path.eq_ignore_ascii_case(path)) else { continue };
        if let Some(l) = cx.push(LeftoverKind::Task, "tasks", Level::Safe, 95, "installTrace", t.path.clone(), None) {
            l.task = Some(t.path.clone());
        }
    }
    // Same finish as a normal scan: real sizes, the path as Windows spells it,
    // and pre-selection by confidence.
    let mut out = cx.out.split_off(before);
    for l in out.iter_mut() {
        if let Some(real) = display_path(&l.path) {
            l.path = real;
        }
        l.size = match l.kind {
            LeftoverKind::Folder => crate::appsize::measure_live(&l.path, &crate::scan::ScanControl::new()).map(|m| m.total).unwrap_or(0),
            LeftoverKind::File | LeftoverKind::Shortcut => std::fs::metadata(&l.path).map(|m| m.len()).unwrap_or(l.size),
            _ => l.size,
        };
        if l.shared {
            l.confidence = l.confidence.min(40);
        }
        l.preselected = l.confidence >= 70 && !l.shared;
    }
    out
}

/// A trace stores paths lowercased; show them the way they are on disk.
fn display_path(path: &str) -> Option<String> {
    let real = std::fs::canonicalize(path).ok()?;
    let s = real.to_string_lossy();
    Some(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

/// `hklm\software\vendor\app` or `hkcu\...\run → name` → a registry target.
fn parse_trace_registry(path: &str) -> Option<crate::regops::RegTarget> {
    use crate::registry::{Hive, View};
    let (head, value) = match path.split_once(" → ") {
        Some((h, v)) => (h, Some(v.to_string())),
        None => (path, None),
    };
    let (hive, rest) = head.split_once('\\')?;
    let hive = match hive.to_lowercase().as_str() {
        "hklm" => Hive::LocalMachine,
        "hkcu" => Hive::CurrentUser,
        _ => return None,
    };
    let view = if rest.to_lowercase().contains("wow6432node") { View::Reg32 } else { View::Default };
    Some(crate::regops::RegTarget { hive, path: rest.to_string(), view, value })
}

/// Scan for leftovers of `p`. `uninstalled` must be true only once the
/// program is gone (otherwise its live uninstall entry would be proposed).
pub fn scan(p: &Program, others: &[Program], level: Level, uninstalled: bool) -> Vec<Leftover> {
    let own_dirs = dirs_of(p);
    let other_dirs: Vec<String> = others
        .iter()
        .filter(|o| o.id != p.id)
        .flat_map(dirs_of)
        // A program registered at a shared container (e.g. a whole vendor
        // folder also used by ours) must not hide our own folder.
        .filter(|d| !own_dirs.iter().any(|o| within(o, d)))
        .collect();
    let mut cx = Ctx { keys: name_keys(p), own_dirs: own_dirs.clone(), other_dirs, level, out: Vec::new() };

    // --- Safe --------------------------------------------------------------
    for d in &own_dirs {
        if Path::new(d).exists() {
            let (conf, reason) = if p.id.starts_with("forced:") {
                (90, "userSelectedFolder")
            } else if p.install_location.is_some() {
                (95, "installFolderRemains")
            } else {
                (85, "uninstallerFolderRemains")
            };
            cx.push(LeftoverKind::Folder, "files", Level::Safe, conf, reason, d.clone(), None);
        }
    }
    if uninstalled {
        if let Some(t) = uninstall_entry(p) {
            cx.push_reg(Level::Safe, 95, "uninstallEntryRemains", t, None);
        }
    }
    if let (Some(fam), Ok(local)) = (&p.package_family_name, std::env::var("LOCALAPPDATA")) {
        let d = format!(r"{local}\Packages\{fam}");
        if Path::new(&d).exists() {
            cx.push(LeftoverKind::Folder, "files", Level::Safe, 95, "packageDataFolder", d, None);
        }
    }
    let uninstaller = p.uninstall_string.as_deref().and_then(crate::programs::exe_from_command);

    // Shortcuts.
    let roots = crate::shortcuts::shortcut_roots();
    let links = crate::shortcuts::find_shortcuts(&roots);
    let targets = crate::shortcuts::resolve_targets(&links);
    for (link, target) in links.iter().zip(targets) {
        let lp = link.to_string_lossy().into_owned();
        match target {
            Some(t) if cx.in_own(&t) || uninstaller.as_deref().is_some_and(|u| u.eq_ignore_ascii_case(&t)) => {
                cx.push(LeftoverKind::Shortcut, "shortcuts", Level::Safe, 90, "shortcutTargetsProgram", lp, Some(t));
            }
            Some(t) if !Path::new(&t).exists() && cx.name_matches(&link.file_stem().unwrap_or_default().to_string_lossy()) => {
                cx.push(LeftoverKind::Shortcut, "shortcuts", Level::Moderate, 70, "brokenShortcutMatchesName", lp, Some(t));
            }
            _ => {}
        }
    }

    // Startup entries.
    for e in crate::sysitems::run_entries() {
        let target = RegTarget { hive: e.hive, path: e.key.clone(), view: e.view, value: Some(e.name.clone()) };
        match &e.exe {
            Some(x) if cx.in_own(x) => cx.push_reg(Level::Safe, 90, "startupEntryTargetsProgram", target, Some(e.command.clone())),
            Some(x) if !Path::new(x).exists() && cx.name_matches(&e.name) => {
                cx.push_reg(Level::Moderate, 65, "brokenStartupEntryMatchesName", target, Some(e.command.clone()))
            }
            _ => {}
        }
    }

    // Services.
    for s in crate::sysitems::services() {
        let (Some(exe), Some(image)) = (&s.exe, &s.image_path) else { continue };
        let by_folder = cx.in_own(exe);
        let by_name = !Path::new(exe).exists() && (cx.name_matches(&s.name) || s.display_name.as_deref().is_some_and(|d| cx.name_matches(d)));
        let (level, conf, reason) = if by_folder {
            (Level::Safe, 90, "serviceBinaryInProgramFolder")
        } else if by_name {
            (Level::Moderate, 60, "brokenServiceMatchesName")
        } else {
            continue;
        };
        let label = format!("{} ({})", s.display_name.clone().unwrap_or_else(|| s.name.clone()), s.name);
        if let Some(item) = cx.push(LeftoverKind::Service, "services", level, conf, reason, label, Some(image.clone())) {
            item.service = Some((s.name.clone(), image.clone()));
        }
    }

    // Scheduled tasks.
    for t in crate::sysitems::tasks() {
        let by_folder = t.exes.iter().any(|x| cx.in_own(x));
        let by_name = t.exes.iter().all(|x| !Path::new(x).exists()) && cx.name_matches(&t.name);
        let (level, conf, reason) = if by_folder {
            (Level::Safe, 90, "taskRunsProgram")
        } else if by_name {
            (Level::Moderate, 60, "brokenTaskMatchesName")
        } else {
            continue;
        };
        let detail = t.exes.first().cloned();
        if let Some(item) = cx.push(LeftoverKind::Task, "tasks", level, conf, reason, t.path.clone(), detail) {
            item.task = Some(t.path.clone());
        }
    }

    // App Paths entries pointing into the program folder.
    for (hive, view, base) in [
        (Hive::LocalMachine, View::Reg64, "SOFTWARE"),
        (Hive::LocalMachine, View::Reg32, "SOFTWARE"),
        (Hive::CurrentUser, View::Default, "Software"),
    ] {
        let ap = format!(r"{base}\Microsoft\Windows\CurrentVersion\App Paths");
        for sub in subkeys(hive, &ap, view) {
            let path = format!(r"{ap}\{sub}");
            let target = Key::open(hive, &path, view).and_then(|k| k.string(""));
            if let Some(t) = target.map(|t| t.trim_matches('"').to_string()) {
                if cx.in_own(&t) {
                    cx.push_reg(Level::Safe, 90, "appPathTargetsProgram", RegTarget { hive, path, view, value: None }, Some(t));
                }
            }
        }
    }

    // --- Moderate --------------------------------------------------------------
    let roots_cache = Roots::load();
    for c in crate::appsize::candidates(p, &roots_cache) {
        if c.confidence == Confidence::Possible && Path::new(&c.path).exists() {
            let conf = if c.reason == "publisherAndNameFolder" { 75 } else { 70 };
            cx.push(LeftoverKind::Folder, "files", Level::Moderate, conf, c.reason, c.path, None);
        }
    }
    // Start Menu folders named like the program.
    for root in roots.iter().filter(|r| r.ends_with("Programs")) {
        for dir in std::fs::read_dir(root).into_iter().flatten().flatten() {
            if dir.file_type().map(|t| t.is_dir()).unwrap_or(false) && cx.name_matches(&dir.file_name().to_string_lossy()) {
                cx.push(LeftoverKind::Folder, "shortcuts", Level::Moderate, 70, "startMenuFolderMatchesName", dir.path().to_string_lossy().into_owned(), None);
            }
        }
    }
    // Registry: Software\Name and Software\Vendor\Name, in every view.
    let publisher = p.publisher.as_deref().map(norm).filter(|s| s.len() >= 3);
    for (hive, view, base) in [
        (Hive::CurrentUser, View::Default, "Software"),
        (Hive::LocalMachine, View::Reg64, "SOFTWARE"),
        (Hive::LocalMachine, View::Reg32, "SOFTWARE"),
    ] {
        for top in subkeys(hive, base, view) {
            let path = format!(r"{base}\{top}");
            if cx.name_matches(&top) {
                cx.push_reg(Level::Moderate, 70, "registryKeyMatchesName", RegTarget { hive, path, view, value: None }, None);
                continue;
            }
            let nt = norm(&top);
            if publisher.as_ref().is_some_and(|pb| *pb == nt || (nt.len() >= 4 && pb.starts_with(&nt))) {
                let children = subkeys(hive, &path, view);
                let matching: Vec<&String> = children.iter().filter(|c| cx.name_matches(c)).collect();
                for m in &matching {
                    cx.push_reg(
                        Level::Moderate,
                        75,
                        "registryKeyUnderPublisher",
                        RegTarget { hive, path: format!(r"{path}\{m}"), view, value: None },
                        None,
                    );
                }
                // --- Advanced: the vendor key itself when it holds nothing else.
                if children.len() == matching.len() {
                    cx.push_reg(Level::Advanced, 50, "publisherKeyOnlyThisProduct", RegTarget { hive, path, view, value: None }, None);
                }
            }
        }
    }

    // --- Advanced --------------------------------------------------------------
    if level >= Level::Advanced {
        let long_keys: Vec<String> = cx.keys.iter().filter(|k| k.len() >= 5).cloned().collect();
        let mut extra_roots: Vec<String> = ["LOCALAPPDATA", "APPDATA", "ProgramData", "TEMP"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .collect();
        extra_roots.dedup();
        for root in extra_roots {
            for dir in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                if !dir.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let n = norm(&dir.file_name().to_string_lossy());
                if n.len() > 3 && long_keys.iter().any(|k| n != *k && n.contains(k.as_str())) {
                    cx.push(LeftoverKind::Folder, "files", Level::Advanced, 45, "folderNameContainsProgram", dir.path().to_string_lossy().into_owned(), None);
                }
            }
        }
    }

    // Sizes and pre-selection.
    let mut out = cx.out;
    for l in out.iter_mut() {
        l.size = match l.kind {
            LeftoverKind::Folder => crate::appsize::measure_live(&l.path, &crate::scan::ScanControl::new()).map(|m| m.total).unwrap_or(0),
            LeftoverKind::File | LeftoverKind::Shortcut => std::fs::metadata(&l.path).map(|m| m.len()).unwrap_or(0),
            _ => 0,
        };
        if l.shared {
            l.confidence = l.confidence.min(40);
        }
        l.preselected = l.confidence >= 70 && !l.shared;
    }
    out.sort_by(|a, b| a.level.cmp(&b.level).then(b.confidence.cmp(&a.confidence)).then(a.category.cmp(&b.category)));
    out
}

// ---- removal -------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "op")]
pub enum PendingOp {
    DeletePath { path: String, recycle: bool },
    DeleteRegistry { target: RegTarget },
    DeleteService { name: String, image: String },
    DeleteTask { path: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalResult {
    pub id: u32,
    pub path: String,
    /// "removed" | "failed" | "skipped" | "needsElevation" | "wouldRemove"
    pub status: &'static str,
    pub error: Option<ErrorPayload>,
}

pub struct RemovalOptions<'a> {
    pub backup_dir: &'a Path,
    pub recycle: bool,
    pub allow_dangerous: bool,
    pub dry_run: bool,
    /// Copy small files and folders into the backup before removing them, so
    /// the Backups page can put them back.
    pub quarantine: bool,
}

/// What a removal did: the per-item result, the items that need the elevated
/// helper, and what was saved into the backup folder.
pub struct Removal {
    pub results: Vec<RemovalResult>,
    pub pending: Vec<(u32, PendingOp)>,
    pub saved: Vec<crate::backups::Entry>,
}

pub fn needs_elevation(e: &AppError) -> bool {
    matches!(e, AppError::AccessDenied { .. } | AppError::ElevationRequired(_))
}

/// Remove the selected leftovers. Registry items are exported to
/// `backup_dir\registry.reg` first; files go to the Recycle Bin when
/// `recycle`. Items refused for lack of rights are returned as pending
/// elevated operations (the caller batches them into one UAC prompt).
pub fn remove(items: &[Leftover], opts: &RemovalOptions) -> Removal {
    use crate::backups::{Entry, EntryKind};
    let mut results = Vec::new();
    let mut pending = Vec::new();
    let mut saved: Vec<Entry> = Vec::new();
    let res = |l: &Leftover, status: &'static str, error: Option<ErrorPayload>| RemovalResult { id: l.id, path: l.path.clone(), status, error };

    let regs: Vec<RegTarget> = items.iter().filter_map(|l| l.reg.clone()).collect();
    if !regs.is_empty() && !opts.dry_run {
        if let Err(e) = crate::regops::export(&regs, &opts.backup_dir.join("registry.reg")) {
            // Without a backup no registry item is touched.
            for l in items.iter().filter(|l| l.reg.is_some()) {
                results.push(res(l, "failed", Some(e.to_payload())));
            }
            return Removal { results, pending, saved };
        }
        for l in items.iter().filter(|l| l.reg.is_some()) {
            saved.push(Entry { kind: EntryKind::Registry, path: l.path.clone(), stored: Some("registry.reg".into()), size: 0 });
        }
    }
    let mut budget = crate::backups::QUARANTINE_TOTAL_LIMIT;

    for l in items {
        if opts.dry_run {
            results.push(res(l, "wouldRemove", None));
            continue;
        }
        match l.kind {
            LeftoverKind::RegistryKey | LeftoverKind::RegistryValue => {
                let Some(t) = &l.reg else { continue };
                if results.iter().any(|r| r.id == l.id) {
                    continue; // already failed (no backup)
                }
                match crate::regops::delete(t) {
                    Ok(()) => results.push(res(l, "removed", None)),
                    Err(e) if needs_elevation(&e) => pending.push((l.id, PendingOp::DeleteRegistry { target: t.clone() })),
                    Err(e) => results.push(res(l, "failed", Some(e.to_payload()))),
                }
            }
            LeftoverKind::Folder | LeftoverKind::File | LeftoverKind::Shortcut => {
                if l.risk == Some(Risk::Dangerous) && !opts.allow_dangerous {
                    results.push(res(l, "skipped", Some(AppError::Protected { path: l.path.clone(), reason: "dangerousNotConfirmed".into() }.to_payload())));
                    continue;
                }
                // Saved first: after the delete there is nothing left to copy.
                let quarantined = if opts.quarantine && !opts.dry_run {
                    crate::backups::quarantine(Path::new(&l.path), opts.backup_dir, saved.len(), &mut budget)
                } else {
                    None
                };
                let mode = if opts.recycle { crate::fsops::DeleteMode::RecycleBin } else { crate::fsops::DeleteMode::Permanent };
                let plan = crate::fsops::plan_delete(&[(l.path.clone(), None)], mode);
                if plan.items.is_empty() {
                    results.push(res(l, "removed", None));
                    continue;
                }
                let r = crate::fsops::execute_delete(
                    &plan,
                    &crate::fsops::ExecuteOptions { dry_run: false, allow_dangerous: opts.allow_dangerous },
                    |_, _| {},
                    || true,
                );
                let outcome = r.into_iter().next().map(|x| x.outcome);
                if let Some((stored, size)) = quarantined {
                    if matches!(outcome, Some(crate::fsops::ItemOutcome::Deleted)) {
                        let kind = if l.kind == LeftoverKind::Folder { EntryKind::Folder } else { EntryKind::File };
                        saved.push(Entry { kind, path: l.path.clone(), stored: Some(stored), size });
                    } else {
                        // Nothing was removed: the copy would only take space.
                        let _ = std::fs::remove_dir_all(opts.backup_dir.join(&stored));
                        let _ = std::fs::remove_file(opts.backup_dir.join(&stored));
                    }
                }
                match outcome {
                    Some(crate::fsops::ItemOutcome::Deleted) => results.push(res(l, "removed", None)),
                    Some(crate::fsops::ItemOutcome::Failed { error }) if error.kind == "notFound" => results.push(res(l, "removed", None)),
                    Some(crate::fsops::ItemOutcome::Failed { error }) if error.kind == "accessDenied" => {
                        pending.push((l.id, PendingOp::DeletePath { path: l.path.clone(), recycle: opts.recycle }))
                    }
                    Some(crate::fsops::ItemOutcome::Failed { error }) => results.push(res(l, "failed", Some(error))),
                    Some(crate::fsops::ItemOutcome::Skipped { reason }) => results.push(res(
                        l,
                        "skipped",
                        Some(AppError::Protected { path: l.path.clone(), reason }.to_payload()),
                    )),
                    _ => results.push(res(l, "failed", None)),
                }
            }
            LeftoverKind::Service => {
                let Some((name, image)) = &l.service else { continue };
                match crate::sysitems::delete_service(name, image) {
                    Ok(()) => results.push(res(l, "removed", None)),
                    Err(e) if needs_elevation(&e) => pending.push((l.id, PendingOp::DeleteService { name: name.clone(), image: image.clone() })),
                    Err(e) => results.push(res(l, "failed", Some(e.to_payload()))),
                }
            }
            LeftoverKind::Task => {
                let Some(path) = &l.task else { continue };
                match crate::sysitems::delete_task(path) {
                    Ok(()) => results.push(res(l, "removed", None)),
                    Err(e) if needs_elevation(&e) || e.to_string().contains("0x80070005") => {
                        pending.push((l.id, PendingOp::DeleteTask { path: path.clone() }))
                    }
                    Err(e) => results.push(res(l, "failed", Some(e.to_payload()))),
                }
            }
        }
    }
    Removal { results, pending, saved }
}

/// Execute one pending operation (inside the elevated helper). Every check is
/// repeated here: the helper never trusts the caller.
pub fn apply_pending(op: &PendingOp) -> crate::Result<()> {
    match op {
        PendingOp::DeletePath { path, recycle } => {
            let mode = if *recycle { crate::fsops::DeleteMode::RecycleBin } else { crate::fsops::DeleteMode::Permanent };
            let plan = crate::fsops::plan_delete(&[(path.clone(), None)], mode);
            let r = crate::fsops::execute_delete(&plan, &crate::fsops::ExecuteOptions { dry_run: false, allow_dangerous: true }, |_, _| {}, || true);
            match r.into_iter().next().map(|x| x.outcome) {
                None | Some(crate::fsops::ItemOutcome::Deleted) => Ok(()),
                Some(crate::fsops::ItemOutcome::Skipped { reason }) => Err(AppError::Protected { path: path.clone(), reason }),
                Some(crate::fsops::ItemOutcome::Failed { error }) => Err(AppError::Helper(error.message)),
                Some(crate::fsops::ItemOutcome::WouldDelete) => Ok(()),
            }
        }
        PendingOp::DeleteRegistry { target } => crate::regops::delete(target),
        PendingOp::DeleteService { name, image } => crate::sysitems::delete_service(name, image),
        PendingOp::DeleteTask { path } => crate::sysitems::delete_task(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_checks_separators() {
        assert!(within(r"C:\A\B\c.txt", r"C:\a\b"));
        assert!(within(r"C:\A\B", r"c:\a\b\"));
        assert!(!within(r"C:\A\BC", r"C:\A\B"));
    }

    #[test]
    fn uninstall_entry_mapping() {
        let p = Program::blank("reg:HKLM32:{ABC}".into(), "X".into(), crate::programs::ProgramSource::Msi);
        let t = uninstall_entry(&p).unwrap();
        assert_eq!(t.view, View::Reg32);
        assert!(t.path.ends_with(r"Uninstall\{ABC}"));
        assert!(deletion_allowed(&t).is_ok());
    }
}
